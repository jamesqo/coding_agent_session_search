use std::path::PathBuf;

use serde::Serialize;

use crate::AppError;
use crate::storage::{SearchHit, Storage};

pub(crate) struct Models {
    root: PathBuf,
    enabled: bool,
}

pub(crate) struct Backend {
    enabled: bool,
}

#[derive(Debug, Serialize)]
pub(crate) struct InstallSummary {
    semantic_support: &'static str,
}

impl Models {
    pub(crate) fn new(root: PathBuf) -> Self {
        Self {
            root,
            enabled: false,
        }
    }

    pub(crate) fn is_installed(&self) -> bool {
        self.enabled && self.root.is_dir()
    }

    pub(crate) fn install(&self) -> Result<InstallSummary, AppError> {
        Err(AppError::model(format!(
            "semantic support is unavailable in this lexical-only build ({})",
            self.root.display()
        )))
    }

    pub(crate) fn load(&self) -> Result<Option<Backend>, AppError> {
        if self.enabled {
            Err(AppError::internal(
                "lexical-only build reported semantic support",
            ))
        } else {
            Ok(None)
        }
    }
}

pub(crate) fn rebuild_embeddings(
    storage: &mut Storage,
    backend: &mut Backend,
) -> Result<u64, AppError> {
    let _ = storage.counts()?;
    if backend.enabled {
        Err(AppError::internal(
            "lexical-only backend attempted embedding inference",
        ))
    } else {
        Ok(0)
    }
}

pub(crate) fn hybrid_search(
    _storage: &Storage,
    _backend: &mut Backend,
    _query: &str,
    _limit: usize,
    _provider: Option<&str>,
    _days: Option<u32>,
) -> Result<Vec<SearchHit>, AppError> {
    Err(AppError::model(
        "semantic support is unavailable in this lexical-only build",
    ))
}
