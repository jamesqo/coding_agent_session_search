use std::collections::BTreeSet;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use chrono::DateTime;
use serde_json::Value;
use walkdir::WalkDir;

use crate::AppError;
use crate::storage::Storage;

const MAX_TOOL_OUTPUT_CHARS: usize = 128 * 1024;

pub(crate) struct ProviderRoots {
    claude: Vec<PathBuf>,
    codex: Vec<PathBuf>,
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
    pub(crate) fn new() -> Self {
        let home = std::env::var_os("HOME").map_or_else(PathBuf::new, PathBuf::from);
        Self {
            claude: configured_roots(
                "CASS_CLAUDE_ROOTS",
                [
                    home.join(".claude/projects"),
                    home.join(".config/claude/projects"),
                ],
            ),
            codex: configured_roots(
                "CASS_CODEX_ROOTS",
                [
                    home.join(".codex/sessions"),
                    home.join(".local/share/codex/sessions"),
                ],
            ),
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

    for path in discover_jsonl_files(&roots.claude) {
        summary.scanned_files += 1;
        let parsed = parse_claude(&path)?;
        summary.malformed_records += parsed.malformed_records;
        if let Some(conversation) = parsed.conversation {
            storage.replace_conversation(&conversation)?;
        }
    }
    for path in discover_jsonl_files(&roots.codex) {
        summary.scanned_files += 1;
        let parsed = parse_codex(&path)?;
        summary.malformed_records += parsed.malformed_records;
        if let Some(conversation) = parsed.conversation {
            storage.replace_conversation(&conversation)?;
        }
    }

    Ok(summary)
}

fn configured_roots<const N: usize>(env_name: &str, defaults: [PathBuf; N]) -> Vec<PathBuf> {
    std::env::var_os(env_name).map_or_else(
        || defaults.into_iter().collect(),
        |value| std::env::split_paths(&value).collect(),
    )
}

fn discover_jsonl_files(roots: &[PathBuf]) -> Vec<PathBuf> {
    let mut files = BTreeSet::new();
    for root in roots {
        if root.is_file() {
            if is_jsonl(root) {
                files.insert(root.clone());
            }
            continue;
        }
        if !root.is_dir() {
            continue;
        }
        for entry in WalkDir::new(root).follow_links(false) {
            let Ok(entry) = entry else {
                continue;
            };
            let path = entry.path();
            if entry.file_type().is_file() && is_jsonl(path) {
                files.insert(path.to_path_buf());
            }
        }
    }
    files.into_iter().collect()
}

fn is_jsonl(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("jsonl"))
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
        let entry_type = raw.get("type").and_then(Value::as_str)?;
        match entry_type {
            "response_item" => extract_codex_response_item(raw),
            "event_msg" => extract_codex_event(raw),
            _ => None,
        }
    })
}

fn extract_codex_response_item(raw: &Value) -> Option<ExtractedMessage> {
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
}

fn extract_codex_event(raw: &Value) -> Option<ExtractedMessage> {
    let payload = raw.get("payload")?;
    let payload_type = payload.get("type").and_then(Value::as_str)?;
    let role = match payload_type {
        "user_message" => "user",
        "agent_message" | "assistant_message" => "assistant",
        _ => return None,
    };
    let content = payload
        .get("message")
        .or_else(|| payload.get("content"))
        .or_else(|| payload.get("text"))
        .and_then(flatten_content)?;
    Some((
        payload.get("id").and_then(Value::as_str).map(str::to_owned),
        role.to_owned(),
        content,
        parse_timestamp(raw.get("timestamp")),
        None,
    ))
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
        Some("thinking") => block
            .get("thinking")
            .and_then(Value::as_str)
            .map(|thinking| format!("[Thinking] {thinking}")),
        Some("toolCall") => {
            let name = block
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            let arguments = block
                .get("arguments")
                .map(stringify_value)
                .unwrap_or_default();
            Some(format!("Tool {name}: {arguments}"))
        }
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
    use veritas_test_macros as veritas;

    #[veritas::claims(
        "ingestion/provider-boundary",
        "ingestion/malformed-records-do-not-panic"
    )]
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

    #[veritas::claims("ingestion/provider-boundary")]
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

    #[veritas::claims("ingestion/provider-boundary", "ingestion/supported-jsonl-indexes")]
    #[test]
    fn codex_parser_keeps_custom_tool_calls() {
        let mut file = tempfile::NamedTempFile::new().expect("temporary JSONL");
        writeln!(file, r#"{{"type":"session_meta","payload":{{"id":"c2"}}}}"#).expect("meta line");
        writeln!(
            file,
            r#"{{"type":"response_item","payload":{{"type":"custom_tool_call","id":"tool1","call_id":"call1","name":"imagegen","input":"draw a diagram"}}}}"#
        )
        .expect("custom tool line");
        writeln!(
            file,
            r#"{{"type":"response_item","payload":{{"type":"custom_tool_call_output","id":"tool2","call_id":"call1","output":"created diagram.png"}}}}"#
        )
        .expect("custom tool output line");

        let parsed = parse_codex(file.path()).expect("parse Codex history");
        let conversation = parsed.conversation.expect("conversation");
        assert_eq!(conversation.id, "c2");
        assert_eq!(conversation.messages.len(), 2);
        assert_eq!(
            conversation.messages[0].content,
            "Tool imagegen: draw a diagram"
        );
        assert_eq!(conversation.messages[1].role, "tool");
        assert_eq!(conversation.messages[1].content, "created diagram.png");
    }
}
