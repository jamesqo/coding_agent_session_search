mod cli;
mod federation;
mod ingestion;
#[cfg(feature = "semantic")]
mod semantic;
#[cfg(not(feature = "semantic"))]
mod semantic_disabled;
#[cfg(not(feature = "semantic"))]
use semantic_disabled as semantic;
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
}

impl AppError {
    fn message(&self) -> &str {
        &self.error.message
    }

    fn usage(message: impl Into<String>) -> Self {
        Self::new(2, "usage", message, false)
    }

    fn missing_database(path: &std::path::Path) -> Self {
        Self::new(
            3,
            "database-missing",
            format!("database does not exist: {}", path.display()),
            true,
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

    #[cfg(feature = "semantic")]
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
        Self::new(4, "model", message, true)
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
            },
            code,
        }
    }
}
