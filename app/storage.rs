use std::fs;
use std::path::Path;

use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;

use crate::AppError;
use crate::ingestion::Conversation;

const SCHEMA: &str = r"
PRAGMA foreign_keys = ON;
PRAGMA journal_mode = WAL;
CREATE TABLE IF NOT EXISTS conversations (
    id TEXT PRIMARY KEY,
    provider TEXT NOT NULL CHECK (provider IN ('claude-code', 'codex')),
    source_path TEXT NOT NULL UNIQUE,
    title TEXT,
    created_at INTEGER,
    updated_at INTEGER
);
CREATE TABLE IF NOT EXISTS messages (
    id TEXT PRIMARY KEY,
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    ordinal INTEGER NOT NULL,
    role TEXT NOT NULL,
    content TEXT NOT NULL,
    created_at INTEGER,
    UNIQUE (conversation_id, ordinal)
);
CREATE VIRTUAL TABLE IF NOT EXISTS message_fts USING fts5(
    content,
    message_id UNINDEXED,
    conversation_id UNINDEXED,
    tokenize = 'unicode61'
);
";

pub(crate) struct Storage {
    connection: Connection,
}

pub(crate) struct Counts {
    pub(crate) conversations: u64,
    pub(crate) messages: u64,
}

#[derive(Debug, Serialize)]
pub(crate) struct SearchHit {
    id: String,
    conversation_id: String,
    provider: String,
    role: String,
    content: String,
    lexical_score: f64,
}

#[derive(Debug, Serialize)]
pub(crate) struct Message {
    id: String,
    ordinal: i64,
    role: String,
    content: String,
    created_at: Option<i64>,
}

impl Storage {
    pub(crate) fn open(path: &Path) -> Result<Self, AppError> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(AppError::io)?;
        }
        let connection = Connection::open(path).map_err(AppError::database)?;
        connection
            .execute_batch(SCHEMA)
            .map_err(AppError::database)?;
        Ok(Self { connection })
    }

    pub(crate) fn open_existing(path: &Path) -> Result<Self, AppError> {
        if !path.is_file() {
            return Err(AppError::missing_database(path));
        }
        Self::open(path)
    }

    pub(crate) fn counts(&self) -> Result<Counts, AppError> {
        let conversations: i64 = self
            .connection
            .query_row("SELECT count(*) FROM conversations", [], |row| row.get(0))
            .map_err(AppError::database)?;
        let messages: i64 = self
            .connection
            .query_row("SELECT count(*) FROM messages", [], |row| row.get(0))
            .map_err(AppError::database)?;
        Ok(Counts {
            conversations: u64::try_from(conversations)
                .map_err(|_| AppError::internal("negative conversation count"))?,
            messages: u64::try_from(messages)
                .map_err(|_| AppError::internal("negative message count"))?,
        })
    }

    pub(crate) fn replace_conversation(
        &mut self,
        conversation: &Conversation,
    ) -> Result<(), AppError> {
        let transaction = self.connection.transaction().map_err(AppError::database)?;
        transaction
            .execute(
                "INSERT INTO conversations(id, provider, source_path, title, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(id) DO UPDATE SET
                    provider = excluded.provider,
                    source_path = excluded.source_path,
                    title = excluded.title,
                    created_at = excluded.created_at,
                    updated_at = excluded.updated_at",
                params![
                    conversation.id,
                    conversation.provider,
                    conversation.source_path.to_string_lossy(),
                    conversation.title,
                    conversation.created_at,
                    conversation.updated_at,
                ],
            )
            .map_err(AppError::database)?;
        transaction
            .execute(
                "DELETE FROM message_fts WHERE conversation_id = ?1",
                [&conversation.id],
            )
            .map_err(AppError::database)?;
        transaction
            .execute(
                "DELETE FROM messages WHERE conversation_id = ?1",
                [&conversation.id],
            )
            .map_err(AppError::database)?;

        for message in &conversation.messages {
            transaction
                .execute(
                    "INSERT INTO messages(id, conversation_id, ordinal, role, content, created_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![
                        message.id,
                        conversation.id,
                        message.ordinal,
                        message.role,
                        message.content,
                        message.created_at,
                    ],
                )
                .map_err(AppError::database)?;
            transaction
                .execute(
                    "INSERT INTO message_fts(content, message_id, conversation_id)
                     VALUES (?1, ?2, ?3)",
                    params![message.content, message.id, conversation.id],
                )
                .map_err(AppError::database)?;
        }
        transaction.commit().map_err(AppError::database)
    }

    pub(crate) fn rebuild_derived_search_state(&self) -> Result<(), AppError> {
        self.connection
            .execute_batch(
                "DELETE FROM message_fts;
                 INSERT INTO message_fts(content, message_id, conversation_id)
                 SELECT content, id, conversation_id FROM messages;",
            )
            .map_err(AppError::database)
    }

    pub(crate) fn search(
        &self,
        query: &str,
        limit: usize,
        provider: Option<&str>,
        _days: Option<u32>,
    ) -> Result<Vec<SearchHit>, AppError> {
        if query.trim().is_empty() {
            return Err(AppError::usage("search query must not be empty"));
        }
        let limit = i64::try_from(limit.min(1_000)).unwrap_or(1_000);
        let mut statement = self
            .connection
            .prepare(
                "SELECT m.id, m.conversation_id, c.provider, m.role, m.content,
                        bm25(message_fts) AS lexical_score
                   FROM message_fts
                   JOIN messages m ON m.id = message_fts.message_id
                   JOIN conversations c ON c.id = m.conversation_id
                  WHERE message_fts MATCH ?1
                    AND (?2 IS NULL OR c.provider = ?2)
                  ORDER BY lexical_score, m.id
                  LIMIT ?3",
            )
            .map_err(AppError::database)?;
        let rows = statement
            .query_map(params![query, provider, limit], |row| {
                Ok(SearchHit {
                    id: row.get(0)?,
                    conversation_id: row.get(1)?,
                    provider: row.get(2)?,
                    role: row.get(3)?,
                    content: row.get(4)?,
                    lexical_score: row.get(5)?,
                })
            })
            .map_err(AppError::database)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(AppError::database)
    }

    pub(crate) fn view(&self, id: &str, context: u32) -> Result<Vec<Message>, AppError> {
        let anchor: Option<(String, i64)> = self
            .connection
            .query_row(
                "SELECT conversation_id, ordinal FROM messages WHERE id = ?1",
                [id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(AppError::database)?;
        let Some((conversation_id, ordinal)) = anchor else {
            return Ok(Vec::new());
        };
        let context = i64::from(context);
        let mut statement = self
            .connection
            .prepare(
                "SELECT id, ordinal, role, content, created_at
                   FROM messages
                  WHERE conversation_id = ?1
                    AND ordinal BETWEEN ?2 AND ?3
                  ORDER BY ordinal",
            )
            .map_err(AppError::database)?;
        let rows = statement
            .query_map(
                params![
                    conversation_id,
                    ordinal.saturating_sub(context),
                    ordinal.saturating_add(context)
                ],
                |row| {
                    Ok(Message {
                        id: row.get(0)?,
                        ordinal: row.get(1)?,
                        role: row.get(2)?,
                        content: row.get(3)?,
                        created_at: row.get(4)?,
                    })
                },
            )
            .map_err(AppError::database)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(AppError::database)
    }

    pub(crate) fn forget(&mut self, id: &str) -> Result<bool, AppError> {
        let transaction = self.connection.transaction().map_err(AppError::database)?;
        transaction
            .execute("DELETE FROM message_fts WHERE conversation_id = ?1", [id])
            .map_err(AppError::database)?;
        let removed = transaction
            .execute("DELETE FROM conversations WHERE id = ?1", [id])
            .map_err(AppError::database)?;
        transaction.commit().map_err(AppError::database)?;
        Ok(removed > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Veritas claim: storage/full-rebuild-is-idempotent
    #[test]
    fn full_rebuild_is_idempotent() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let storage = Storage::open(&directory.path().join("cass.sqlite3")).expect("open database");
        storage
            .rebuild_derived_search_state()
            .expect("first rebuild");
        storage
            .rebuild_derived_search_state()
            .expect("second rebuild");
        let counts = storage.counts().expect("counts");
        assert_eq!(counts.conversations, 0);
        assert_eq!(counts.messages, 0);
    }
}
