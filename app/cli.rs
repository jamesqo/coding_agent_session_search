use std::ffi::OsString;
use std::path::{Path, PathBuf};

#[cfg(test)]
use clap::CommandFactory;
use clap::{Parser, Subcommand};
use directories::ProjectDirs;
use serde::Serialize;

use crate::AppError;
use crate::ingestion::{self, ProviderRoots};
use crate::storage::Storage;

#[derive(Debug, Parser)]
#[command(name = "cass", version, about, arg_required_else_help = true)]
struct Cli {
    /// Canonical CASS SQLite database.
    #[arg(long, global = true, value_name = "PATH")]
    db: Option<PathBuf>,

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
        /// Claude Code projects directory.
        #[arg(long, value_name = "PATH")]
        claude_root: Option<PathBuf>,
        /// Codex sessions directory.
        #[arg(long, value_name = "PATH")]
        codex_root: Option<PathBuf>,
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
}

#[derive(Debug, Serialize)]
pub(super) struct IndexResponse {
    indexed_conversations: u64,
    indexed_messages: u64,
    scanned_files: u64,
    malformed_records: u64,
    full: bool,
}

#[derive(Debug, Serialize)]
pub(super) struct SearchResponse {
    query: String,
    realized_mode: &'static str,
    fallback_mode: Option<&'static str>,
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
    realized_mode: &'static str,
    recommended_action: Option<&'static str>,
}

#[derive(Debug, Serialize)]
pub(super) struct ForgetResponse {
    id: String,
    forgotten: bool,
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

    match cli.command {
        Command::Index {
            full,
            claude_root,
            codex_root,
        } => index(
            &database_path,
            full,
            &ProviderRoots::new(claude_root, codex_root),
        ),
        Command::Search {
            query,
            limit,
            provider,
            days,
        } => search(&database_path, query, limit, provider.as_deref(), days),
        Command::View { id, context } => view(&database_path, id, context),
        Command::Status => status(&database_path),
        Command::Forget { id } => forget(&database_path, id),
        Command::Models {
            command: ModelsCommand::Install,
        } => Err(AppError::unavailable(
            "semantic model installation will be enabled after backend selection",
        )),
    }
}

fn index(database_path: &Path, full: bool, roots: &ProviderRoots) -> Result<Response, AppError> {
    let mut storage = Storage::open(database_path)?;
    let summary = ingestion::index(&mut storage, roots)?;
    if full {
        storage.rebuild_derived_search_state()?;
    }
    let counts = storage.counts()?;
    Ok(Response::Index(IndexResponse {
        indexed_conversations: counts.conversations,
        indexed_messages: counts.messages,
        scanned_files: summary.scanned_files,
        malformed_records: summary.malformed_records,
        full,
    }))
}

fn search(
    database_path: &Path,
    query: String,
    limit: usize,
    provider: Option<&str>,
    days: Option<u32>,
) -> Result<Response, AppError> {
    let storage = Storage::open_existing(database_path)?;
    let results = storage.search(&query, limit, provider, days)?;
    Ok(Response::Search(SearchResponse {
        query,
        realized_mode: "lexical",
        fallback_mode: Some("lexical"),
        results,
    }))
}

fn view(database_path: &Path, id: String, context: u32) -> Result<Response, AppError> {
    let storage = Storage::open_existing(database_path)?;
    let messages = storage.view(&id, context)?;
    Ok(Response::View(ViewResponse { id, messages }))
}

fn status(database_path: &Path) -> Result<Response, AppError> {
    if !database_path.is_file() {
        return Ok(Response::Status(StatusResponse {
            ready: false,
            database_path: database_path.to_path_buf(),
            conversations: 0,
            messages: 0,
            models_installed: false,
            realized_mode: "unavailable",
            recommended_action: Some("index"),
        }));
    }

    let storage = Storage::open_existing(database_path)?;
    let counts = storage.counts()?;
    Ok(Response::Status(StatusResponse {
        ready: true,
        database_path: database_path.to_path_buf(),
        conversations: counts.conversations,
        messages: counts.messages,
        models_installed: false,
        realized_mode: "lexical",
        recommended_action: None,
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

#[cfg(test)]
mod tests {
    use super::*;

    // Veritas claim: cli/operational-command-surface
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
