use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

#[cfg(test)]
use clap::CommandFactory;
use clap::{Parser, Subcommand};
use directories::ProjectDirs;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::AppError;
use crate::federation::{self, SearchEnvelope, SearchRequest, ViewEnvelope, ViewRequest};
use crate::ingestion::{self, ProviderRoots};
use crate::semantic::{self, Models};
use crate::storage::Storage;

#[derive(Debug, Parser)]
#[command(name = "cass", version, about, arg_required_else_help = true)]
struct Cli {
    /// Canonical CASS SQLite database.
    #[arg(long, global = true, value_name = "PATH")]
    db: Option<PathBuf>,

    /// Semantic model asset directory.
    #[arg(long, global = true, value_name = "PATH")]
    models_dir: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Discover and index supported local histories.
    Index {
        /// Recreate all derived search state.
        #[arg(long)]
        full: bool,
    },
    /// Search indexed messages.
    Search {
        query: Option<String>,
        #[arg(long, default_value_t = 20)]
        limit: usize,
        #[arg(long)]
        provider: Option<String>,
        #[arg(long)]
        days: Option<u32>,
        /// Search an SSH node in addition to this machine.
        #[arg(long, value_name = "ALIAS")]
        node: Vec<String>,
        #[arg(long, hide = true)]
        federation_request: bool,
    },
    /// Return a message and adjacent context.
    View {
        id: Option<String>,
        #[arg(long, default_value_t = 0)]
        context: u32,
        /// Read the message from an SSH node.
        #[arg(long, value_name = "ALIAS")]
        node: Option<String>,
        #[arg(long, hide = true)]
        federation_request: bool,
    },
    /// Report canonical storage and model readiness.
    Status,
    /// Remove one conversation and its derived search rows.
    Forget { id: String },
    /// Manage semantic retrieval models.
    Models {
        #[command(subcommand)]
        command: ModelsCommand,
    },
}

#[derive(Debug, Subcommand)]
enum ModelsCommand {
    /// Explicitly install embedding and reranking models.
    Install,
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
pub(super) enum Response {
    Index(IndexResponse),
    Search(SearchResponse),
    FederationSearch(SearchEnvelope),
    View(ViewResponse),
    FederationView(ViewEnvelope),
    Status(StatusResponse),
    Forget(ForgetResponse),
    ModelsInstall(ModelsInstallResponse),
}

#[derive(Debug, Serialize)]
pub(super) struct IndexResponse {
    indexed_conversations: u64,
    indexed_messages: u64,
    searchable_messages: u64,
    scanned_files: u64,
    malformed_records: u64,
    changed_messages: u64,
    removed_messages: u64,
    unchanged_sources: u64,
    checkpoint_skipped_sources: u64,
    tombstoned_sources: u64,
    purged_conversations: u64,
    committed_batches: u64,
    discovered_bytes: u64,
    processed_bytes: u64,
    full: bool,
    embeddings: u64,
    realized_mode: &'static str,
    model_load_milliseconds: u64,
    storage_setup_milliseconds: u64,
    ingestion_milliseconds: u64,
    search_refresh_milliseconds: u64,
    embedding_milliseconds: u64,
    total_milliseconds: u64,
}

#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct SearchResponse {
    pub(crate) query: String,
    pub(crate) realized_mode: String,
    pub(crate) results: Vec<crate::storage::SearchHit>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) nodes: Option<Vec<federation::NodeOutcome>>,
}

#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct ViewResponse {
    pub(crate) id: String,
    pub(crate) messages: Vec<crate::storage::Message>,
}

#[derive(Debug, Serialize)]
pub(super) struct StatusResponse {
    ready: bool,
    database_path: PathBuf,
    conversations: u64,
    messages: u64,
    searchable_messages: u64,
    embeddings: u64,
    stored_embeddings: u64,
    models_installed: bool,
    semantic_support: bool,
    realized_mode: &'static str,
    recommended_action: Option<&'static str>,
}

#[derive(Debug, Serialize)]
pub(super) struct ForgetResponse {
    id: String,
    forgotten: bool,
}

#[derive(Debug, Serialize)]
pub(super) struct ModelsInstallResponse {
    installed: bool,
    model_directory: PathBuf,
    #[serde(flatten)]
    summary: semantic::InstallSummary,
    recommended_action: &'static str,
}

pub(super) fn run<I, T>(args: I) -> Result<Response, AppError>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let cli = Cli::try_parse_from(args).map_err(|error| {
        let _ = error.print();
        AppError::usage(error.to_string())
    })?;
    let database_path = cli.db.unwrap_or_else(default_database_path);
    let models_dir = cli.models_dir.unwrap_or_else(default_models_path);

    match cli.command {
        Command::Index { full } => index(&database_path, &models_dir, full, &ProviderRoots::new()),
        Command::Search {
            query,
            limit,
            provider,
            days,
            node,
            federation_request,
        } => {
            if federation_request {
                if query.is_some() || !node.is_empty() {
                    return Err(AppError::usage(
                        "--federation-request reads its search request from stdin",
                    ));
                }
                let request: SearchRequest = read_federation_request()?;
                federation::validate_protocol(request.protocol)?;
                local_search(
                    &database_path,
                    &models_dir,
                    request.query,
                    request.limit,
                    request.provider.as_deref(),
                    request.days,
                )
                .map(|response| Response::FederationSearch(SearchEnvelope::new(response)))
            } else {
                let query = query.ok_or_else(|| AppError::usage("search requires a query"))?;
                let nodes = federation::select_nodes(&node)?;
                search(
                    &database_path,
                    &models_dir,
                    query,
                    limit,
                    provider.as_deref(),
                    days,
                    &nodes,
                )
            }
        }
        Command::View {
            id,
            context,
            node,
            federation_request,
        } => {
            if federation_request {
                if id.is_some() || node.is_some() {
                    return Err(AppError::usage(
                        "--federation-request reads its view request from stdin",
                    ));
                }
                let request: ViewRequest = read_federation_request()?;
                federation::validate_protocol(request.protocol)?;
                local_view(&database_path, request.id, request.context)
                    .map(|response| Response::FederationView(ViewEnvelope::new(response)))
            } else {
                let id = id.ok_or_else(|| AppError::usage("view requires an id"))?;
                view(&database_path, id, context, node)
            }
        }
        Command::Status => status(&database_path, &models_dir),
        Command::Forget { id } => forget(&database_path, id),
        Command::Models {
            command: ModelsCommand::Install,
        } => install_models(&models_dir),
    }
}

fn index(
    database_path: &Path,
    models_dir: &Path,
    full: bool,
    roots: &ProviderRoots,
) -> Result<Response, AppError> {
    let total_started = Instant::now();
    let models = Models::new(models_dir.to_path_buf());
    let model_started = Instant::now();
    let mut backend = models.load()?.ok_or_else(|| {
        AppError::model("semantic models are not installed; run `cass models install`")
    })?;
    let model_load_milliseconds = elapsed_milliseconds(model_started);
    let storage_started = Instant::now();
    let mut storage = Storage::open_writer(database_path)?;
    if full || storage.derived_search_is_dirty()? {
        storage.defer_search_updates()?;
    }
    if full {
        storage.mark_derived_search_dirty()?;
    }
    let storage_setup_milliseconds = elapsed_milliseconds(storage_started);
    let ingestion_started = Instant::now();
    let summary = ingestion::index(&mut storage, roots)?;
    let ingestion_milliseconds = elapsed_milliseconds(ingestion_started);
    let search_started = Instant::now();
    if storage.derived_search_is_dirty()? {
        emit_index_phase("search-rebuild", &summary);
        storage.rebuild_derived_search_state()?;
    }
    let search_refresh_milliseconds = elapsed_milliseconds(search_started);
    emit_index_phase("semantic-embeddings", &summary);
    let embedding_started = Instant::now();
    storage.invalidate_embedding_generation(semantic::embedding_generation())?;
    let embeddings = semantic::rebuild_embeddings(&mut storage, &mut backend)?;
    let embedding_milliseconds = elapsed_milliseconds(embedding_started);
    let counts = storage.counts()?;
    if !storage.semantic_coverage_is_complete(semantic::embedding_generation())? {
        return Err(AppError::search_not_ready(
            "semantic embedding coverage is incomplete",
        ));
    }
    storage.commit_writer()?;
    emit_index_phase("complete", &summary);
    Ok(Response::Index(IndexResponse {
        indexed_conversations: counts.conversations,
        indexed_messages: counts.messages,
        searchable_messages: counts.searchable_messages,
        scanned_files: summary.scanned_files,
        malformed_records: summary.malformed_records,
        changed_messages: summary.changed_messages,
        removed_messages: summary.removed_messages,
        unchanged_sources: summary.unchanged_sources,
        checkpoint_skipped_sources: summary.checkpoint_skipped_sources,
        tombstoned_sources: summary.tombstoned_sources,
        purged_conversations: summary.purged_conversations,
        committed_batches: summary.committed_batches,
        discovered_bytes: summary.discovered_bytes,
        processed_bytes: summary.processed_bytes,
        full,
        embeddings,
        realized_mode: "hybrid",
        model_load_milliseconds,
        storage_setup_milliseconds,
        ingestion_milliseconds,
        search_refresh_milliseconds,
        embedding_milliseconds,
        total_milliseconds: elapsed_milliseconds(total_started),
    }))
}

fn elapsed_milliseconds(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

fn emit_index_phase(phase: &'static str, summary: &ingestion::IndexSummary) {
    let event = serde_json::json!({
        "event": "index-progress",
        "phase": phase,
        "processed_files": summary.processed_files,
        "scanned_files": summary.scanned_files,
        "processed_bytes": summary.processed_bytes,
        "discovered_bytes": summary.discovered_bytes,
        "changed_messages": summary.changed_messages,
        "committed_batches": summary.committed_batches,
    });
    let stderr = std::io::stderr();
    let mut stderr = stderr.lock();
    if serde_json::to_writer(&mut stderr, &event).is_ok() {
        let _ = writeln!(stderr);
    }
}

fn search(
    database_path: &Path,
    models_dir: &Path,
    query: String,
    limit: usize,
    provider: Option<&str>,
    days: Option<u32>,
    nodes: &[String],
) -> Result<Response, AppError> {
    if nodes.is_empty() {
        return local_search(database_path, models_dir, query, limit, provider, days)
            .map(Response::Search);
    }
    let request = SearchRequest::new(query.clone(), limit, provider.map(ToOwned::to_owned), days);
    let (local, remote) = std::thread::scope(|scope| {
        let handles = nodes
            .iter()
            .map(|node| {
                let node = node.clone();
                let request = &request;
                (
                    node.clone(),
                    scope.spawn(move || federation::remote_search(node, request)),
                )
            })
            .collect::<Vec<_>>();
        let local = local_search(database_path, models_dir, query, limit, provider, days);
        let remote = handles
            .into_iter()
            .map(|(node, handle)| {
                handle
                    .join()
                    .unwrap_or_else(|_| federation::thread_failure(node))
            })
            .collect::<Vec<_>>();
        (local, remote)
    });
    Ok(Response::Search(federation::merge_search(
        local?, remote, limit,
    )))
}

fn local_search(
    database_path: &Path,
    models_dir: &Path,
    query: String,
    limit: usize,
    provider: Option<&str>,
    days: Option<u32>,
) -> Result<SearchResponse, AppError> {
    let provider = normalize_provider_filter(provider)?;
    let models = Models::new(models_dir.to_path_buf());
    models.require_installed()?;
    let storage = Storage::open_existing(database_path)?;
    let counts = storage.counts()?;
    if storage.derived_search_is_dirty()?
        || counts.messages == 0
        || !storage.semantic_coverage_is_complete(semantic::embedding_generation())?
    {
        return Err(AppError::search_not_ready(
            "semantic index is incomplete; run `cass index`",
        ));
    }
    let mut backend = models.load()?.ok_or_else(|| {
        AppError::model("semantic models are not installed; run `cass models install`")
    })?;
    let results = semantic::hybrid_search(&storage, &mut backend, &query, limit, provider, days)?;
    Ok(SearchResponse {
        query,
        realized_mode: "hybrid".to_owned(),
        results,
        nodes: None,
    })
}

fn normalize_provider_filter(provider: Option<&str>) -> Result<Option<&'static str>, AppError> {
    match provider {
        None => Ok(None),
        Some("claude" | "claude-code" | "claude_code") => Ok(Some("claude-code")),
        Some("codex") => Ok(Some("codex")),
        Some("opencode" | "open-code" | "open_code") => Ok(Some("opencode")),
        Some("github-copilot" | "copilot" | "github_copilot") => Ok(Some("github-copilot")),
        Some("hermes" | "hermes-agent" | "hermes_agent") => Ok(Some("hermes")),
        Some("pi" | "pi-agent" | "pi_agent") => Ok(Some("pi")),
        Some(provider) => Err(AppError::usage(format!(
            "unsupported provider filter: {provider}"
        ))),
    }
}

fn view(
    database_path: &Path,
    id: String,
    context: u32,
    node: Option<String>,
) -> Result<Response, AppError> {
    let Some(node) = node else {
        return local_view(database_path, id, context).map(Response::View);
    };
    federation::validate_node(&node)?;
    let remote = federation::remote_view(node, &ViewRequest::new(id, context));
    if let Some(response) = remote.response {
        return Ok(Response::View(response));
    }
    let kind = remote.outcome.error_kind.as_deref().unwrap_or("remote");
    let detail = remote
        .outcome
        .error
        .as_deref()
        .unwrap_or("remote view failed");
    Err(AppError::internal(format!(
        "remote view on {} failed ({kind}): {detail}",
        remote.outcome.node
    )))
}

fn local_view(database_path: &Path, id: String, context: u32) -> Result<ViewResponse, AppError> {
    let storage = Storage::open_existing(database_path)?;
    let messages = storage.view(&id, context)?;
    Ok(ViewResponse { id, messages })
}

fn read_federation_request<T: DeserializeOwned>() -> Result<T, AppError> {
    let mut input = Vec::new();
    std::io::stdin()
        .take(16 * 1024 * 1024)
        .read_to_end(&mut input)
        .map_err(AppError::io)?;
    serde_json::from_slice(&input)
        .map_err(|error| AppError::usage(format!("invalid federation request: {error}")))
}

fn status(database_path: &Path, models_dir: &Path) -> Result<Response, AppError> {
    let models_installed = Models::new(models_dir.to_path_buf()).is_installed();
    if !database_path.is_file() {
        return Ok(Response::Status(StatusResponse {
            ready: false,
            database_path: database_path.to_path_buf(),
            conversations: 0,
            messages: 0,
            searchable_messages: 0,
            embeddings: 0,
            stored_embeddings: 0,
            models_installed,
            semantic_support: true,
            realized_mode: "unavailable",
            recommended_action: Some(if models_installed {
                "index"
            } else {
                "models install"
            }),
        }));
    }

    let snapshot = Storage::status_snapshot(database_path, semantic::embedding_generation())?;
    let counts = snapshot.counts;
    let current_embeddings = snapshot.current_embeddings;
    let hybrid_ready = models_installed
        && snapshot.derived_clean
        && counts.messages > 0
        && snapshot.exact_semantic_coverage;
    Ok(Response::Status(StatusResponse {
        ready: hybrid_ready,
        database_path: database_path.to_path_buf(),
        conversations: counts.conversations,
        messages: counts.messages,
        searchable_messages: counts.searchable_messages,
        embeddings: current_embeddings,
        stored_embeddings: counts.embeddings,
        models_installed,
        semantic_support: true,
        realized_mode: if hybrid_ready {
            "hybrid"
        } else {
            "unavailable"
        },
        recommended_action: if !models_installed {
            Some("models install")
        } else if hybrid_ready {
            None
        } else {
            Some("index")
        },
    }))
}

fn install_models(models_dir: &Path) -> Result<Response, AppError> {
    let summary = Models::new(models_dir.to_path_buf()).install()?;
    Ok(Response::ModelsInstall(ModelsInstallResponse {
        installed: true,
        model_directory: models_dir.to_path_buf(),
        summary,
        recommended_action: "index",
    }))
}

fn forget(database_path: &Path, id: String) -> Result<Response, AppError> {
    let mut storage = Storage::open_existing(database_path)?;
    let forgotten = storage.forget(&id)?;
    Ok(Response::Forget(ForgetResponse { id, forgotten }))
}

fn default_database_path() -> PathBuf {
    ProjectDirs::from("dev", "jamesqo", "cass").map_or_else(
        || PathBuf::from("cass.sqlite3"),
        |directories| directories.data_local_dir().join("cass.sqlite3"),
    )
}

fn default_models_path() -> PathBuf {
    ProjectDirs::from("dev", "jamesqo", "cass").map_or_else(
        || PathBuf::from("models"),
        |directories| directories.data_local_dir().join("models"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use veritas_test_macros as veritas;

    #[veritas::claims("cli/operational-command-surface")]
    #[test]
    fn command_surface_contains_only_the_contract_commands() {
        let command = Cli::command();
        let names: Vec<_> = command
            .get_subcommands()
            .map(|command| command.get_name().to_owned())
            .collect();
        assert_eq!(
            names,
            ["index", "search", "view", "status", "forget", "models"]
        );
    }
}
