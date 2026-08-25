use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use chrono::DateTime;
use rusqlite::{Connection as SqliteConnection, OpenFlags};
use serde_json::Value;
use walkdir::WalkDir;

use crate::AppError;
use crate::storage::Storage;

const MAX_TOOL_OUTPUT_CHARS: usize = 128 * 1024;

pub(crate) struct ProviderRoots {
    claude: PathBuf,
    codex: PathBuf,
    opencode: PathBuf,
    copilot: PathBuf,
    hermes: PathBuf,
}

pub(crate) struct IndexSummary {
    pub(crate) scanned_files: u64,
    pub(crate) malformed_records: u64,
}

pub(crate) struct Conversation {
    pub(crate) id: String,
    pub(crate) provider: &'static str,
    pub(crate) source_path: PathBuf,
    pub(crate) title: Option<String>,
    pub(crate) created_at: Option<i64>,
    pub(crate) updated_at: Option<i64>,
    pub(crate) messages: Vec<NormalizedMessage>,
}

pub(crate) struct NormalizedMessage {
    pub(crate) id: String,
    pub(crate) ordinal: i64,
    pub(crate) role: String,
    pub(crate) content: String,
    pub(crate) created_at: Option<i64>,
}

struct ParsedFile {
    conversation: Option<Conversation>,
    malformed_records: u64,
}

type ExtractedMessage = (Option<String>, String, String, Option<i64>, Option<String>);

impl ProviderRoots {
    pub(crate) fn new(
        claude: Option<PathBuf>,
        codex: Option<PathBuf>,
        opencode: Option<PathBuf>,
        copilot: Option<PathBuf>,
        hermes: Option<PathBuf>,
    ) -> Self {
        let home = std::env::var_os("HOME").map_or_else(PathBuf::new, PathBuf::from);
        Self {
            claude: claude.unwrap_or_else(|| home.join(".claude/projects")),
            codex: codex.unwrap_or_else(|| home.join(".codex/sessions")),
            opencode: opencode.unwrap_or_else(|| home.join(".local/share/opencode/opencode.db")),
            copilot: copilot.unwrap_or_else(|| home.join(".copilot/session-state")),
            hermes: hermes.unwrap_or_else(|| home.join(".hermes/state.db")),
        }
    }
}

pub(crate) fn index(
    storage: &mut Storage,
    roots: &ProviderRoots,
) -> Result<IndexSummary, AppError> {
    let mut summary = IndexSummary {
        scanned_files: 0,
        malformed_records: 0,
    };
    for (provider, root) in [("claude-code", &roots.claude), ("codex", &roots.codex)] {
        if !root.is_dir() {
            continue;
        }
        for entry in WalkDir::new(root).follow_links(false) {
            let entry = entry.map_err(|error| AppError::internal(error.to_string()))?;
            let path = entry.path();
            if !entry.file_type().is_file()
                || path
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .is_none_or(|extension| !extension.eq_ignore_ascii_case("jsonl"))
            {
                continue;
            }
            summary.scanned_files += 1;
            let parsed = if provider == "claude-code" {
                parse_claude(path)?
            } else {
                parse_codex(path)?
            };
            summary.malformed_records += parsed.malformed_records;
            if let Some(conversation) = parsed.conversation {
                storage.replace_conversation(&conversation)?;
            }
        }
    }
    if roots.opencode.is_file() {
        summary.scanned_files += 1;
        let parsed = parse_opencode(&roots.opencode)?;
        summary.malformed_records += parsed.malformed_records;
        for conversation in parsed.conversations {
            storage.replace_conversation(&conversation)?;
        }
    }
    if roots.copilot.is_dir() {
        for entry in WalkDir::new(&roots.copilot).follow_links(false) {
            let entry = entry.map_err(|error| AppError::internal(error.to_string()))?;
            let path = entry.path();
            if !entry.file_type().is_file()
                || path.file_name().and_then(|name| name.to_str()) != Some("events.jsonl")
            {
                continue;
            }
            summary.scanned_files += 1;
            let parsed = parse_copilot(path)?;
            summary.malformed_records += parsed.malformed_records;
            if let Some(conversation) = parsed.conversation {
                storage.replace_conversation(&conversation)?;
            }
        }
    }
    if roots.hermes.is_file() {
        summary.scanned_files += 1;
        for conversation in parse_hermes(&roots.hermes)? {
            storage.replace_conversation(&conversation)?;
        }
    }
    Ok(summary)
}

struct ParsedDatabase {
    conversations: Vec<Conversation>,
    malformed_records: u64,
}

fn parse_opencode(path: &Path) -> Result<ParsedDatabase, AppError> {
    let connection = SqliteConnection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(AppError::database)?;
    let mut conversations = Vec::new();
    let mut malformed_records = 0;
    for (session_id, title, created_at, updated_at) in opencode_sessions(&connection)? {
        let mut message_query = connection
            .prepare(
                "SELECT id, data, time_created
                 FROM message WHERE session_id = ?1
                 ORDER BY time_created, id",
            )
            .map_err(AppError::database)?;
        let rows = message_query
            .query_map([&session_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                ))
            })
            .map_err(AppError::database)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(AppError::database)?;
        drop(message_query);

        let mut messages = Vec::new();
        for (source_id, message_data, message_created_at) in rows {
            let role = if let Ok(data) = serde_json::from_str::<Value>(&message_data) {
                data.get("role")
                    .and_then(Value::as_str)
                    .unwrap_or("assistant")
                    .to_owned()
            } else {
                malformed_records += 1;
                continue;
            };
            let Some(content) =
                opencode_message_content(&connection, &source_id, &mut malformed_records)?
            else {
                continue;
            };
            let ordinal = i64::try_from(messages.len())
                .map_err(|_| AppError::internal("conversation contains too many messages"))?;
            messages.push(NormalizedMessage {
                id: stable_id(
                    "message",
                    &["opencode", &path.to_string_lossy(), &source_id],
                ),
                ordinal,
                role,
                content,
                created_at: message_created_at,
            });
        }
        if messages.is_empty() {
            continue;
        }
        conversations.push(Conversation {
            id: session_id.clone(),
            provider: "opencode",
            source_path: PathBuf::from(format!("{}#{session_id}", path.display())),
            title,
            created_at,
            updated_at,
            messages,
        });
    }
    Ok(ParsedDatabase {
        conversations,
        malformed_records,
    })
}

type OpenCodeSession = (String, Option<String>, Option<i64>, Option<i64>);

fn opencode_sessions(connection: &SqliteConnection) -> Result<Vec<OpenCodeSession>, AppError> {
    let mut query = connection
        .prepare(
            "SELECT id, title, time_created, time_updated
             FROM session ORDER BY time_created, id",
        )
        .map_err(AppError::database)?;
    query
        .query_map([], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })
        .map_err(AppError::database)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(AppError::database)
}

fn opencode_message_content(
    connection: &SqliteConnection,
    message_id: &str,
    malformed_records: &mut u64,
) -> Result<Option<String>, AppError> {
    let mut query = connection
        .prepare(
            "SELECT data FROM part WHERE message_id = ?1
             ORDER BY time_created, id",
        )
        .map_err(AppError::database)?;
    let parts = query
        .query_map([message_id], |row| row.get::<_, String>(0))
        .map_err(AppError::database)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(AppError::database)?;
    let mut content = Vec::new();
    for part in parts {
        if let Ok(data) = serde_json::from_str::<Value>(&part) {
            let part_type = data.get("type").and_then(Value::as_str);
            if matches!(part_type, Some("text" | "reasoning"))
                && let Some(text) = data.get("text").and_then(Value::as_str)
                && !text.trim().is_empty()
            {
                content.push(text.to_owned());
            }
        } else {
            *malformed_records += 1;
        }
    }
    Ok((!content.is_empty()).then(|| content.join("\n")))
}

fn parse_copilot(path: &Path) -> Result<ParsedFile, AppError> {
    let mut parsed = parse_jsonl(path, "github-copilot", |raw| {
        let role = match raw.get("type").and_then(Value::as_str)? {
            "user.message" => "user",
            "assistant.message" => "assistant",
            _ => return None,
        };
        let payload = raw.get("data").unwrap_or(raw);
        let content = payload
            .get("content")
            .or_else(|| payload.get("message"))
            .or_else(|| payload.get("text"))
            .and_then(flatten_content)?;
        Some((
            raw.get("id").and_then(Value::as_str).map(str::to_owned),
            role.to_owned(),
            content,
            parse_timestamp(raw.get("timestamp")),
            None,
        ))
    })?;
    if let Some(conversation) = &mut parsed.conversation
        && let Some(session_id) = path
            .parent()
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
    {
        session_id.clone_into(&mut conversation.id);
    }
    Ok(parsed)
}

fn parse_hermes(path: &Path) -> Result<Vec<Conversation>, AppError> {
    let connection = SqliteConnection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(AppError::database)?;
    let mut query = connection
        .prepare(
            "SELECT id, title,
                    CAST(started_at * 1000 AS INTEGER),
                    CAST(ended_at * 1000 AS INTEGER)
             FROM sessions ORDER BY started_at, id",
        )
        .map_err(AppError::database)?;
    let sessions = query
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<i64>>(2)?,
                row.get::<_, Option<i64>>(3)?,
            ))
        })
        .map_err(AppError::database)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(AppError::database)?;
    drop(query);

    let mut conversations = Vec::new();
    for (session_id, title, created_at, updated_at) in sessions {
        let mut message_query = connection
            .prepare(
                "SELECT id, role, content, reasoning,
                        CAST(timestamp * 1000 AS INTEGER)
                 FROM messages WHERE session_id = ?1
                 ORDER BY timestamp, id",
            )
            .map_err(AppError::database)?;
        let rows = message_query
            .query_map([&session_id], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<i64>>(4)?,
                ))
            })
            .map_err(AppError::database)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(AppError::database)?;
        drop(message_query);

        let mut messages = Vec::new();
        for (source_id, role, content, reasoning, message_created_at) in rows {
            if role == "session_meta" {
                continue;
            }
            let content = [content, reasoning]
                .into_iter()
                .flatten()
                .filter(|text| !text.trim().is_empty())
                .collect::<Vec<_>>()
                .join("\n\n[reasoning]\n");
            if content.is_empty() {
                continue;
            }
            let ordinal = i64::try_from(messages.len())
                .map_err(|_| AppError::internal("conversation contains too many messages"))?;
            messages.push(NormalizedMessage {
                id: stable_id(
                    "message",
                    &["hermes", &path.to_string_lossy(), &source_id.to_string()],
                ),
                ordinal,
                role,
                content,
                created_at: message_created_at,
            });
        }
        if messages.is_empty() {
            continue;
        }
        conversations.push(Conversation {
            id: session_id.clone(),
            provider: "hermes",
            source_path: PathBuf::from(format!("{}#{session_id}", path.display())),
            title,
            created_at,
            updated_at,
            messages,
        });
    }
    Ok(conversations)
}

fn parse_claude(path: &Path) -> Result<ParsedFile, AppError> {
    parse_jsonl(path, "claude-code", |raw| {
        let entry_type = raw.get("type").and_then(Value::as_str)?;
        if !matches!(entry_type, "user" | "assistant") {
            return None;
        }
        let message = raw.get("message")?;
        let role = message
            .get("role")
            .and_then(Value::as_str)
            .unwrap_or(entry_type);
        let content = flatten_content(message.get("content")?)?;
        Some((
            raw.get("uuid").and_then(Value::as_str).map(str::to_owned),
            role.to_owned(),
            content,
            parse_timestamp(raw.get("timestamp")),
            raw.get("sessionId")
                .and_then(Value::as_str)
                .map(str::to_owned),
        ))
    })
}

fn parse_codex(path: &Path) -> Result<ParsedFile, AppError> {
    parse_jsonl(path, "codex", |raw| {
        if raw.get("type").and_then(Value::as_str) != Some("response_item") {
            return None;
        }
        let payload = raw.get("payload")?;
        let payload_type = payload.get("type").and_then(Value::as_str)?;
        let (role, content) = match payload_type {
            "message" => (
                payload
                    .get("role")
                    .and_then(Value::as_str)
                    .unwrap_or("assistant")
                    .to_owned(),
                flatten_content(payload.get("content")?)?,
            ),
            "function_call" | "custom_tool_call" => {
                let name = payload
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown");
                let arguments = payload
                    .get("arguments")
                    .or_else(|| payload.get("input"))
                    .map(stringify_value)
                    .unwrap_or_default();
                ("assistant".to_owned(), format!("Tool {name}: {arguments}"))
            }
            "function_call_output" | "custom_tool_call_output" => {
                let output = payload.get("output").map(stringify_value)?;
                (
                    "tool".to_owned(),
                    truncate_chars(&output, MAX_TOOL_OUTPUT_CHARS),
                )
            }
            _ => return None,
        };
        Some((
            payload
                .get("id")
                .or_else(|| payload.get("call_id"))
                .and_then(Value::as_str)
                .map(str::to_owned),
            role,
            content,
            parse_timestamp(raw.get("timestamp")),
            None,
        ))
    })
}

fn parse_jsonl(
    path: &Path,
    provider: &'static str,
    mut extract: impl FnMut(&Value) -> Option<ExtractedMessage>,
) -> Result<ParsedFile, AppError> {
    let file = File::open(path).map_err(AppError::io)?;
    let mut malformed_records = 0;
    let mut messages = Vec::new();
    let mut session_id = None;
    for line in BufReader::new(file).lines() {
        let line = line.map_err(AppError::io)?;
        if line.trim().is_empty() {
            continue;
        }
        let raw: Value = if let Ok(raw) = serde_json::from_str(&line) {
            raw
        } else {
            malformed_records += 1;
            continue;
        };
        if provider == "codex" && raw.get("type").and_then(Value::as_str) == Some("session_meta") {
            session_id = raw
                .get("payload")
                .and_then(|payload| payload.get("id").or_else(|| payload.get("session_id")))
                .and_then(Value::as_str)
                .map(str::to_owned);
        }
        let Some((source_id, role, content, created_at, message_session_id)) = extract(&raw) else {
            continue;
        };
        if content.trim().is_empty() {
            continue;
        }
        if session_id.is_none() {
            session_id = message_session_id;
        }
        let ordinal = i64::try_from(messages.len())
            .map_err(|_| AppError::internal("conversation contains too many messages"))?;
        let message_key = source_id.unwrap_or_else(|| ordinal.to_string());
        messages.push(NormalizedMessage {
            id: stable_id(
                "message",
                &[provider, &path.to_string_lossy(), &message_key],
            ),
            ordinal,
            role,
            content,
            created_at,
        });
    }
    if messages.is_empty() {
        return Ok(ParsedFile {
            conversation: None,
            malformed_records,
        });
    }
    let id =
        session_id.unwrap_or_else(|| stable_id("session", &[provider, &path.to_string_lossy()]));
    let created_at = messages
        .iter()
        .filter_map(|message| message.created_at)
        .min();
    let updated_at = messages
        .iter()
        .filter_map(|message| message.created_at)
        .max();
    let title = messages
        .iter()
        .find(|message| message.role == "user")
        .map(|message| truncate_chars(message.content.trim(), 120));
    Ok(ParsedFile {
        conversation: Some(Conversation {
            id,
            provider,
            source_path: path.to_path_buf(),
            title,
            created_at,
            updated_at,
            messages,
        }),
        malformed_records,
    })
}

fn flatten_content(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Array(blocks) => {
            let parts: Vec<String> = blocks.iter().filter_map(flatten_block).collect();
            (!parts.is_empty()).then(|| parts.join("\n"))
        }
        Value::Object(_) => flatten_block(value),
        _ => None,
    }
}

fn flatten_block(block: &Value) -> Option<String> {
    if let Some(text) = block
        .get("text")
        .or_else(|| block.get("content"))
        .and_then(Value::as_str)
    {
        return Some(text.to_owned());
    }
    match block.get("type").and_then(Value::as_str) {
        Some("tool_use") => {
            let name = block
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            let input = block.get("input").map(stringify_value).unwrap_or_default();
            Some(format!("Tool {name}: {input}"))
        }
        Some("tool_result") => block.get("content").and_then(flatten_content),
        Some("input_text" | "output_text") => {
            block.get("text").and_then(Value::as_str).map(str::to_owned)
        }
        _ => None,
    }
}

fn stringify_value(value: &Value) -> String {
    value
        .as_str()
        .map_or_else(|| value.to_string(), str::to_owned)
}

fn stable_id(domain: &str, parts: &[&str]) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(domain.as_bytes());
    for part in parts {
        hasher.update(&[0]);
        hasher.update(part.as_bytes());
    }
    hasher.finalize().to_hex()[..24].to_owned()
}

fn parse_timestamp(value: Option<&Value>) -> Option<i64> {
    DateTime::parse_from_rfc3339(value?.as_str()?)
        .ok()
        .map(|timestamp| timestamp.timestamp_millis())
}

fn truncate_chars(value: &str, limit: usize) -> String {
    value.chars().take(limit).collect()
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    // Veritas claims: ingestion/provider-boundary,
    // ingestion/malformed-records-do-not-panic
    #[test]
    fn claude_parser_skips_malformed_lines_and_keeps_messages() {
        let mut file = tempfile::NamedTempFile::new().expect("temporary JSONL");
        writeln!(file, "not-json").expect("malformed line");
        writeln!(file, r#"{{"type":"user","sessionId":"s1","uuid":"u1","timestamp":"2026-01-01T00:00:00Z","message":{{"role":"user","content":"hello"}}}}"#).expect("user line");
        writeln!(file, r#"{{"type":"assistant","sessionId":"s1","uuid":"a1","message":{{"role":"assistant","content":[{{"type":"text","text":"world"}}]}}}}"#).expect("assistant line");

        let parsed = parse_claude(file.path()).expect("parse Claude history");
        let conversation = parsed.conversation.expect("conversation");
        assert_eq!(parsed.malformed_records, 1);
        assert_eq!(conversation.id, "s1");
        assert_eq!(conversation.messages.len(), 2);
        assert_eq!(conversation.messages[1].content, "world");
    }

    // Veritas claim: ingestion/provider-boundary
    #[test]
    fn codex_parser_keeps_messages_and_tool_calls() {
        let mut file = tempfile::NamedTempFile::new().expect("temporary JSONL");
        writeln!(file, r#"{{"type":"session_meta","payload":{{"id":"c1"}}}}"#).expect("meta line");
        writeln!(file, r#"{{"type":"response_item","payload":{{"type":"message","id":"m1","role":"user","content":[{{"type":"input_text","text":"find it"}}]}}}}"#).expect("message line");
        writeln!(file, r#"{{"type":"response_item","payload":{{"type":"function_call","call_id":"call1","name":"search","arguments":"{{\"q\":\"needle\"}}"}}}}"#).expect("tool line");

        let parsed = parse_codex(file.path()).expect("parse Codex history");
        let conversation = parsed.conversation.expect("conversation");
        assert_eq!(conversation.id, "c1");
        assert_eq!(conversation.messages.len(), 2);
        assert!(conversation.messages[1].content.contains("Tool search"));
    }

    // Veritas claims: ingestion/provider-boundary,
    // ingestion/supported-jsonl-indexes
    #[test]
    fn codex_parser_keeps_custom_tool_calls() {
        let mut file = tempfile::NamedTempFile::new().expect("temporary JSONL");
        writeln!(file, r#"{{"type":"session_meta","payload":{{"id":"c2"}}}}"#).expect("meta line");
        writeln!(
            file,
            r#"{{"type":"response_item","payload":{{"type":"custom_tool_call","id":"tool1","call_id":"call1","name":"imagegen","input":"draw a fox"}}}}"#
        )
        .expect("custom tool line");
        writeln!(
            file,
            r#"{{"type":"response_item","payload":{{"type":"custom_tool_call_output","id":"tool2","call_id":"call1","output":"created fox.png"}}}}"#
        )
        .expect("custom tool output line");

        let parsed = parse_codex(file.path()).expect("parse Codex history");
        let conversation = parsed.conversation.expect("conversation");
        assert_eq!(conversation.id, "c2");
        assert_eq!(conversation.messages.len(), 2);
        assert_eq!(
            conversation.messages[0].content,
            "Tool imagegen: draw a fox"
        );
        assert_eq!(conversation.messages[1].role, "tool");
        assert_eq!(conversation.messages[1].content, "created fox.png");
    }
}
