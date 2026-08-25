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
    provider TEXT NOT NULL CHECK (
        provider IN (
            'claude-code', 'codex', 'opencode', 'github-copilot', 'hermes', 'pi'
        )
    ),
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
CREATE TABLE IF NOT EXISTS message_embeddings (
    message_id TEXT PRIMARY KEY REFERENCES messages(id) ON DELETE CASCADE,
    dimensions INTEGER NOT NULL,
    vector BLOB NOT NULL
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
    pub(crate) embeddings: u64,
}

#[derive(Debug, Serialize)]
pub(crate) struct SearchHit {
    pub(crate) id: String,
    pub(crate) conversation_id: String,
    pub(crate) provider: String,
    pub(crate) role: String,
    pub(crate) content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) lexical_score: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) semantic_score: Option<f32>,
    pub(crate) fusion_score: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) rerank_score: Option<f32>,
}

#[derive(Debug, Serialize)]
pub(crate) struct Message {
    id: String,
    ordinal: i64,
    role: String,
    content: String,
    created_at: Option<i64>,
}

pub(crate) struct SearchableMessage {
    pub(crate) id: String,
    pub(crate) content: String,
}

pub(crate) struct SemanticDocument {
    pub(crate) hit: SearchHit,
    pub(crate) vector: Vec<f32>,
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
        let embeddings: i64 = self
            .connection
            .query_row("SELECT count(*) FROM message_embeddings", [], |row| {
                row.get(0)
            })
            .map_err(AppError::database)?;
        Ok(Counts {
            conversations: u64::try_from(conversations)
                .map_err(|_| AppError::internal("negative conversation count"))?,
            messages: u64::try_from(messages)
                .map_err(|_| AppError::internal("negative message count"))?,
            embeddings: u64::try_from(embeddings)
                .map_err(|_| AppError::internal("negative embedding count"))?,
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
                 DELETE FROM message_embeddings;
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
        days: Option<u32>,
    ) -> Result<Vec<SearchHit>, AppError> {
        if query.trim().is_empty() {
            return Err(AppError::usage("search query must not be empty"));
        }
        let limit = i64::try_from(limit.min(1_000)).unwrap_or(1_000);
        let cutoff = cutoff_timestamp(days)?;
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
                    AND (?3 IS NULL OR m.created_at >= ?3)
                  ORDER BY lexical_score, m.id
                  LIMIT ?4",
            )
            .map_err(AppError::database)?;
        let rows = statement
            .query_map(params![query, provider, cutoff, limit], |row| {
                Ok(SearchHit {
                    id: row.get(0)?,
                    conversation_id: row.get(1)?,
                    provider: row.get(2)?,
                    role: row.get(3)?,
                    content: row.get(4)?,
                    lexical_score: Some(row.get(5)?),
                    semantic_score: None,
                    fusion_score: 0.0,
                    rerank_score: None,
                })
            })
            .map_err(AppError::database)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(AppError::database)
    }

    pub(crate) fn searchable_messages(&self) -> Result<Vec<SearchableMessage>, AppError> {
        let mut statement = self
            .connection
            .prepare("SELECT id, content FROM messages ORDER BY id")
            .map_err(AppError::database)?;
        let rows = statement
            .query_map([], |row| {
                Ok(SearchableMessage {
                    id: row.get(0)?,
                    content: row.get(1)?,
                })
            })
            .map_err(AppError::database)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(AppError::database)
    }

    pub(crate) fn replace_embeddings(
        &mut self,
        embeddings: &[(&str, &[f32])],
    ) -> Result<(), AppError> {
        let transaction = self.connection.transaction().map_err(AppError::database)?;
        transaction
            .execute("DELETE FROM message_embeddings", [])
            .map_err(AppError::database)?;
        for (message_id, vector) in embeddings {
            let dimensions = i64::try_from(vector.len())
                .map_err(|_| AppError::internal("embedding has too many dimensions"))?;
            transaction
                .execute(
                    "INSERT INTO message_embeddings(message_id, dimensions, vector)
                     VALUES (?1, ?2, ?3)",
                    params![message_id, dimensions, encode_vector(vector)],
                )
                .map_err(AppError::database)?;
        }
        transaction.commit().map_err(AppError::database)
    }

    pub(crate) fn semantic_documents(
        &self,
        provider: Option<&str>,
        days: Option<u32>,
    ) -> Result<Vec<SemanticDocument>, AppError> {
        let cutoff = cutoff_timestamp(days)?;
        let mut statement = self
            .connection
            .prepare(
                "SELECT m.id, m.conversation_id, c.provider, m.role, m.content,
                        e.dimensions, e.vector
                   FROM message_embeddings e
                   JOIN messages m ON m.id = e.message_id
                   JOIN conversations c ON c.id = m.conversation_id
                  WHERE (?1 IS NULL OR c.provider = ?1)
                    AND (?2 IS NULL OR m.created_at >= ?2)
                  ORDER BY m.id",
            )
            .map_err(AppError::database)?;
        let rows = statement
            .query_map(params![provider, cutoff], |row| {
                let dimensions: i64 = row.get(5)?;
                let bytes: Vec<u8> = row.get(6)?;
                let vector = decode_vector(dimensions, &bytes).map_err(|message| {
                    rusqlite::Error::FromSqlConversionFailure(
                        bytes.len(),
                        rusqlite::types::Type::Blob,
                        Box::new(std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            message,
                        )),
                    )
                })?;
                Ok(SemanticDocument {
                    hit: SearchHit {
                        id: row.get(0)?,
                        conversation_id: row.get(1)?,
                        provider: row.get(2)?,
                        role: row.get(3)?,
                        content: row.get(4)?,
                        lexical_score: None,
                        semantic_score: None,
                        fusion_score: 0.0,
                        rerank_score: None,
                    },
                    vector,
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

fn encode_vector(vector: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(std::mem::size_of_val(vector));
    for value in vector {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes
}

fn decode_vector(dimensions: i64, bytes: &[u8]) -> Result<Vec<f32>, &'static str> {
    let dimensions = usize::try_from(dimensions).map_err(|_| "negative embedding dimensions")?;
    if bytes.len() != dimensions.saturating_mul(size_of::<f32>()) {
        return Err("embedding blob length does not match its dimensions");
    }
    let (chunks, remainder) = bytes.as_chunks::<4>();
    if !remainder.is_empty() {
        return Err("embedding blob contains a partial value");
    }
    Ok(chunks
        .iter()
        .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect())
}

fn cutoff_timestamp(days: Option<u32>) -> Result<Option<i64>, AppError> {
    let Some(days) = days else {
        return Ok(None);
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|error| {
            AppError::internal(format!("system clock precedes Unix epoch: {error}"))
        })?;
    let age = std::time::Duration::from_secs(u64::from(days).saturating_mul(86_400));
    let cutoff = now.saturating_sub(age).as_millis();
    i64::try_from(cutoff)
        .map(Some)
        .map_err(|_| AppError::internal("timestamp exceeds SQLite range"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use veritas_test_macros as veritas;

    #[veritas::claims("storage/full-rebuild-is-idempotent")]
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
        assert_eq!(counts.embeddings, 0);
    }

    #[test]
    fn embedding_blobs_round_trip() {
        let vector = [-1.5, 0.0, 2.25];
        let bytes = encode_vector(&vector);
        assert_eq!(decode_vector(3, &bytes), Ok(vector.to_vec()));
        assert!(decode_vector(2, &bytes).is_err());
    }
}
