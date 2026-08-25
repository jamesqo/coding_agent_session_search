mod cli;
mod federation;
mod ingestion;
mod semantic;
mod storage;

use std::ffi::OsString;
use std::io::{self, Write};

use serde::Serialize;

/// Runs CASS with the process arguments and writes its JSON response.
///
/// # Examples
///
/// ```no_run
/// let exit_code = coding_agent_search::main_entry();
/// std::process::exit(exit_code);
/// ```
#[must_use]
pub fn main_entry() -> i32 {
    execute(std::env::args_os(), &mut io::stdout(), &mut io::stderr())
}

fn execute<I, T>(args: I, stdout: &mut dyn Write, stderr: &mut dyn Write) -> i32
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    match cli::run(args) {
        Ok(response) => write_json(stdout, &response).map_or_else(
            |error| {
                let _ = write_json(stderr, &error);
                error.code
            },
            |()| 0,
        ),
        Err(error) => {
            let _ = write_json(stderr, &error);
            error.code
        }
    }
}

fn write_json<T: Serialize>(writer: &mut dyn Write, value: &T) -> Result<(), AppError> {
    serde_json::to_writer(&mut *writer, value)
        .map_err(|error| AppError::internal(format!("failed to serialize response: {error}")))?;
    writeln!(writer)
        .map_err(|error| AppError::internal(format!("failed to write response: {error}")))
}

#[derive(Debug, Serialize)]
struct AppError {
    error: ErrorBody,
    #[serde(skip)]
    code: i32,
}

#[derive(Debug, Serialize)]
struct ErrorBody {
    kind: &'static str,
    message: String,
    retryable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    recommended_action: Option<&'static str>,
}

impl AppError {
    fn usage(message: impl Into<String>) -> Self {
        Self::new(2, "usage", message, false)
    }

    fn missing_database(path: &std::path::Path) -> Self {
        Self::with_action(
            3,
            "database-missing",
            format!("database does not exist: {}", path.display()),
            true,
            "index",
        )
    }

    fn database(error: rusqlite::Error) -> Self {
        if matches!(
            &error,
            rusqlite::Error::SqliteFailure(details, _)
                if matches!(
                    details.code,
                    rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked
                )
        ) {
            return Self::new(6, "index-busy", "another index writer is active", true);
        }
        let message = error.to_string();
        drop(error);
        Self::new(5, "database", message, true)
    }

    fn database_data(message: impl Into<String>) -> Self {
        Self::new(5, "database", message, false)
    }

    fn io(error: io::Error) -> Self {
        let message = error.to_string();
        drop(error);
        Self::new(5, "io", message, true)
    }

    fn internal(message: impl Into<String>) -> Self {
        Self::new(10, "internal", message, false)
    }

    fn model(message: impl Into<String>) -> Self {
        Self::with_action(4, "model", message, true, "models install")
    }

    fn search_not_ready(message: impl Into<String>) -> Self {
        Self::with_action(8, "search-not-ready", message, true, "index")
    }

    fn schema(message: impl Into<String>) -> Self {
        Self::new(7, "schema-incompatible", message, false)
    }

    fn new(code: i32, kind: &'static str, message: impl Into<String>, retryable: bool) -> Self {
        Self {
            error: ErrorBody {
                kind,
                message: message.into(),
                retryable,
                recommended_action: None,
            },
            code,
        }
    }

    fn with_action(
        code: i32,
        kind: &'static str,
        message: impl Into<String>,
        retryable: bool,
        recommended_action: &'static str,
    ) -> Self {
        let mut error = Self::new(code, kind, message, retryable);
        error.error.recommended_action = Some(recommended_action);
        error
    }
}
