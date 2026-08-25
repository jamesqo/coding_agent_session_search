use std::collections::BTreeSet;
use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{RecvTimeoutError, sync_channel};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use chrono::DateTime;
use serde::Serialize;
use serde_json::Value;
use walkdir::WalkDir;

use crate::AppError;
use crate::storage::Storage;

const MAX_TOOL_OUTPUT_CHARS: usize = 128 * 1024;
const CHECKPOINT_FILES: u64 = 32;
const CHECKPOINT_BYTES: u64 = 128 * 1024 * 1024;

pub(crate) struct IndexOptions {
    pub(crate) claude_code: Option<Vec<PathBuf>>,
    pub(crate) codex: Option<Vec<PathBuf>>,
    pub(crate) roots_are_authoritative: bool,
    pub(crate) since_days: Option<u32>,
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
    /// `None` searches canonical content, an empty string makes the message
    /// context-only, and a nonempty value searches that filtered projection.
    pub(crate) search_projection: Option<String>,
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

#[derive(Clone, Copy)]
struct ProviderScan<'a> {
    provider: &'static str,
    roots: &'a [PathBuf],
    cutoff_modified_ns: Option<i64>,
    parse: fn(&Path) -> Result<ParsedFile, AppError>,
    authoritative: bool,
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
    messages_per_second: u64,
    bytes_per_second: u64,
}

struct ExtractedMessage {
    source_id: Option<String>,
    role: String,
    content: String,
    search_projection: Option<String>,
    created_at: Option<i64>,
    session_id: Option<String>,
}

pub(crate) fn index(
    storage: &mut Storage,
    options: &IndexOptions,
) -> Result<IndexSummary, AppError> {
    index_at(storage, options, SystemTime::now())
}

fn index_at(
    storage: &mut Storage,
    options: &IndexOptions,
    run_started: SystemTime,
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
    let cutoff_modified_ns = cutoff_modified_ns(run_started, options.since_days)?;

    // Inspect every configured root before opening the ingestion pipeline. A
    // bad root must fail closed without committing another provider first.
    let claude_discovery = options
        .claude_code
        .as_ref()
        .map(|roots| discover_jsonl_files(roots, options.roots_are_authoritative))
        .transpose()?;
    let codex_discovery = options
        .codex
        .as_ref()
        .map(|roots| discover_jsonl_files(roots, options.roots_are_authoritative))
        .transpose()?;

    if let (Some(roots), Some(discovery)) = (&options.claude_code, claude_discovery) {
        index_provider(
            storage,
            ProviderScan {
                provider: "claude-code",
                roots,
                cutoff_modified_ns,
                parse: parse_claude,
                authoritative: options.roots_are_authoritative,
            },
            discovery,
            &mut summary,
            &mut progress,
        )?;
    }
    if let (Some(roots), Some(discovery)) = (&options.codex, codex_discovery) {
        index_provider(
            storage,
            ProviderScan {
                provider: "codex",
                roots,
                cutoff_modified_ns,
                parse: parse_codex,
                authoritative: options.roots_are_authoritative,
            },
            discovery,
            &mut summary,
            &mut progress,
        )?;
    }

    summary.processed_files = summary.scanned_files;
    progress.emit(&summary, "ingestion-complete", true);

    Ok(summary)
}

fn index_provider(
    storage: &mut Storage,
    scan: ProviderScan<'_>,
    discovery: Discovery,
    summary: &mut IndexSummary,
    progress: &mut IndexProgress,
) -> Result<(), AppError> {
    if !scan.authoritative {
        return index_provider_inner(storage, scan, discovery, summary, progress);
    }
    storage.begin_provider_scan()?;
    match index_provider_inner(storage, scan, discovery, summary, progress) {
        Ok(()) => storage.finish_provider_scan(),
        Err(error) => match storage.rollback_provider_scan() {
            Ok(()) => Err(error),
            Err(rollback_error) => Err(rollback_error),
        },
    }
}

#[allow(
    clippy::too_many_lines,
    reason = "keep the bounded parser producer/consumer transaction in one linear scope"
)]
fn index_provider_inner(
    storage: &mut Storage,
    scan: ProviderScan<'_>,
    discovery: Discovery,
    summary: &mut IndexSummary,
    progress: &mut IndexProgress,
) -> Result<(), AppError> {
    let ProviderScan {
        provider,
        roots,
        cutoff_modified_ns,
        parse,
        authoritative,
    } = scan;
    let (mut pending, observed_paths, mut complete) = prepare_sources(
        storage,
        provider,
        cutoff_modified_ns,
        authoritative,
        discovery,
        summary,
    )?;
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
                    let parsed = parse(&source.path).map_err(|error| {
                        if authoritative {
                            configured_source_error(&source.path, &error)
                        } else {
                            error
                        }
                    });
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
                    record_first_error(&mut complete, &mut first_error, error);
                    continue;
                }
            }
            batch_files = batch_files.saturating_add(1);
            batch_bytes = batch_bytes.saturating_add(source_bytes);
            if !authoritative
                && (batch_files >= CHECKPOINT_FILES || batch_bytes >= CHECKPOINT_BYTES)
                && let Err(error) = checkpoint_provider_batch(
                    storage,
                    summary,
                    progress,
                    &mut batch_files,
                    &mut batch_bytes,
                )
            {
                record_first_error(&mut complete, &mut first_error, error);
            }
            progress.emit(summary, "indexing", false);
        }
    });
    if authoritative {
        verify_authoritative_discovery(roots, &observed_paths)?;
    }
    summary.purged_conversations += reconcile_complete_sources(
        storage,
        complete,
        provider,
        &observed_paths,
        roots,
        cutoff_modified_ns,
    )?;
    first_error.map_or(Ok(()), Err)
}

fn reconcile_complete_sources(
    storage: &mut Storage,
    complete: bool,
    provider: &str,
    observed_paths: &BTreeSet<String>,
    roots: &[PathBuf],
    cutoff_modified_ns: Option<i64>,
) -> Result<u64, AppError> {
    if !complete {
        return Ok(0);
    }
    storage.purge_missing_sources(provider, observed_paths, roots, cutoff_modified_ns)
}

fn verify_authoritative_discovery(
    roots: &[PathBuf],
    observed_paths: &BTreeSet<String>,
) -> Result<(), AppError> {
    let final_paths = discover_jsonl_files(roots, true)?
        .files
        .into_iter()
        .map(|path| path.to_string_lossy().into_owned())
        .collect::<BTreeSet<_>>();
    if final_paths != *observed_paths {
        return Err(AppError::configuration(
            "configured provider roots changed while indexing",
        ));
    }
    Ok(())
}

fn record_first_error(complete: &mut bool, first_error: &mut Option<AppError>, error: AppError) {
    *complete = false;
    if first_error.is_none() {
        *first_error = Some(error);
    }
}

fn checkpoint_provider_batch(
    storage: &mut Storage,
    summary: &mut IndexSummary,
    progress: &mut IndexProgress,
    batch_files: &mut u64,
    batch_bytes: &mut u64,
) -> Result<(), AppError> {
    storage.checkpoint_writer()?;
    summary.committed_batches += 1;
    *batch_files = 0;
    *batch_bytes = 0;
    progress.emit(summary, "checkpoint", true);
    Ok(())
}

fn prepare_sources(
    storage: &Storage,
    provider: &str,
    cutoff_modified_ns: Option<i64>,
    authoritative: bool,
    discovery: Discovery,
    summary: &mut IndexSummary,
) -> Result<(Vec<SourceFile>, BTreeSet<String>, bool), AppError> {
    let complete = discovery.complete;
    let mut pending = Vec::new();
    let mut observed_paths = BTreeSet::new();
    for path in discovery.files {
        let source_path = path.to_string_lossy().into_owned();
        observed_paths.insert(source_path.clone());
        let (size_bytes, modified_ns) = source_stamp(&path).map_err(|error| {
            if authoritative {
                configured_source_error(&path, &error)
            } else {
                error
            }
        })?;
        if cutoff_modified_ns.is_some_and(|cutoff| modified_ns < cutoff) {
            continue;
        }
        summary.scanned_files += 1;
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

fn configured_source_error(path: &Path, error: &AppError) -> AppError {
    AppError::configuration(format!(
        "configured provider source became inaccessible: {}: {}",
        path.display(),
        error.error.message
    ))
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
            messages_per_second: summary
                .changed_messages
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

fn discover_jsonl_files(roots: &[PathBuf], authoritative: bool) -> Result<Discovery, AppError> {
    discover_files(roots, authoritative, is_jsonl)
}

fn discover_files(
    roots: &[PathBuf],
    authoritative: bool,
    accept: fn(&Path) -> bool,
) -> Result<Discovery, AppError> {
    let mut files = BTreeSet::new();
    let mut complete = true;
    for root in roots {
        let metadata = match root.metadata() {
            Ok(metadata) => metadata,
            Err(error) if authoritative => {
                return Err(AppError::configuration(format!(
                    "configured provider root is not accessible: {}: {error}",
                    root.display()
                )));
            }
            Err(_) => {
                complete = false;
                continue;
            }
        };
        if metadata.is_file() {
            if accept(root) {
                files.insert(root.clone());
            } else if authoritative {
                return Err(AppError::configuration(format!(
                    "configured provider file is not a supported history: {}",
                    root.display()
                )));
            } else {
                complete = false;
            }
            continue;
        }
        if !metadata.is_dir() {
            if authoritative {
                return Err(AppError::configuration(format!(
                    "configured provider root is not a file or directory: {}",
                    root.display()
                )));
            }
            complete = false;
            continue;
        }
        for entry in WalkDir::new(root).follow_links(false) {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) if authoritative => {
                    return Err(AppError::configuration(format!(
                        "failed to inspect configured provider root {}: {error}",
                        root.display()
                    )));
                }
                Err(_) => {
                    complete = false;
                    continue;
                }
            };
            let path = entry.path();
            if entry.file_type().is_file() && accept(path) {
                files.insert(path.to_path_buf());
            }
        }
    }
    Ok(Discovery {
        files: files.into_iter().collect(),
        complete,
    })
}

fn cutoff_modified_ns(
    run_started: SystemTime,
    since_days: Option<u32>,
) -> Result<Option<i64>, AppError> {
    let Some(days) = since_days else {
        return Ok(None);
    };
    let seconds = u64::from(days)
        .checked_mul(24 * 60 * 60)
        .ok_or_else(|| AppError::usage("--since-days is out of range"))?;
    let cutoff = run_started
        .checked_sub(Duration::from_secs(seconds))
        .unwrap_or(UNIX_EPOCH);
    let nanoseconds = cutoff
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let nanoseconds = i64::try_from(nanoseconds)
        .map_err(|_| AppError::internal("index cutoff is out of range"))?;
    Ok(Some(nanoseconds))
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
        let (content, search_projection) = claude_content(message.get("content")?)?;
        Some(ExtractedMessage {
            source_id: raw.get("uuid").and_then(Value::as_str).map(str::to_owned),
            role: role.to_owned(),
            content,
            search_projection,
            created_at: parse_timestamp(raw.get("timestamp")),
            session_id: raw
                .get("sessionId")
                .and_then(Value::as_str)
                .map(str::to_owned),
        })
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

fn extract_codex_response_item(raw: &Value) -> Option<ExtractedMessage> {
    let payload = raw.get("payload")?;
    let payload_type = payload.get("type").and_then(Value::as_str)?;
    let (role, content, search_projection) = match payload_type {
        "message" => (
            payload
                .get("role")
                .and_then(Value::as_str)
                .unwrap_or("assistant")
                .to_owned(),
            flatten_content(payload.get("content")?)?,
            None,
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
            (
                "assistant".to_owned(),
                format!("Tool {name}: {arguments}"),
                None,
            )
        }
        "function_call_output" | "custom_tool_call_output" => {
            let output = payload.get("output").map(stringify_value)?;
            let content = truncate_chars(&output, MAX_TOOL_OUTPUT_CHARS);
            ("tool".to_owned(), content, Some(String::new()))
        }
        _ => return None,
    };
    Some(ExtractedMessage {
        source_id: payload
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| {
                payload
                    .get("call_id")
                    .and_then(Value::as_str)
                    .map(|call_id| format!("{payload_type}:{call_id}"))
            }),
        role,
        content,
        search_projection,
        created_at: parse_timestamp(raw.get("timestamp")),
        session_id: None,
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
        if provider == "codex"
            && session_id.is_none()
            && raw.get("type").and_then(Value::as_str) == Some("session_meta")
        {
            session_id = raw
                .get("payload")
                .and_then(|payload| payload.get("id").or_else(|| payload.get("session_id")))
                .and_then(Value::as_str)
                .map(str::to_owned);
        }
        let Some(extracted) = extract(&raw) else {
            continue;
        };
        if extracted.content.trim().is_empty() {
            continue;
        }
        if session_id.is_none() {
            session_id = extracted.session_id;
        }
        let ordinal = i64::try_from(messages.len())
            .map_err(|_| AppError::internal("conversation contains too many messages"))?;
        let message_key = extracted.source_id.unwrap_or_else(|| ordinal.to_string());
        messages.push(NormalizedMessage {
            id: stable_id(
                "message",
                &[provider, &path.to_string_lossy(), &message_key],
            ),
            ordinal,
            role: extracted.role,
            content: extracted.content,
            search_projection: extracted.search_projection,
            created_at: extracted.created_at,
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

fn claude_content(value: &Value) -> Option<(String, Option<String>)> {
    let content = flatten_content(value)?;
    if is_tool_result(value) {
        return Some((content, Some(String::new())));
    }
    let Value::Array(blocks) = value else {
        return Some((content, None));
    };
    let contains_tool_result = blocks.iter().any(is_tool_result);
    if !contains_tool_result {
        return Some((content, None));
    }
    let projection = blocks
        .iter()
        .filter(|block| !is_tool_result(block))
        .filter_map(flatten_block)
        .collect::<Vec<_>>()
        .join("\n");
    Some((content, Some(projection)))
}

fn is_tool_result(block: &Value) -> bool {
    block.get("type").and_then(Value::as_str) == Some("tool_result")
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
    use std::fs::{self, FileTimes};
    use std::io::Write;

    use super::*;
    use veritas_test_macros as veritas;

    fn write_claude_session(path: &Path, session: &str, message: &str) {
        fs::write(
            path,
            format!(
                "{{\"type\":\"user\",\"sessionId\":\"{session}\",\"uuid\":\"{session}-m1\",\"message\":{{\"role\":\"user\",\"content\":\"{message}\"}}}}\n"
            ),
        )
        .expect("Claude fixture");
    }

    fn test_conversation(
        id: &str,
        provider: &'static str,
        source_path: PathBuf,
        content: &str,
    ) -> Conversation {
        Conversation {
            id: id.to_owned(),
            provider,
            source_path,
            title: None,
            created_at: None,
            updated_at: None,
            messages: vec![NormalizedMessage {
                id: format!("{id}-message"),
                ordinal: 0,
                role: "user".to_owned(),
                content: content.to_owned(),
                search_projection: None,
                created_at: None,
            }],
        }
    }

    fn empty_summary() -> IndexSummary {
        IndexSummary {
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
        }
    }

    fn set_modified(path: &Path, modified: SystemTime) {
        fs::File::options()
            .write(true)
            .open(path)
            .expect("open fixture")
            .set_times(FileTimes::new().set_modified(modified))
            .expect("set fixture modification time");
    }

    fn parse_with_late_failure(path: &Path) -> Result<ParsedFile, AppError> {
        if path.file_name().and_then(|name| name.to_str()) == Some("failure.jsonl") {
            std::thread::sleep(Duration::from_millis(50));
            return Err(AppError::io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "source disappeared",
            )));
        }
        parse_claude(path)
    }

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

    #[veritas::claims(
        "search/tool-results-are-not-searchable",
        "search/mixed-message-excludes-tool-result-text",
        "view/tool-results-remain-visible"
    )]
    #[test]
    fn claude_parser_separates_tool_results_from_searchable_text() {
        let mut file = tempfile::NamedTempFile::new().expect("temporary JSONL");
        writeln!(file, r#"{{"type":"user","sessionId":"s1","uuid":"mixed","message":{{"role":"user","content":[{{"type":"text","text":"keep this request"}},{{"type":"tool_result","content":"private tool payload"}}]}}}}"#).expect("mixed line");
        writeln!(file, r#"{{"type":"user","sessionId":"s1","uuid":"tool-only","message":{{"role":"user","content":[{{"type":"tool_result","content":"only tool payload"}}]}}}}"#).expect("tool-result line");

        let parsed = parse_claude(file.path()).expect("parse Claude history");
        let messages = parsed.conversation.expect("conversation").messages;
        assert_eq!(
            messages[0].content,
            "keep this request\nprivate tool payload"
        );
        assert_eq!(
            messages[0].search_projection.as_deref(),
            Some("keep this request")
        );
        assert_eq!(messages[1].content, "only tool payload");
        assert_eq!(messages[1].search_projection.as_deref(), Some(""));

        let mut object_file = tempfile::NamedTempFile::new().expect("temporary JSONL");
        writeln!(object_file, r#"{{"type":"user","sessionId":"s2","uuid":"object","message":{{"role":"user","content":{{"type":"tool_result","content":"object tool payload"}}}}}}"#).expect("object tool-result line");
        let object = parse_claude(object_file.path())
            .expect("parse object tool result")
            .conversation
            .expect("object conversation")
            .messages
            .remove(0);
        assert_eq!(object.content, "object tool payload");
        assert_eq!(object.search_projection.as_deref(), Some(""));
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
        assert_eq!(
            conversation.messages[1].search_projection.as_deref(),
            Some("")
        );
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
                    search_projection: None,
                    created_at: None,
                }],
            })
            .expect("seed source");
        writer.commit_writer().expect("commit seed");
        drop(writer);

        fs::write(claude_root.join("malformed.jsonl"), "not-json\n").expect("malformed source");
        let options = IndexOptions {
            claude_code: Some(vec![claude_root.clone()]),
            codex: None,
            roots_are_authoritative: true,
            since_days: None,
        };
        let mut writer = Storage::open_writer(&database).expect("incomplete writer");
        let incomplete = index(&mut writer, &options).expect("bounded malformed scan");
        assert_eq!(incomplete.malformed_records, 1);
        writer.commit_writer().expect("commit incomplete scan");
        assert_eq!(writer.counts().expect("counts").conversations, 1);
        drop(writer);

        fs::remove_file(claude_root.join("malformed.jsonl")).expect("remove malformed source");
        let mut writer = Storage::open_writer(&database).expect("complete writer");
        let complete = index(&mut writer, &options).expect("complete scan");
        assert_eq!(complete.purged_conversations, 1);
        writer.commit_writer().expect("commit complete scan");
        assert_eq!(writer.counts().expect("counts").conversations, 0);
    }

    #[veritas::claims(
        "ingestion/configured-provider-roots-index",
        "ingestion/unsupported-providers-ignored"
    )]
    #[test]
    fn authoritative_roots_index_only_the_two_concrete_formats() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let claude = directory.path().join("claude");
        let codex = directory.path().join("codex");
        fs::create_dir(&claude).expect("Claude root");
        fs::create_dir(&codex).expect("Codex root");
        write_claude_session(
            &claude.join("claude.jsonl"),
            "claude-session",
            "claude sentinel",
        );
        fs::write(
            claude.join("unsupported.jsonl"),
            "{\"info\":{\"id\":\"opencode-session\"},\"messages\":[{\"role\":\"user\",\"content\":\"unsupported sentinel\"}]}\n",
        )
        .expect("unsupported fixture");
        fs::write(
            codex.join("codex.jsonl"),
            concat!(
                "{\"type\":\"session_meta\",\"payload\":{\"id\":\"codex-session\"}}\n",
                "{\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"id\":\"codex-message\",\"role\":\"user\",\"content\":[{\"type\":\"input_text\",\"text\":\"codex sentinel\"}]}}\n"
            ),
        )
        .expect("Codex fixture");
        let mut writer =
            Storage::open_writer(&directory.path().join("cass.sqlite3")).expect("writer");
        let summary = index(
            &mut writer,
            &IndexOptions {
                claude_code: Some(vec![claude]),
                codex: Some(vec![codex]),
                roots_are_authoritative: true,
                since_days: None,
            },
        )
        .expect("configured index");

        assert_eq!(summary.scanned_files, 3);
        assert_eq!(writer.counts().expect("counts").conversations, 2);
        writer.commit_writer().expect("commit configured index");
        assert_eq!(
            writer
                .search("claude", 10, Some("claude-code"), None)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            writer
                .search("codex", 10, Some("codex"), None)
                .unwrap()
                .len(),
            1
        );
        assert!(
            writer
                .search("unsupported", 10, None, None)
                .unwrap()
                .is_empty()
        );
    }

    #[veritas::claims("indexing/inaccessible-roots-never-authorize-purge")]
    #[test]
    fn every_authoritative_root_is_preflighted_before_ingestion_writes() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let claude_root = directory.path().join("claude");
        fs::create_dir(&claude_root).expect("Claude root");
        write_claude_session(&claude_root.join("session.jsonl"), "session", "sentinel");
        let options = IndexOptions {
            claude_code: Some(vec![claude_root]),
            codex: Some(vec![directory.path().join("missing-codex")]),
            roots_are_authoritative: true,
            since_days: None,
        };
        let mut writer =
            Storage::open_writer(&directory.path().join("cass.sqlite3")).expect("writer");

        let error = index(&mut writer, &options).err().expect("missing root");

        assert_eq!(error.error.kind, "configuration");
        assert_eq!(writer.counts().expect("counts").conversations, 0);
        assert_eq!(writer.counts().expect("counts").messages, 0);
    }

    #[veritas::claims("indexing/incomplete-scan-preserves-state")]
    #[test]
    fn missing_builtin_root_is_nonfatal_but_never_authorizes_purge() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let root = directory.path().join("claude");
        fs::create_dir(&root).expect("Claude root");
        write_claude_session(&root.join("session.jsonl"), "session", "durable sentinel");
        let database = directory.path().join("cass.sqlite3");
        let options = IndexOptions {
            claude_code: Some(vec![root.clone()]),
            codex: None,
            roots_are_authoritative: false,
            since_days: None,
        };
        let mut writer = Storage::open_writer(&database).expect("initial writer");
        index(&mut writer, &options).expect("initial index");
        writer.commit_writer().expect("commit initial index");
        drop(writer);
        fs::rename(&root, directory.path().join("offline")).expect("take root offline");

        let mut writer = Storage::open_writer(&database).expect("offline writer");
        let refresh = index(&mut writer, &options).expect("nonfatal refresh");

        assert_eq!(refresh.purged_conversations, 0);
        assert_eq!(writer.counts().expect("counts").conversations, 1);
        assert_eq!(
            writer
                .search("durable", 10, None, None)
                .expect("preserved search row")
                .len(),
            1
        );
        drop(writer);

        fs::write(&root, "not a JSONL history").expect("replace root with unusable file");
        let mut writer = Storage::open_writer(&database).expect("wrong-type writer");
        let refresh = index(&mut writer, &options).expect("nonfatal wrong-type refresh");
        assert_eq!(refresh.purged_conversations, 0);
        assert_eq!(writer.counts().expect("counts").conversations, 1);
    }

    #[veritas::claims("indexing/inaccessible-roots-never-authorize-purge")]
    #[test]
    fn configured_root_disappearing_after_discovery_rolls_back_without_purge() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let root = directory.path().join("configured");
        fs::create_dir(&root).expect("configured root");
        let discovery = discover_jsonl_files(std::slice::from_ref(&root), true)
            .expect("initial empty discovery");
        let database = directory.path().join("cass.sqlite3");
        let mut writer = Storage::open_writer(&database).expect("seed writer");
        writer
            .replace_conversation(&test_conversation(
                "existing",
                "claude-code",
                root.join("missing.jsonl"),
                "durable sentinel",
            ))
            .expect("seed conversation");
        writer.commit_writer().expect("commit seed");
        drop(writer);
        fs::remove_dir(&root).expect("remove configured root");

        let mut writer = Storage::open_writer(&database).expect("configured writer");
        let error = index_provider(
            &mut writer,
            ProviderScan {
                provider: "claude-code",
                roots: std::slice::from_ref(&root),
                cutoff_modified_ns: None,
                parse: parse_claude,
                authoritative: true,
            },
            discovery,
            &mut empty_summary(),
            &mut IndexProgress::new(),
        )
        .expect_err("disappearing configured root");

        assert_eq!(error.error.kind, "configuration");
        assert_eq!(writer.counts().expect("counts").conversations, 1);
        writer.commit_writer().expect("commit unchanged state");
        assert_eq!(writer.search("durable", 10, None, None).unwrap().len(), 1);
    }

    #[veritas::claims("indexing/inaccessible-roots-never-authorize-purge")]
    #[test]
    fn late_configured_source_failure_rolls_back_provider_changes() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let database = directory.path().join("cass.sqlite3");
        let old_source = directory.path().join("old.jsonl");
        let mut writer = Storage::open_writer(&database).expect("seed writer");
        for (id, content) in [("existing", "durable sentinel"), ("forgotten", "forgotten")] {
            writer
                .replace_conversation(&test_conversation(
                    id,
                    "claude-code",
                    if id == "existing" {
                        old_source.clone()
                    } else {
                        directory.path().join("forgotten.jsonl")
                    },
                    content,
                ))
                .expect("seed conversation");
        }
        writer
            .record_source_checkpoint("claude-code", old_source.to_str().unwrap(), 10, 20)
            .expect("seed checkpoint");
        writer
            .replace_embeddings(
                "test-generation",
                &[crate::storage::EmbeddingWrite {
                    message_id: "existing-message",
                    vector: &[127],
                    norm: 127.0,
                }],
            )
            .expect("seed embedding");
        writer.commit_writer().expect("commit seed");
        drop(writer);
        let mut storage = Storage::open(&database).expect("forget storage");
        assert!(storage.forget("forgotten").expect("seed tombstone"));
        drop(storage);

        let root = directory.path().join("configured");
        fs::create_dir(&root).expect("configured root");
        let valid = root.join("valid.jsonl");
        let failure = root.join("failure.jsonl");
        write_claude_session(&valid, "new-session", "uncommitted sentinel");
        fs::write(&failure, "placeholder\n").expect("failure fixture");
        let discovery = discover_jsonl_files(std::slice::from_ref(&root), true)
            .expect("initial configured discovery");
        let mut writer = Storage::open_writer(&database).expect("configured writer");
        let mut summary = empty_summary();
        let error = index_provider(
            &mut writer,
            ProviderScan {
                provider: "claude-code",
                roots: std::slice::from_ref(&root),
                cutoff_modified_ns: None,
                parse: parse_with_late_failure,
                authoritative: true,
            },
            discovery,
            &mut summary,
            &mut IndexProgress::new(),
        )
        .expect_err("late source failure");

        assert_eq!(error.error.kind, "configuration");
        assert_eq!(summary.committed_batches, 0);
        assert_eq!(writer.counts().expect("counts").conversations, 1);
        assert_eq!(writer.embedding_count("test-generation").unwrap(), 1);
        assert!(
            writer
                .source_checkpoint_matches("claude-code", old_source.to_str().unwrap(), 10, 20)
                .unwrap()
        );
        assert_eq!(writer.search("durable", 10, None, None).unwrap().len(), 1);
        assert!(
            writer
                .search("uncommitted", 10, None, None)
                .unwrap()
                .is_empty()
        );
        let tombstone = writer
            .replace_conversation(&test_conversation(
                "forgotten",
                "claude-code",
                directory.path().join("forgotten.jsonl"),
                "forgotten",
            ))
            .expect("tombstone remains");
        assert!(tombstone.tombstoned);
    }

    #[veritas::claims(
        "indexing/recency-horizon-bounds-admission-not-retention",
        "indexing/recency-exclusion-preserves-stored-state"
    )]
    #[test]
    fn horizon_admits_the_boundary_and_preserves_deleted_old_sources() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let root = directory.path().join("claude");
        fs::create_dir(&root).expect("Claude root");
        let old_path = root.join("old.jsonl");
        let recent_path = root.join("recent.jsonl");
        write_claude_session(&old_path, "old-session", "old sentinel");
        fs::write(
            &recent_path,
            concat!(
                "{\"type\":\"user\",\"sessionId\":\"recent-session\",\"uuid\":\"recent-1\",\"timestamp\":\"2020-01-01T00:00:00Z\",\"message\":{\"role\":\"user\",\"content\":\"recent sentinel\"}}\n",
                "{\"type\":\"assistant\",\"sessionId\":\"recent-session\",\"uuid\":\"recent-2\",\"timestamp\":\"2020-01-02T00:00:00Z\",\"message\":{\"role\":\"assistant\",\"content\":\"complete conversation sentinel\"}}\n"
            ),
        )
        .expect("boundary fixture");
        let run_started = UNIX_EPOCH + Duration::from_hours(200 * 24);
        let boundary = run_started - Duration::from_hours(90 * 24);
        set_modified(&old_path, boundary - Duration::from_nanos(1));
        set_modified(&recent_path, boundary);
        let database = directory.path().join("cass.sqlite3");
        let bounded = IndexOptions {
            claude_code: Some(vec![root.clone()]),
            codex: None,
            roots_are_authoritative: true,
            since_days: Some(90),
        };
        let mut writer = Storage::open_writer(&database).expect("bounded writer");
        let admitted = index_at(&mut writer, &bounded, run_started).expect("bounded index");
        assert_eq!(admitted.scanned_files, 1);
        assert_eq!(writer.counts().expect("bounded counts").conversations, 1);
        assert_eq!(writer.counts().expect("bounded counts").messages, 2);
        writer.commit_writer().expect("commit bounded index");
        assert_eq!(writer.search("complete", 10, None, None).unwrap().len(), 1);
        assert!(writer.search("old", 10, None, None).unwrap().is_empty());
        drop(writer);

        let all_history = IndexOptions {
            since_days: None,
            ..bounded
        };
        let mut writer = Storage::open_writer(&database).expect("all-history writer");
        let archival = index_at(&mut writer, &all_history, run_started).expect("all-history index");
        assert_eq!(archival.scanned_files, 2);
        assert_eq!(
            writer.counts().expect("all-history counts").conversations,
            2
        );
        let old_message_id = stable_id(
            "message",
            &["claude-code", &old_path.to_string_lossy(), "old-session-m1"],
        );
        writer
            .replace_embeddings(
                "old-generation",
                &[crate::storage::EmbeddingWrite {
                    message_id: &old_message_id,
                    vector: &[127],
                    norm: 127.0,
                }],
            )
            .expect("old embedding");
        let (old_size_bytes, old_modified_ns) = source_stamp(&old_path).expect("old source stamp");
        writer.commit_writer().expect("commit all-history index");
        drop(writer);

        fs::remove_file(&old_path).expect("remove old source");
        fs::remove_file(&recent_path).expect("remove recent source");
        let bounded = IndexOptions {
            since_days: Some(90),
            ..all_history
        };
        let mut writer = Storage::open_writer(&database).expect("bounded writer");
        let refresh = index_at(&mut writer, &bounded, run_started).expect("bounded refresh");

        assert_eq!(refresh.purged_conversations, 1);
        assert_eq!(writer.counts().expect("counts").conversations, 1);
        assert_eq!(writer.embedding_count("old-generation").unwrap(), 1);
        assert!(
            writer
                .source_checkpoint_matches(
                    "claude-code",
                    old_path.to_str().unwrap(),
                    old_size_bytes,
                    old_modified_ns,
                )
                .expect("old checkpoint")
        );
        writer.commit_writer().expect("commit bounded refresh");
        assert_eq!(writer.search("old", 10, None, None).unwrap().len(), 1);
        assert!(writer.search("recent", 10, None, None).unwrap().is_empty());
    }

    #[veritas::claims("indexing/partial-provider-scan-preserves-others")]
    #[test]
    fn selecting_one_provider_never_reconciles_an_unselected_provider() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let root = directory.path().join("codex");
        fs::create_dir(&root).expect("Codex root");
        let database = directory.path().join("cass.sqlite3");
        let mut writer = Storage::open_writer(&database).expect("writer");
        for (id, provider, source_path) in [
            (
                "claude-session",
                "claude-code",
                directory.path().join("claude.jsonl"),
            ),
            ("codex-session", "codex", root.join("missing.jsonl")),
            (
                "claude-forgotten",
                "claude-code",
                directory.path().join("claude-forgotten.jsonl"),
            ),
        ] {
            writer
                .replace_conversation(&test_conversation(
                    id,
                    provider,
                    source_path,
                    &format!("{provider} sentinel"),
                ))
                .expect("seed conversation");
        }
        writer
            .record_source_checkpoint(
                "claude-code",
                directory.path().join("claude.jsonl").to_str().unwrap(),
                10,
                20,
            )
            .expect("Claude checkpoint");
        writer
            .replace_embeddings(
                "claude-generation",
                &[crate::storage::EmbeddingWrite {
                    message_id: "claude-session-message",
                    vector: &[127],
                    norm: 127.0,
                }],
            )
            .expect("Claude embedding");
        writer.commit_writer().expect("commit seed");
        drop(writer);
        let mut storage = Storage::open(&database).expect("tombstone storage");
        assert!(
            storage
                .forget("claude-forgotten")
                .expect("Claude tombstone")
        );
        drop(storage);

        let mut writer = Storage::open_writer(&database).expect("refresh writer");
        let summary = index(
            &mut writer,
            &IndexOptions {
                claude_code: None,
                codex: Some(vec![root]),
                roots_are_authoritative: true,
                since_days: None,
            },
        )
        .expect("Codex-only refresh");

        assert_eq!(summary.purged_conversations, 1);
        assert_eq!(writer.counts().expect("counts").conversations, 1);
        assert_eq!(writer.embedding_count("claude-generation").unwrap(), 1);
        assert!(
            writer
                .source_checkpoint_matches(
                    "claude-code",
                    directory.path().join("claude.jsonl").to_str().unwrap(),
                    10,
                    20,
                )
                .unwrap()
        );
        assert_eq!(
            writer
                .search("claude", 10, None, None)
                .expect("Claude remains")
                .len(),
            1
        );
        let tombstone = writer
            .replace_conversation(&test_conversation(
                "claude-forgotten",
                "claude-code",
                directory.path().join("claude-forgotten.jsonl"),
                "forgotten",
            ))
            .expect("Claude tombstone remains");
        assert!(tombstone.tombstoned);
    }
}
