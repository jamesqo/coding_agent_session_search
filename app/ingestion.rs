use std::collections::BTreeSet;
use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{RecvTimeoutError, sync_channel};
use std::time::{Duration, Instant, UNIX_EPOCH};

use chrono::DateTime;
use rusqlite::{Connection as SqliteConnection, OpenFlags};
use serde::Serialize;
use serde_json::Value;
use walkdir::WalkDir;

use crate::AppError;
use crate::storage::Storage;

const MAX_TOOL_OUTPUT_CHARS: usize = 128 * 1024;
const CHECKPOINT_FILES: u64 = 32;
const CHECKPOINT_BYTES: u64 = 128 * 1024 * 1024;

pub(crate) struct ProviderRoots {
    claude: Vec<PathBuf>,
    codex: Vec<PathBuf>,
    opencode: Vec<PathBuf>,
    copilot: Vec<PathBuf>,
    hermes: Vec<PathBuf>,
    pi: Vec<PathBuf>,
}

pub(crate) struct IndexSummary {
    pub(crate) scanned_files: u64,
    pub(crate) malformed_records: u64,
    pub(crate) changed_messages: u64,
    pub(crate) removed_messages: u64,
    pub(crate) unchanged_sources: u64,
    pub(crate) checkpoint_skipped_sources: u64,
    pub(crate) tombstoned_sources: u64,
    pub(crate) purged_conversations: u64,
    pub(crate) committed_batches: u64,
    pub(crate) discovered_bytes: u64,
    pub(crate) processed_bytes: u64,
    pub(crate) processed_files: u64,
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

struct Discovery {
    files: Vec<PathBuf>,
    complete: bool,
}

struct SourceFile {
    path: PathBuf,
    source_path: String,
    size_bytes: i64,
    modified_ns: i64,
}

struct ParsedSource {
    source: SourceFile,
    parsed: Result<ParsedFile, AppError>,
}

struct IndexProgress {
    started: Instant,
    last_event: Instant,
}

#[derive(Serialize)]
struct ProgressEvent<'a> {
    event: &'static str,
    phase: &'a str,
    processed_files: u64,
    scanned_files: u64,
    processed_bytes: u64,
    discovered_bytes: u64,
    changed_messages: u64,
    committed_batches: u64,
    elapsed_milliseconds: u64,
    files_per_second: u64,
    bytes_per_second: u64,
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
            opencode: configured_roots(
                "CASS_OPENCODE_ROOTS",
                [home.join(".local/share/opencode/opencode.db")],
            ),
            copilot: configured_roots("CASS_COPILOT_ROOTS", [home.join(".copilot/session-state")]),
            hermes: configured_roots("CASS_HERMES_ROOTS", [home.join(".hermes/state.db")]),
            pi: configured_roots("CASS_PI_ROOTS", [home.join(".pi/agent/sessions")]),
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
        changed_messages: 0,
        removed_messages: 0,
        unchanged_sources: 0,
        checkpoint_skipped_sources: 0,
        tombstoned_sources: 0,
        purged_conversations: 0,
        committed_batches: 0,
        discovered_bytes: 0,
        processed_bytes: 0,
        processed_files: 0,
    };
    let mut progress = IndexProgress::new();

    index_provider(
        storage,
        "claude-code",
        discover_jsonl_files(&roots.claude),
        parse_claude,
        &mut summary,
        &mut progress,
    )?;
    index_database_provider(
        storage,
        "opencode",
        &roots.opencode,
        parse_opencode,
        &mut summary,
    )?;
    index_provider(
        storage,
        "github-copilot",
        discover_files(&roots.copilot, |path| {
            path.file_name().and_then(|name| name.to_str()) == Some("events.jsonl")
        }),
        parse_copilot,
        &mut summary,
        &mut progress,
    )?;
    index_database_provider(storage, "hermes", &roots.hermes, parse_hermes, &mut summary)?;
    index_provider(
        storage,
        "pi",
        discover_jsonl_files(&roots.pi),
        parse_pi,
        &mut summary,
        &mut progress,
    )?;
    index_provider(
        storage,
        "codex",
        discover_jsonl_files(&roots.codex),
        parse_codex,
        &mut summary,
        &mut progress,
    )?;

    summary.processed_files = summary.scanned_files;
    progress.emit(&summary, "complete", true);

    Ok(summary)
}

fn index_provider(
    storage: &mut Storage,
    provider: &'static str,
    discovery: Discovery,
    parse: fn(&Path) -> Result<ParsedFile, AppError>,
    summary: &mut IndexSummary,
    progress: &mut IndexProgress,
) -> Result<(), AppError> {
    let (mut pending, observed_paths, mut complete) =
        prepare_sources(storage, provider, discovery, summary)?;
    pending.sort_unstable_by(|left, right| {
        right
            .size_bytes
            .cmp(&left.size_bytes)
            .then_with(|| left.source_path.cmp(&right.source_path))
    });
    progress.emit(summary, "discovered", true);

    let worker_count = parser_worker_count(pending.len());
    let next_source = AtomicUsize::new(0);
    let (sender, receiver) = sync_channel(1);
    let mut first_error = None;
    let mut batch_files = 0_u64;
    let mut batch_bytes = 0_u64;
    std::thread::scope(|scope| {
        for _ in 0..worker_count {
            let sender = sender.clone();
            let pending = &pending;
            let next_source = &next_source;
            scope.spawn(move || {
                loop {
                    let index = next_source.fetch_add(1, Ordering::Relaxed);
                    let Some(source) = pending.get(index) else {
                        break;
                    };
                    let parsed = parse(&source.path);
                    let source = SourceFile {
                        path: source.path.clone(),
                        source_path: source.source_path.clone(),
                        size_bytes: source.size_bytes,
                        modified_ns: source.modified_ns,
                    };
                    if sender.send(ParsedSource { source, parsed }).is_err() {
                        break;
                    }
                }
            });
        }
        drop(sender);

        loop {
            let result = match receiver.recv_timeout(Duration::from_secs(1)) {
                Ok(result) => result,
                Err(RecvTimeoutError::Timeout) => {
                    progress.emit(summary, "indexing", true);
                    continue;
                }
                Err(RecvTimeoutError::Disconnected) => break,
            };
            let source_bytes = u64::try_from(result.source.size_bytes).unwrap_or_default();
            summary.processed_files += 1;
            summary.processed_bytes = summary.processed_bytes.saturating_add(source_bytes);
            match apply_parsed_source(storage, provider, result, summary) {
                Ok(source_complete) => complete &= source_complete,
                Err(error) => {
                    complete = false;
                    if first_error.is_none() {
                        first_error = Some(error);
                    }
                    continue;
                }
            }
            batch_files = batch_files.saturating_add(1);
            batch_bytes = batch_bytes.saturating_add(source_bytes);
            if storage.supports_ingestion_checkpoints()
                && (batch_files >= CHECKPOINT_FILES || batch_bytes >= CHECKPOINT_BYTES)
            {
                match storage.checkpoint_writer() {
                    Ok(()) => {
                        summary.committed_batches += 1;
                        batch_files = 0;
                        batch_bytes = 0;
                        progress.emit(summary, "checkpoint", true);
                    }
                    Err(error) => {
                        complete = false;
                        if first_error.is_none() {
                            first_error = Some(error);
                        }
                    }
                }
            }
            progress.emit(summary, "indexing", false);
        }
    });
    if complete {
        summary.purged_conversations += storage.purge_missing_sources(provider, &observed_paths)?;
    }
    first_error.map_or(Ok(()), Err)
}

fn prepare_sources(
    storage: &Storage,
    provider: &str,
    discovery: Discovery,
    summary: &mut IndexSummary,
) -> Result<(Vec<SourceFile>, BTreeSet<String>, bool), AppError> {
    let complete = discovery.complete;
    let mut pending = Vec::new();
    let mut observed_paths = BTreeSet::new();
    for path in discovery.files {
        summary.scanned_files += 1;
        let source_path = path.to_string_lossy().into_owned();
        observed_paths.insert(source_path.clone());
        let (size_bytes, modified_ns) = source_stamp(&path)?;
        let bytes = u64::try_from(size_bytes).unwrap_or_default();
        summary.discovered_bytes = summary.discovered_bytes.saturating_add(bytes);
        if storage.source_checkpoint_matches(provider, &source_path, size_bytes, modified_ns)? {
            summary.unchanged_sources += 1;
            summary.checkpoint_skipped_sources += 1;
            summary.processed_files += 1;
            summary.processed_bytes = summary.processed_bytes.saturating_add(bytes);
        } else {
            pending.push(SourceFile {
                path,
                source_path,
                size_bytes,
                modified_ns,
            });
        }
    }
    Ok((pending, observed_paths, complete))
}

fn apply_parsed_source(
    storage: &mut Storage,
    provider: &str,
    result: ParsedSource,
    summary: &mut IndexSummary,
) -> Result<bool, AppError> {
    let parsed = result.parsed?;
    summary.malformed_records += parsed.malformed_records;
    if let Some(conversation) = parsed.conversation {
        apply_conversation(storage, &conversation, summary)?;
    } else if parsed.malformed_records == 0
        && storage.remove_source(provider, &result.source.source_path)?
    {
        summary.purged_conversations += 1;
    }
    if parsed.malformed_records == 0 {
        storage.record_source_checkpoint(
            provider,
            &result.source.source_path,
            result.source.size_bytes,
            result.source.modified_ns,
        )?;
    }
    Ok(parsed.malformed_records == 0)
}

impl IndexProgress {
    fn new() -> Self {
        let now = Instant::now();
        Self {
            started: now,
            last_event: now,
        }
    }

    fn emit(&mut self, summary: &IndexSummary, phase: &str, force: bool) {
        let now = Instant::now();
        if !force && now.duration_since(self.last_event) < Duration::from_secs(1) {
            return;
        }
        self.last_event = now;
        let elapsed_milliseconds =
            u64::try_from(now.duration_since(self.started).as_millis().max(1)).unwrap_or(u64::MAX);
        let event = ProgressEvent {
            event: "index-progress",
            phase,
            processed_files: summary.processed_files,
            scanned_files: summary.scanned_files,
            processed_bytes: summary.processed_bytes,
            discovered_bytes: summary.discovered_bytes,
            changed_messages: summary.changed_messages,
            committed_batches: summary.committed_batches,
            elapsed_milliseconds,
            files_per_second: summary
                .processed_files
                .saturating_mul(1_000)
                .checked_div(elapsed_milliseconds)
                .unwrap_or_default(),
            bytes_per_second: summary
                .processed_bytes
                .saturating_mul(1_000)
                .checked_div(elapsed_milliseconds)
                .unwrap_or_default(),
        };
        let stderr = std::io::stderr();
        let mut stderr = stderr.lock();
        if serde_json::to_writer(&mut stderr, &event).is_ok() {
            let _ = writeln!(stderr);
        }
    }
}

fn parser_worker_count(source_count: usize) -> usize {
    let available = std::thread::available_parallelism().map_or(1, usize::from);
    source_count.min(available.saturating_sub(1).max(1)).min(8)
}

fn source_stamp(path: &Path) -> Result<(i64, i64), AppError> {
    let metadata = path.metadata().map_err(AppError::io)?;
    let size_bytes = i64::try_from(metadata.len())
        .map_err(|_| AppError::internal("source file is too large"))?;
    let modified_ns = metadata
        .modified()
        .map_err(AppError::io)?
        .duration_since(UNIX_EPOCH)
        .map_err(|_| AppError::internal("source modification time predates Unix epoch"))?
        .as_nanos();
    let modified_ns = i64::try_from(modified_ns)
        .map_err(|_| AppError::internal("source modification time is out of range"))?;
    Ok((size_bytes, modified_ns))
}

fn index_database_provider(
    storage: &mut Storage,
    provider: &'static str,
    roots: &[PathBuf],
    parse: fn(&Path) -> Result<ParsedDatabase, AppError>,
    summary: &mut IndexSummary,
) -> Result<(), AppError> {
    let mut complete = true;
    let mut observed_paths = BTreeSet::new();
    for path in roots.iter().filter(|path| path.is_file()) {
        summary.scanned_files += 1;
        let parsed = parse(path)?;
        summary.malformed_records += parsed.malformed_records;
        complete &= parsed.malformed_records == 0;
        for conversation in parsed.conversations {
            observed_paths.insert(conversation.source_path.to_string_lossy().into_owned());
            apply_conversation(storage, &conversation, summary)?;
        }
    }
    if complete {
        summary.purged_conversations += storage.purge_missing_sources(provider, &observed_paths)?;
    }
    Ok(())
}

fn apply_conversation(
    storage: &mut Storage,
    conversation: &Conversation,
    summary: &mut IndexSummary,
) -> Result<(), AppError> {
    let change = storage.replace_conversation(conversation)?;
    summary.changed_messages += u64::try_from(change.changed_message_ids.len())
        .map_err(|_| AppError::internal("too many changed messages"))?;
    summary.removed_messages += change.removed_messages;
    summary.unchanged_sources += u64::from(change.unchanged);
    summary.tombstoned_sources += u64::from(change.tombstoned);
    Ok(())
}

fn configured_roots<const N: usize>(env_name: &str, defaults: [PathBuf; N]) -> Vec<PathBuf> {
    std::env::var_os(env_name).map_or_else(
        || defaults.into_iter().collect(),
        |value| std::env::split_paths(&value).collect(),
    )
}

fn discover_jsonl_files(roots: &[PathBuf]) -> Discovery {
    discover_files(roots, is_jsonl)
}

fn discover_files(roots: &[PathBuf], accept: fn(&Path) -> bool) -> Discovery {
    let mut files = BTreeSet::new();
    let mut complete = true;
    for root in roots {
        if root.is_file() {
            if accept(root) {
                files.insert(root.clone());
            }
            continue;
        }
        if !root.is_dir() {
            continue;
        }
        for entry in WalkDir::new(root).follow_links(false) {
            let Ok(entry) = entry else {
                complete = false;
                continue;
            };
            let path = entry.path();
            if entry.file_type().is_file() && accept(path) {
                files.insert(path.to_path_buf());
            }
        }
    }
    Discovery {
        files: files.into_iter().collect(),
        complete,
    }
}

fn is_jsonl(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("jsonl"))
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
        let mut query = connection
            .prepare(
                "SELECT id, data, time_created FROM message
                 WHERE session_id = ?1 ORDER BY time_created, id",
            )
            .map_err(AppError::database)?;
        let rows = query
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
        drop(query);
        let mut messages = Vec::new();
        for (source_id, data, message_created_at) in rows {
            let role = if let Ok(data) = serde_json::from_str::<Value>(&data) {
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
        if !messages.is_empty() {
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
            if matches!(
                data.get("type").and_then(Value::as_str),
                Some("text" | "reasoning")
            ) && let Some(text) = data.get("text").and_then(Value::as_str)
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

fn parse_hermes(path: &Path) -> Result<ParsedDatabase, AppError> {
    let connection = SqliteConnection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(AppError::database)?;
    let mut query = connection
        .prepare(
            "SELECT id, title, CAST(started_at * 1000 AS INTEGER),
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
        let mut query = connection
            .prepare(
                "SELECT id, role, content, reasoning,
                        CAST(timestamp * 1000 AS INTEGER)
                 FROM messages WHERE session_id = ?1 ORDER BY timestamp, id",
            )
            .map_err(AppError::database)?;
        let rows = query
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
        drop(query);
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
        if !messages.is_empty() {
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
    }
    Ok(ParsedDatabase {
        conversations,
        malformed_records: 0,
    })
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
        if entry_type == "response_item" {
            extract_codex_response_item(raw)
        } else {
            None
        }
    })
}

fn parse_pi(path: &Path) -> Result<ParsedFile, AppError> {
    parse_jsonl(path, "pi", |raw| {
        if raw.get("type").and_then(Value::as_str) != Some("message") {
            return None;
        }
        let message = raw.get("message")?;
        let role = match message.get("role").and_then(Value::as_str)? {
            "toolResult" => "tool",
            role => role,
        };
        let mut content = flatten_content(message.get("content")?)?;
        if role == "tool" {
            content = truncate_chars(&content, MAX_TOOL_OUTPUT_CHARS);
        }
        Some((
            raw.get("id").and_then(Value::as_str).map(str::to_owned),
            role.to_owned(),
            content,
            parse_timestamp(raw.get("timestamp")),
            None,
        ))
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
        if provider == "codex"
            && session_id.is_none()
            && raw.get("type").and_then(Value::as_str) == Some("session_meta")
        {
            session_id = raw
                .get("payload")
                .and_then(|payload| payload.get("id").or_else(|| payload.get("session_id")))
                .and_then(Value::as_str)
                .map(str::to_owned);
        } else if provider == "pi" && raw.get("type").and_then(Value::as_str) == Some("session") {
            session_id = raw.get("id").and_then(Value::as_str).map(str::to_owned);
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
    use std::fs;
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

    #[veritas::claims("ingestion/provider-boundary")]
    #[test]
    fn codex_parser_keeps_the_rollout_session_identity() {
        let mut file = tempfile::NamedTempFile::new().expect("temporary JSONL");
        writeln!(
            file,
            r#"{{"type":"session_meta","payload":{{"id":"rollout-session"}}}}"#
        )
        .expect("rollout metadata");
        writeln!(file, r#"{{"type":"response_item","payload":{{"type":"message","id":"m1","role":"user","content":[{{"type":"input_text","text":"original turn"}}]}}}}"#)
            .expect("message line");
        writeln!(
            file,
            r#"{{"type":"session_meta","payload":{{"id":"nested-session"}}}}"#
        )
        .expect("nested metadata");

        let parsed = parse_codex(file.path()).expect("parse Codex history");
        let conversation = parsed.conversation.expect("conversation");
        assert_eq!(conversation.id, "rollout-session");
    }

    #[veritas::claims("ingestion/provider-boundary", "ingestion/supported-jsonl-indexes")]
    #[test]
    fn codex_parser_ignores_mirrored_event_messages() {
        let mut file = tempfile::NamedTempFile::new().expect("temporary JSONL");
        writeln!(
            file,
            r#"{{"type":"session_meta","payload":{{"id":"c-mirror"}}}}"#
        )
        .expect("meta line");
        writeln!(file, r#"{{"type":"response_item","payload":{{"type":"message","id":"m1","role":"user","content":[{{"type":"input_text","text":"mirrored turn"}}]}}}}"#)
            .expect("response item");
        writeln!(file, r#"{{"type":"event_msg","payload":{{"type":"user_message","message":"mirrored turn"}}}}"#)
            .expect("mirrored event");

        let parsed = parse_codex(file.path()).expect("parse Codex history");
        let conversation = parsed.conversation.expect("conversation");
        assert_eq!(conversation.messages.len(), 1);
        assert_eq!(conversation.messages[0].content, "mirrored turn");
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

    #[veritas::claims(
        "indexing/incomplete-scan-preserves-state",
        "indexing/complete-scan-purges-missing-source"
    )]
    #[test]
    fn purge_requires_a_complete_provider_scan() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let database = directory.path().join("cass.sqlite3");
        let claude_root = directory.path().join("claude");
        fs::create_dir_all(&claude_root).expect("Claude root");
        let missing_source = claude_root.join("missing.jsonl");

        let mut writer = Storage::open_writer(&database).expect("writer");
        writer
            .replace_conversation(&Conversation {
                id: "old-session".to_owned(),
                provider: "claude-code",
                source_path: missing_source,
                title: None,
                created_at: None,
                updated_at: None,
                messages: vec![NormalizedMessage {
                    id: "old-message".to_owned(),
                    ordinal: 0,
                    role: "user".to_owned(),
                    content: "preserve until complete".to_owned(),
                    created_at: None,
                }],
            })
            .expect("seed source");
        writer.commit_writer().expect("commit seed");
        drop(writer);

        fs::write(claude_root.join("malformed.jsonl"), "not-json\n").expect("malformed source");
        let roots = ProviderRoots {
            claude: vec![claude_root.clone()],
            codex: Vec::new(),
            opencode: Vec::new(),
            copilot: Vec::new(),
            hermes: Vec::new(),
            pi: Vec::new(),
        };
        let mut writer = Storage::open_writer(&database).expect("incomplete writer");
        let incomplete = index(&mut writer, &roots).expect("bounded malformed scan");
        assert_eq!(incomplete.malformed_records, 1);
        writer.commit_writer().expect("commit incomplete scan");
        assert_eq!(writer.counts().expect("counts").conversations, 1);
        drop(writer);

        fs::remove_file(claude_root.join("malformed.jsonl")).expect("remove malformed source");
        let mut writer = Storage::open_writer(&database).expect("complete writer");
        let complete = index(&mut writer, &roots).expect("complete scan");
        assert_eq!(complete.purged_conversations, 1);
        writer.commit_writer().expect("commit complete scan");
        assert_eq!(writer.counts().expect("counts").conversations, 0);
    }
}
