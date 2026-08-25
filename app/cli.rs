use std::ffi::OsString;
use std::path::{Path, PathBuf};

#[cfg(test)]
use clap::CommandFactory;
use clap::{Parser, Subcommand};
use directories::ProjectDirs;
use serde::Serialize;

use crate::AppError;
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
        query: String,
        #[arg(long, default_value_t = 20)]
        limit: usize,
        #[arg(long)]
        provider: Option<String>,
        #[arg(long)]
        days: Option<u32>,
    },
    /// Return a message and adjacent context.
    View {
        id: String,
        #[arg(long, default_value_t = 0)]
        context: u32,
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
    View(ViewResponse),
    Status(StatusResponse),
    Forget(ForgetResponse),
    ModelsInstall(ModelsInstallResponse),
}

#[derive(Debug, Serialize)]
pub(super) struct IndexResponse {
    indexed_conversations: u64,
    indexed_messages: u64,
    scanned_files: u64,
    malformed_records: u64,
    changed_messages: u64,
    removed_messages: u64,
    unchanged_sources: u64,
    tombstoned_sources: u64,
    purged_conversations: u64,
    full: bool,
    embeddings: u64,
    realized_mode: &'static str,
    fallback_reason: Option<String>,
}

#[derive(Debug, Serialize)]
pub(super) struct SearchResponse {
    query: String,
    realized_mode: &'static str,
    fallback_mode: Option<&'static str>,
    fallback_reason: Option<String>,
    results: Vec<crate::storage::SearchHit>,
}

#[derive(Debug, Serialize)]
pub(super) struct ViewResponse {
    id: String,
    messages: Vec<crate::storage::Message>,
}

#[derive(Debug, Serialize)]
pub(super) struct StatusResponse {
    ready: bool,
    database_path: PathBuf,
    conversations: u64,
    messages: u64,
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
        } => search(
            &database_path,
            &models_dir,
            query,
            limit,
            provider.as_deref(),
            days,
        ),
        Command::View { id, context } => view(&database_path, id, context),
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
    let mut storage = Storage::open_writer(database_path)?;
    let summary = ingestion::index(&mut storage, roots)?;
    if full {
        storage.rebuild_derived_search_state()?;
    }
    let models = Models::new(models_dir.to_path_buf());
    let (embeddings, fallback_reason) = match models.load() {
        Ok(Some(mut backend)) => match semantic::rebuild_embeddings(&mut storage, &mut backend) {
            Ok(count) => (count, None),
            Err(error) => (0, Some(error.message().to_owned())),
        },
        Ok(None) => (0, semantic_unavailable_reason()),
        Err(error) => (0, Some(error.message().to_owned())),
    };
    let counts = storage.counts()?;
    storage.commit_writer()?;
    let hybrid_ready = counts.messages > 0 && counts.embeddings == counts.messages;
    Ok(Response::Index(IndexResponse {
        indexed_conversations: counts.conversations,
        indexed_messages: counts.messages,
        scanned_files: summary.scanned_files,
        malformed_records: summary.malformed_records,
        changed_messages: summary.changed_messages,
        removed_messages: summary.removed_messages,
        unchanged_sources: summary.unchanged_sources,
        tombstoned_sources: summary.tombstoned_sources,
        purged_conversations: summary.purged_conversations,
        full,
        embeddings,
        realized_mode: if hybrid_ready { "hybrid" } else { "lexical" },
        fallback_reason,
    }))
}

fn search(
    database_path: &Path,
    models_dir: &Path,
    query: String,
    limit: usize,
    provider: Option<&str>,
    days: Option<u32>,
) -> Result<Response, AppError> {
    let provider = normalize_provider_filter(provider)?;
    let storage = Storage::open_existing(database_path)?;
    let counts = storage.counts()?;
    let models = Models::new(models_dir.to_path_buf());
    let backend = if counts.embeddings > 0 {
        models.load()
    } else {
        Ok(None)
    };
    let (results, realized_mode, fallback_mode, fallback_reason) = match backend {
        Ok(Some(mut backend)) => {
            match semantic::hybrid_search(&storage, &mut backend, &query, limit, provider, days) {
                Ok(results) => (results, "hybrid", None, None),
                Err(error) => (
                    storage.search(&query, limit, provider, days)?,
                    "lexical",
                    Some("lexical"),
                    Some(error.message().to_owned()),
                ),
            }
        }
        Ok(None) => (
            storage.search(&query, limit, provider, days)?,
            "lexical",
            Some("lexical"),
            semantic_unavailable_reason(),
        ),
        Err(error) => (
            storage.search(&query, limit, provider, days)?,
            "lexical",
            Some("lexical"),
            Some(error.message().to_owned()),
        ),
    };
    Ok(Response::Search(SearchResponse {
        query,
        realized_mode,
        fallback_mode,
        fallback_reason,
        results,
    }))
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

fn view(database_path: &Path, id: String, context: u32) -> Result<Response, AppError> {
    let storage = Storage::open_existing(database_path)?;
    let messages = storage.view(&id, context)?;
    Ok(Response::View(ViewResponse { id, messages }))
}

fn status(database_path: &Path, models_dir: &Path) -> Result<Response, AppError> {
    let models_installed = Models::new(models_dir.to_path_buf()).is_installed();
    if !database_path.is_file() {
        return Ok(Response::Status(StatusResponse {
            ready: false,
            database_path: database_path.to_path_buf(),
            conversations: 0,
            messages: 0,
            models_installed,
            semantic_support: cfg!(feature = "semantic"),
            realized_mode: "unavailable",
            recommended_action: Some("index"),
        }));
    }

    let storage = Storage::open_existing(database_path)?;
    let counts = storage.counts()?;
    let hybrid_ready =
        models_installed && counts.messages > 0 && counts.embeddings == counts.messages;
    Ok(Response::Status(StatusResponse {
        ready: true,
        database_path: database_path.to_path_buf(),
        conversations: counts.conversations,
        messages: counts.messages,
        models_installed,
        semantic_support: cfg!(feature = "semantic"),
        realized_mode: if hybrid_ready { "hybrid" } else { "lexical" },
        recommended_action: (models_installed && counts.embeddings != counts.messages)
            .then_some("index"),
    }))
}

fn semantic_unavailable_reason() -> Option<String> {
    if cfg!(feature = "semantic") {
        None
    } else {
        Some("semantic support is unavailable in this lexical-only build".to_owned())
    }
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
