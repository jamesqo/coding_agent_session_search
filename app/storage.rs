use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;
use std::time::Duration;

use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;

use crate::AppError;
use crate::ingestion::Conversation;

const SCHEMA_VERSION: i64 = 6;
const SCHEMA: &str = r"
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
    updated_at INTEGER,
    source_fingerprint TEXT NOT NULL DEFAULT ''
);
CREATE TABLE IF NOT EXISTS messages (
    id TEXT PRIMARY KEY,
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    ordinal INTEGER NOT NULL,
    role TEXT NOT NULL,
    content TEXT NOT NULL,
    created_at INTEGER,
    fingerprint TEXT NOT NULL DEFAULT '',
    UNIQUE (conversation_id, ordinal)
);
CREATE TABLE IF NOT EXISTS message_embeddings (
    message_id TEXT PRIMARY KEY REFERENCES messages(id) ON DELETE CASCADE,
    generation TEXT NOT NULL DEFAULT '',
    dimensions INTEGER NOT NULL,
    norm REAL NOT NULL,
    vector BLOB NOT NULL
);
CREATE VIRTUAL TABLE IF NOT EXISTS message_fts USING fts5(
    content,
    message_id UNINDEXED,
    conversation_id UNINDEXED,
    tokenize = 'unicode61'
);
CREATE TABLE IF NOT EXISTS tombstones (
    provider TEXT NOT NULL,
    conversation_id TEXT NOT NULL,
    forgotten_at INTEGER NOT NULL,
    PRIMARY KEY (provider, conversation_id)
);
CREATE TABLE IF NOT EXISTS source_checkpoints (
    provider TEXT NOT NULL,
    source_path TEXT NOT NULL,
    size_bytes INTEGER NOT NULL,
    modified_ns INTEGER NOT NULL,
    PRIMARY KEY (provider, source_path)
);
";

pub(crate) struct Storage {
    connection: Connection,
    writer_active: bool,
    defer_search_updates: bool,
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

#[cfg(feature = "semantic")]
pub(crate) struct SearchableMessage {
    pub(crate) id: String,
    pub(crate) content: String,
}

#[cfg(feature = "semantic")]
pub(crate) struct EmbeddingWrite<'a> {
    pub(crate) message_id: &'a str,
    pub(crate) vector: &'a [u8],
    pub(crate) norm: f32,
}

#[cfg(feature = "semantic")]
pub(crate) struct SemanticVectors {
    pub(crate) message_ids: Vec<String>,
    pub(crate) values: Vec<u8>,
    pub(crate) norms: Vec<f32>,
    pub(crate) dimensions: usize,
}

#[derive(Default)]
pub(crate) struct ConversationChange {
    pub(crate) unchanged: bool,
    pub(crate) tombstoned: bool,
    pub(crate) changed_message_ids: Vec<String>,
    pub(crate) removed_messages: u64,
}

impl Storage {
    pub(crate) fn open(path: &Path) -> Result<Self, AppError> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(AppError::io)?;
        }
        let connection = Connection::open(path).map_err(AppError::database)?;
        initialize(&connection)?;
        Ok(Self {
            connection,
            writer_active: false,
            defer_search_updates: false,
        })
    }

    pub(crate) fn open_writer(path: &Path) -> Result<Self, AppError> {
        let mut storage = Self::open(path)?;
        storage
            .connection
            .busy_timeout(Duration::ZERO)
            .map_err(AppError::database)?;
        storage
            .connection
            .execute_batch("BEGIN IMMEDIATE")
            .map_err(AppError::database)?;
        storage.writer_active = true;
        Ok(storage)
    }

    pub(crate) fn commit_writer(&mut self) -> Result<(), AppError> {
        if self.writer_active {
            self.connection
                .execute_batch("COMMIT")
                .map_err(AppError::database)?;
            self.writer_active = false;
            self.connection
                .execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
                .map_err(AppError::database)?;
        }
        Ok(())
    }

    pub(crate) fn checkpoint_writer(&mut self) -> Result<(), AppError> {
        self.require_writer()?;
        self.connection
            .execute_batch("COMMIT")
            .map_err(AppError::database)?;
        self.writer_active = false;
        self.connection
            .execute_batch("PRAGMA wal_checkpoint(PASSIVE); BEGIN IMMEDIATE")
            .map_err(AppError::database)?;
        self.writer_active = true;
        Ok(())
    }

    pub(crate) fn supports_ingestion_checkpoints(&self) -> bool {
        !self.defer_search_updates
    }

    pub(crate) fn defer_search_updates(&mut self) -> Result<(), AppError> {
        self.require_writer()?;
        self.defer_search_updates = true;
        Ok(())
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

    #[cfg(feature = "semantic")]
    pub(crate) fn embedding_count(&self, generation: &str) -> Result<u64, AppError> {
        let count: i64 = self
            .connection
            .query_row(
                "SELECT count(*) FROM message_embeddings WHERE generation = ?1",
                [generation],
                |row| row.get(0),
            )
            .map_err(AppError::database)?;
        u64::try_from(count).map_err(|_| AppError::internal("negative embedding count"))
    }

    pub(crate) fn replace_conversation(
        &mut self,
        conversation: &Conversation,
    ) -> Result<ConversationChange, AppError> {
        self.require_writer()?;
        let tombstoned = self
            .connection
            .query_row(
                "SELECT 1 FROM tombstones WHERE provider = ?1 AND conversation_id = ?2",
                params![conversation.provider, conversation.id],
                |_| Ok(()),
            )
            .optional()
            .map_err(AppError::database)?
            .is_some();
        if tombstoned {
            return Ok(ConversationChange {
                tombstoned: true,
                ..ConversationChange::default()
            });
        }

        let source_fingerprint = conversation_fingerprint(conversation);
        let existing_source_fingerprint: Option<String> = self
            .connection
            .query_row(
                "SELECT source_fingerprint FROM conversations WHERE id = ?1",
                [&conversation.id],
                |row| row.get(0),
            )
            .optional()
            .map_err(AppError::database)?;
        if existing_source_fingerprint.as_deref() == Some(source_fingerprint.as_str()) {
            return Ok(ConversationChange {
                unchanged: true,
                ..ConversationChange::default()
            });
        }

        let existing_messages = self.message_fingerprints(&conversation.id)?;
        self.connection
            .execute(
                "INSERT INTO conversations(
                    id, provider, source_path, title, created_at, updated_at, source_fingerprint
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(id) DO UPDATE SET
                    provider = excluded.provider,
                    source_path = excluded.source_path,
                    title = excluded.title,
                    created_at = excluded.created_at,
                    updated_at = excluded.updated_at,
                    source_fingerprint = excluded.source_fingerprint",
                params![
                    conversation.id,
                    conversation.provider,
                    conversation.source_path.to_string_lossy(),
                    conversation.title,
                    conversation.created_at,
                    conversation.updated_at,
                    source_fingerprint,
                ],
            )
            .map_err(AppError::database)?;

        let (changed_message_ids, removed_messages) =
            self.reconcile_messages(conversation, &existing_messages)?;
        Ok(ConversationChange {
            changed_message_ids,
            removed_messages,
            ..ConversationChange::default()
        })
    }

    pub(crate) fn source_checkpoint_matches(
        &self,
        provider: &str,
        source_path: &str,
        size_bytes: i64,
        modified_ns: i64,
    ) -> Result<bool, AppError> {
        Ok(self
            .connection
            .query_row(
                "SELECT 1 FROM source_checkpoints
                  WHERE provider = ?1 AND source_path = ?2
                    AND size_bytes = ?3 AND modified_ns = ?4",
                params![provider, source_path, size_bytes, modified_ns],
                |_| Ok(()),
            )
            .optional()
            .map_err(AppError::database)?
            .is_some())
    }

    pub(crate) fn record_source_checkpoint(
        &mut self,
        provider: &str,
        source_path: &str,
        size_bytes: i64,
        modified_ns: i64,
    ) -> Result<(), AppError> {
        self.require_writer()?;
        self.connection
            .execute(
                "INSERT INTO source_checkpoints(provider, source_path, size_bytes, modified_ns)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(provider, source_path) DO UPDATE SET
                    size_bytes = excluded.size_bytes,
                    modified_ns = excluded.modified_ns",
                params![provider, source_path, size_bytes, modified_ns],
            )
            .map_err(AppError::database)?;
        Ok(())
    }

    fn reconcile_messages(
        &mut self,
        conversation: &Conversation,
        existing_messages: &BTreeMap<String, String>,
    ) -> Result<(Vec<String>, u64), AppError> {
        let mut changed = Vec::new();
        let mut upsert_message = self
            .connection
            .prepare_cached(
                "INSERT INTO messages(
                    id, conversation_id, ordinal, role, content, created_at, fingerprint
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(id) DO UPDATE SET
                    conversation_id = excluded.conversation_id,
                    ordinal = excluded.ordinal,
                    role = excluded.role,
                    content = excluded.content,
                    created_at = excluded.created_at,
                    fingerprint = excluded.fingerprint",
            )
            .map_err(AppError::database)?;
        let mut delete_fts = self
            .connection
            .prepare_cached("DELETE FROM message_fts WHERE message_id = ?1")
            .map_err(AppError::database)?;
        let mut insert_fts = self
            .connection
            .prepare_cached(
                "INSERT INTO message_fts(content, message_id, conversation_id)
                 VALUES (?1, ?2, ?3)",
            )
            .map_err(AppError::database)?;
        let mut delete_embedding = self
            .connection
            .prepare_cached("DELETE FROM message_embeddings WHERE message_id = ?1")
            .map_err(AppError::database)?;
        let mut delete_message = self
            .connection
            .prepare_cached("DELETE FROM messages WHERE id = ?1")
            .map_err(AppError::database)?;

        let incoming_ids = conversation
            .messages
            .iter()
            .map(|message| message.id.as_str())
            .collect::<BTreeSet<_>>();
        let removed: Vec<String> = existing_messages
            .keys()
            .filter(|id| !incoming_ids.contains(id.as_str()))
            .cloned()
            .collect();
        for id in &removed {
            if !self.defer_search_updates {
                delete_fts.execute([id]).map_err(AppError::database)?;
            }
            delete_message.execute([id]).map_err(AppError::database)?;
        }

        for message in &conversation.messages {
            let fingerprint = message_fingerprint(message);
            if existing_messages.get(&message.id) == Some(&fingerprint) {
                continue;
            }
            upsert_message
                .execute(params![
                    message.id,
                    conversation.id,
                    message.ordinal,
                    message.role,
                    message.content,
                    message.created_at,
                    fingerprint,
                ])
                .map_err(AppError::database)?;
            if !self.defer_search_updates {
                delete_fts
                    .execute([&message.id])
                    .map_err(AppError::database)?;
                insert_fts
                    .execute(params![message.content, message.id, conversation.id])
                    .map_err(AppError::database)?;
                delete_embedding
                    .execute([&message.id])
                    .map_err(AppError::database)?;
            }
            changed.push(message.id.clone());
        }
        let removed = u64::try_from(removed.len())
            .map_err(|_| AppError::internal("too many removed messages"))?;
        Ok((changed, removed))
    }

    pub(crate) fn rebuild_derived_search_state(&mut self) -> Result<(), AppError> {
        self.connection
            .execute_batch(
                "DELETE FROM message_fts;
                 DELETE FROM message_embeddings;
                 INSERT INTO message_fts(content, message_id, conversation_id)
                 SELECT content, id, conversation_id FROM messages;",
            )
            .map_err(AppError::database)?;
        self.defer_search_updates = false;
        Ok(())
    }

    pub(crate) fn purge_missing_sources(
        &mut self,
        provider: &str,
        observed_paths: &BTreeSet<String>,
    ) -> Result<u64, AppError> {
        self.require_writer()?;
        let mut statement = self
            .connection
            .prepare("SELECT id, source_path FROM conversations WHERE provider = ?1")
            .map_err(AppError::database)?;
        let rows = statement
            .query_map([provider], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(AppError::database)?;
        let existing = rows
            .collect::<Result<Vec<_>, _>>()
            .map_err(AppError::database)?;
        drop(statement);
        let mut removed = 0_u64;
        for (id, source_path) in existing {
            if observed_paths.contains(&source_path) {
                continue;
            }
            self.connection
                .execute("DELETE FROM message_fts WHERE conversation_id = ?1", [&id])
                .map_err(AppError::database)?;
            self.connection
                .execute("DELETE FROM conversations WHERE id = ?1", [&id])
                .map_err(AppError::database)?;
            removed += 1;
        }
        let mut statement = self
            .connection
            .prepare("SELECT source_path FROM source_checkpoints WHERE provider = ?1")
            .map_err(AppError::database)?;
        let checkpoint_paths = statement
            .query_map([provider], |row| row.get::<_, String>(0))
            .map_err(AppError::database)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(AppError::database)?;
        drop(statement);
        for source_path in checkpoint_paths {
            if !observed_paths.contains(&source_path) {
                self.connection
                    .execute(
                        "DELETE FROM source_checkpoints
                          WHERE provider = ?1 AND source_path = ?2",
                        params![provider, source_path],
                    )
                    .map_err(AppError::database)?;
            }
        }
        Ok(removed)
    }

    pub(crate) fn remove_source(
        &mut self,
        provider: &str,
        source_path: &str,
    ) -> Result<bool, AppError> {
        self.require_writer()?;
        let id: Option<String> = self
            .connection
            .query_row(
                "SELECT id FROM conversations WHERE provider = ?1 AND source_path = ?2",
                params![provider, source_path],
                |row| row.get(0),
            )
            .optional()
            .map_err(AppError::database)?;
        let Some(id) = id else {
            return Ok(false);
        };
        self.connection
            .execute("DELETE FROM message_fts WHERE conversation_id = ?1", [&id])
            .map_err(AppError::database)?;
        self.connection
            .execute("DELETE FROM conversations WHERE id = ?1", [&id])
            .map_err(AppError::database)?;
        Ok(true)
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

    #[cfg(feature = "semantic")]
    pub(crate) fn messages_needing_embeddings(
        &self,
        generation: &str,
    ) -> Result<Vec<SearchableMessage>, AppError> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT messages.id, messages.content
                   FROM messages
                   LEFT JOIN message_embeddings
                     ON message_embeddings.message_id = messages.id
                    AND message_embeddings.generation = ?1
                  WHERE message_embeddings.message_id IS NULL
                  ORDER BY messages.id",
            )
            .map_err(AppError::database)?;
        let rows = statement
            .query_map([generation], |row| {
                Ok(SearchableMessage {
                    id: row.get(0)?,
                    content: row.get(1)?,
                })
            })
            .map_err(AppError::database)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(AppError::database)
    }

    #[cfg(feature = "semantic")]
    pub(crate) fn replace_embeddings(
        &mut self,
        generation: &str,
        embeddings: &[EmbeddingWrite<'_>],
    ) -> Result<(), AppError> {
        let mut upsert = self
            .connection
            .prepare_cached(
                "INSERT INTO message_embeddings(
                    message_id, generation, dimensions, norm, vector
                 ) VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(message_id) DO UPDATE SET
                    generation = excluded.generation,
                    dimensions = excluded.dimensions,
                    norm = excluded.norm,
                    vector = excluded.vector",
            )
            .map_err(AppError::database)?;
        for embedding in embeddings {
            let dimensions = i64::try_from(embedding.vector.len())
                .map_err(|_| AppError::internal("embedding has too many dimensions"))?;
            upsert
                .execute(params![
                    embedding.message_id,
                    generation,
                    dimensions,
                    embedding.norm,
                    embedding.vector,
                ])
                .map_err(AppError::database)?;
        }
        Ok(())
    }

    #[cfg(feature = "semantic")]
    pub(crate) fn semantic_vectors(
        &self,
        generation: &str,
        provider: Option<&str>,
        days: Option<u32>,
    ) -> Result<SemanticVectors, AppError> {
        let cutoff = cutoff_timestamp(days)?;
        let mut statement = self
            .connection
            .prepare(
                "SELECT m.id, e.dimensions, e.norm, e.vector
                   FROM message_embeddings e
                   JOIN messages m ON m.id = e.message_id
                   JOIN conversations c ON c.id = m.conversation_id
                  WHERE e.generation = ?1
                    AND (?2 IS NULL OR c.provider = ?2)
                    AND (?3 IS NULL OR m.created_at >= ?3)
                  ORDER BY m.id",
            )
            .map_err(AppError::database)?;
        let mut rows = statement
            .query(params![generation, provider, cutoff])
            .map_err(AppError::database)?;
        let mut result = SemanticVectors {
            message_ids: Vec::new(),
            values: Vec::new(),
            norms: Vec::new(),
            dimensions: 0,
        };
        while let Some(row) = rows.next().map_err(AppError::database)? {
            let dimensions: i64 = row.get(1).map_err(AppError::database)?;
            let dimensions = usize::try_from(dimensions)
                .map_err(|_| AppError::database_data("negative embedding dimensions"))?;
            let norm: f32 = row.get(2).map_err(AppError::database)?;
            let vector: Vec<u8> = row.get(3).map_err(AppError::database)?;
            validate_quantized_vector(dimensions, norm, &vector)
                .map_err(AppError::database_data)?;
            if result.dimensions == 0 {
                result.dimensions = dimensions;
            } else if result.dimensions != dimensions {
                return Err(AppError::database_data(
                    "embedding rows have inconsistent dimensions",
                ));
            }
            result
                .message_ids
                .push(row.get(0).map_err(AppError::database)?);
            result.norms.push(norm);
            result.values.extend(vector);
        }
        Ok(result)
    }

    #[cfg(feature = "semantic")]
    pub(crate) fn search_hits(&self, message_ids: &[&str]) -> Result<Vec<SearchHit>, AppError> {
        let mut statement = self
            .connection
            .prepare_cached(
                "SELECT m.id, m.conversation_id, c.provider, m.role, m.content
                   FROM messages m
                   JOIN conversations c ON c.id = m.conversation_id
                  WHERE m.id = ?1",
            )
            .map_err(AppError::database)?;
        let mut hits = Vec::with_capacity(message_ids.len());
        for message_id in message_ids {
            let hit = statement
                .query_row([message_id], |row| {
                    Ok(SearchHit {
                        id: row.get(0)?,
                        conversation_id: row.get(1)?,
                        provider: row.get(2)?,
                        role: row.get(3)?,
                        content: row.get(4)?,
                        lexical_score: None,
                        semantic_score: None,
                        fusion_score: 0.0,
                        rerank_score: None,
                    })
                })
                .optional()
                .map_err(AppError::database)?;
            let hit = hit.ok_or_else(|| {
                AppError::database_data("semantic index references a missing message")
            })?;
            hits.push(hit);
        }
        Ok(hits)
    }

    #[cfg(feature = "semantic")]
    pub(crate) fn invalidate_embedding_generation(
        &mut self,
        generation: &str,
    ) -> Result<u64, AppError> {
        self.require_writer()?;
        let removed = self
            .connection
            .execute(
                "DELETE FROM message_embeddings WHERE generation <> ?1",
                [generation],
            )
            .map_err(AppError::database)?;
        u64::try_from(removed).map_err(|_| AppError::internal("too many stale embeddings"))
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
        let provider: Option<String> = transaction
            .query_row(
                "SELECT provider FROM conversations WHERE id = ?1",
                [id],
                |row| row.get(0),
            )
            .optional()
            .map_err(AppError::database)?;
        let Some(provider) = provider else {
            transaction.commit().map_err(AppError::database)?;
            return Ok(false);
        };
        transaction
            .execute(
                "INSERT INTO tombstones(provider, conversation_id, forgotten_at)
                 VALUES (?1, ?2, unixepoch())
                 ON CONFLICT(provider, conversation_id) DO NOTHING",
                params![provider, id],
            )
            .map_err(AppError::database)?;
        transaction
            .execute("DELETE FROM message_fts WHERE conversation_id = ?1", [id])
            .map_err(AppError::database)?;
        let removed = transaction
            .execute("DELETE FROM conversations WHERE id = ?1", [id])
            .map_err(AppError::database)?;
        transaction.commit().map_err(AppError::database)?;
        Ok(removed > 0)
    }

    fn message_fingerprints(
        &self,
        conversation_id: &str,
    ) -> Result<BTreeMap<String, String>, AppError> {
        let mut statement = self
            .connection
            .prepare("SELECT id, fingerprint FROM messages WHERE conversation_id = ?1")
            .map_err(AppError::database)?;
        let rows = statement
            .query_map([conversation_id], |row| Ok((row.get(0)?, row.get(1)?)))
            .map_err(AppError::database)?;
        rows.collect::<Result<BTreeMap<_, _>, _>>()
            .map_err(AppError::database)
    }

    fn require_writer(&self) -> Result<(), AppError> {
        if self.writer_active {
            Ok(())
        } else {
            Err(AppError::internal(
                "index mutation requires an active writer",
            ))
        }
    }
}

impl Drop for Storage {
    fn drop(&mut self) {
        if self.writer_active {
            let _ = self.connection.execute_batch("ROLLBACK");
        }
    }
}

fn initialize(connection: &Connection) -> Result<(), AppError> {
    let version: i64 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(AppError::database)?;
    if version > SCHEMA_VERSION {
        return Err(AppError::schema(format!(
            "database schema version {version} is newer than supported version {SCHEMA_VERSION}"
        )));
    }

    connection
        .execute_batch("PRAGMA foreign_keys = ON; PRAGMA journal_mode = WAL;")
        .map_err(AppError::database)?;
    if version == SCHEMA_VERSION {
        return Ok(());
    }

    connection
        .pragma_update(None, "foreign_keys", false)
        .map_err(AppError::database)?;
    let transaction = connection
        .unchecked_transaction()
        .map_err(AppError::database)?;
    transaction
        .execute_batch(SCHEMA)
        .map_err(AppError::database)?;
    add_column_if_missing(
        &transaction,
        "conversations",
        "source_fingerprint",
        "TEXT NOT NULL DEFAULT ''",
    )?;
    add_column_if_missing(
        &transaction,
        "messages",
        "fingerprint",
        "TEXT NOT NULL DEFAULT ''",
    )?;
    add_column_if_missing(
        &transaction,
        "message_embeddings",
        "generation",
        "TEXT NOT NULL DEFAULT ''",
    )?;
    add_column_if_missing(
        &transaction,
        "message_embeddings",
        "norm",
        "REAL NOT NULL DEFAULT 0",
    )?;
    purge_unsupported_providers(&transaction)?;
    if !provider_schema_is_current(&transaction)? {
        rebuild_provider_schema(&transaction)?;
    }
    transaction
        .pragma_update(None, "user_version", SCHEMA_VERSION)
        .map_err(AppError::database)?;
    transaction.commit().map_err(AppError::database)?;
    connection
        .pragma_update(None, "foreign_keys", true)
        .map_err(AppError::database)
}

fn provider_schema_is_current(connection: &Connection) -> Result<bool, AppError> {
    let sql: String = connection
        .query_row(
            "SELECT sql FROM sqlite_schema WHERE type = 'table' AND name = 'conversations'",
            [],
            |row| row.get(0),
        )
        .map_err(AppError::database)?;
    Ok(sql.contains("'opencode'") && sql.contains("'github-copilot'") && sql.contains("'pi'"))
}

fn rebuild_provider_schema(connection: &Connection) -> Result<(), AppError> {
    connection
        .execute_batch(
            "CREATE TABLE conversations_next (
                id TEXT PRIMARY KEY,
                provider TEXT NOT NULL CHECK (provider IN (
                    'claude-code', 'codex', 'opencode', 'github-copilot', 'hermes', 'pi'
                )),
                source_path TEXT NOT NULL UNIQUE,
                title TEXT, created_at INTEGER, updated_at INTEGER,
                source_fingerprint TEXT NOT NULL DEFAULT ''
             );
             CREATE TABLE messages_next (
                id TEXT PRIMARY KEY,
                conversation_id TEXT NOT NULL REFERENCES conversations_next(id) ON DELETE CASCADE,
                ordinal INTEGER NOT NULL, role TEXT NOT NULL, content TEXT NOT NULL,
                created_at INTEGER, fingerprint TEXT NOT NULL DEFAULT '',
                UNIQUE(conversation_id, ordinal)
             );
             CREATE TABLE message_embeddings_next (
                message_id TEXT PRIMARY KEY REFERENCES messages_next(id) ON DELETE CASCADE,
                generation TEXT NOT NULL DEFAULT '',
                dimensions INTEGER NOT NULL, norm REAL NOT NULL, vector BLOB NOT NULL
             );
             INSERT INTO conversations_next
                SELECT id, provider, source_path, title, created_at, updated_at, source_fingerprint
                FROM conversations;
             INSERT INTO messages_next
                SELECT id, conversation_id, ordinal, role, content, created_at, fingerprint
                FROM messages;
             INSERT INTO message_embeddings_next(
                message_id, generation, dimensions, norm, vector
             ) SELECT message_id, generation, dimensions, norm, vector FROM message_embeddings;
             DROP TABLE message_fts;
             DROP TABLE message_embeddings;
             DROP TABLE messages;
             DROP TABLE conversations;
             ALTER TABLE conversations_next RENAME TO conversations;
             ALTER TABLE messages_next RENAME TO messages;
             ALTER TABLE message_embeddings_next RENAME TO message_embeddings;
             CREATE VIRTUAL TABLE message_fts USING fts5(
                content, message_id UNINDEXED, conversation_id UNINDEXED,
                tokenize = 'unicode61'
             );
             INSERT INTO message_fts(content, message_id, conversation_id)
                SELECT content, id, conversation_id FROM messages;",
        )
        .map_err(AppError::database)
}

fn add_column_if_missing(
    connection: &Connection,
    table: &str,
    column: &str,
    declaration: &str,
) -> Result<(), AppError> {
    let mut statement = connection
        .prepare(&format!("PRAGMA table_info({table})"))
        .map_err(AppError::database)?;
    let names = statement
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(AppError::database)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(AppError::database)?;
    if !names.iter().any(|name| name == column) {
        connection
            .execute_batch(&format!(
                "ALTER TABLE {table} ADD COLUMN {column} {declaration}"
            ))
            .map_err(AppError::database)?;
    }
    Ok(())
}

fn conversation_fingerprint(conversation: &Conversation) -> String {
    let mut hasher = blake3::Hasher::new();
    update_hash(&mut hasher, conversation.provider);
    update_hash(&mut hasher, &conversation.id);
    update_hash(
        &mut hasher,
        conversation.title.as_deref().unwrap_or_default(),
    );
    update_hash(
        &mut hasher,
        &conversation.created_at.unwrap_or_default().to_string(),
    );
    update_hash(
        &mut hasher,
        &conversation.updated_at.unwrap_or_default().to_string(),
    );
    for message in &conversation.messages {
        update_hash(&mut hasher, &message_fingerprint(message));
    }
    hasher.finalize().to_hex().to_string()
}

fn message_fingerprint(message: &crate::ingestion::NormalizedMessage) -> String {
    let mut hasher = blake3::Hasher::new();
    update_hash(&mut hasher, &message.id);
    update_hash(&mut hasher, &message.ordinal.to_string());
    update_hash(&mut hasher, &message.role);
    update_hash(&mut hasher, &message.content);
    update_hash(
        &mut hasher,
        &message.created_at.unwrap_or_default().to_string(),
    );
    hasher.finalize().to_hex().to_string()
}

fn update_hash(hasher: &mut blake3::Hasher, value: &str) {
    hasher.update(&(value.len() as u64).to_le_bytes());
    hasher.update(value.as_bytes());
}

fn purge_unsupported_providers(connection: &Connection) -> Result<(), AppError> {
    connection
        .execute_batch(
            "DELETE FROM message_fts
              WHERE conversation_id IN (
                    SELECT id FROM conversations
                     WHERE provider NOT IN (
                        'claude-code', 'codex', 'opencode', 'github-copilot', 'hermes', 'pi'
                     )
              );
             DELETE FROM message_embeddings
              WHERE message_id IN (
                    SELECT messages.id
                      FROM messages
                      JOIN conversations ON conversations.id = messages.conversation_id
                     WHERE conversations.provider NOT IN (
                        'claude-code', 'codex', 'opencode', 'github-copilot', 'hermes', 'pi'
                     )
              );
             DELETE FROM messages
              WHERE conversation_id IN (
                    SELECT id FROM conversations
                     WHERE provider NOT IN (
                        'claude-code', 'codex', 'opencode', 'github-copilot', 'hermes', 'pi'
                     )
              );
             DELETE FROM conversations
              WHERE provider NOT IN (
                'claude-code', 'codex', 'opencode', 'github-copilot', 'hermes', 'pi'
              );",
        )
        .map_err(AppError::database)
}

#[cfg(any(feature = "semantic", test))]
fn validate_quantized_vector(
    dimensions: usize,
    norm: f32,
    bytes: &[u8],
) -> Result<(), &'static str> {
    if bytes.len() != dimensions {
        return Err("embedding blob length does not match its dimensions");
    }
    if !norm.is_finite() || norm < 0.0 {
        return Err("embedding norm must be finite and nonnegative");
    }
    Ok(())
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
    use std::path::PathBuf;
    use veritas_test_macros as veritas;

    fn conversation(content: &str) -> Conversation {
        Conversation {
            id: "session-1".to_owned(),
            provider: "codex",
            source_path: PathBuf::from("/tmp/session-1.jsonl"),
            title: Some("session".to_owned()),
            created_at: Some(1),
            updated_at: Some(2),
            messages: vec![crate::ingestion::NormalizedMessage {
                id: "message-1".to_owned(),
                ordinal: 0,
                role: "user".to_owned(),
                content: content.to_owned(),
                created_at: Some(1),
            }],
        }
    }

    #[veritas::claims("storage/full-rebuild-is-idempotent")]
    #[test]
    fn full_rebuild_is_idempotent() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let mut storage =
            Storage::open(&directory.path().join("cass.sqlite3")).expect("open database");
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
    fn writer_checkpoint_survives_a_later_rollback() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("cass.sqlite3");
        let mut writer = Storage::open_writer(&path).expect("writer");
        writer
            .replace_conversation(&conversation("durable needle"))
            .expect("durable mutation");
        writer
            .record_source_checkpoint("codex", "/tmp/session-1.jsonl", 100, 200)
            .expect("durable source checkpoint");
        writer.checkpoint_writer().expect("checkpoint batch");
        writer
            .replace_conversation(&conversation("rolled back text"))
            .expect("later mutation");
        drop(writer);

        let storage = Storage::open_existing(&path).expect("reopen database");
        assert_eq!(
            storage
                .search("durable", 10, None, None)
                .expect("search committed batch")
                .len(),
            1
        );
        assert!(
            storage
                .source_checkpoint_matches("codex", "/tmp/session-1.jsonl", 100, 200)
                .expect("read checkpoint")
        );
    }

    #[test]
    fn full_rebuild_defers_per_message_search_writes() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("cass.sqlite3");
        let mut storage = Storage::open_writer(&path).expect("writer");
        storage
            .defer_search_updates()
            .expect("defer derived writes");
        storage
            .replace_conversation(&conversation("deferred needle"))
            .expect("insert conversation");
        assert_eq!(
            storage
                .connection
                .query_row("SELECT count(*) FROM message_fts", [], |row| row
                    .get::<_, i64>(0))
                .expect("FTS count before rebuild"),
            0
        );

        storage
            .rebuild_derived_search_state()
            .expect("bulk rebuild");
        assert_eq!(
            storage
                .search("needle", 10, None, None)
                .expect("search rebuilt FTS")
                .len(),
            1
        );
    }

    #[test]
    fn quantized_embedding_blobs_are_validated() {
        let vector = [129_u8, 0, 127];
        assert_eq!(validate_quantized_vector(3, 181.0, &vector), Ok(()));
        assert!(validate_quantized_vector(2, 181.0, &vector).is_err());
        assert!(validate_quantized_vector(3, f32::NAN, &vector).is_err());
    }

    #[veritas::claims("storage/supported-schema-migrates")]
    #[test]
    fn supported_schema_migrates_once_and_preserves_rows() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("cass.sqlite3");
        let connection = Connection::open(&path).expect("seed database");
        connection
            .execute_batch(
                "CREATE TABLE conversations (
                    id TEXT PRIMARY KEY, provider TEXT NOT NULL, source_path TEXT NOT NULL UNIQUE,
                    title TEXT, created_at INTEGER, updated_at INTEGER
                 );
                 CREATE TABLE messages (
                    id TEXT PRIMARY KEY,
                    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
                    ordinal INTEGER NOT NULL, role TEXT NOT NULL, content TEXT NOT NULL,
                    created_at INTEGER, UNIQUE(conversation_id, ordinal)
                 );
                 CREATE TABLE message_embeddings (
                    message_id TEXT PRIMARY KEY REFERENCES messages(id) ON DELETE CASCADE,
                    dimensions INTEGER NOT NULL, vector BLOB NOT NULL
                 );
                 CREATE VIRTUAL TABLE message_fts USING fts5(
                    content, message_id UNINDEXED, conversation_id UNINDEXED
                 );
                 INSERT INTO conversations(id, provider, source_path)
                    VALUES ('session-1', 'codex', '/tmp/session-1.jsonl');
                 INSERT INTO messages(id, conversation_id, ordinal, role, content)
                    VALUES ('message-1', 'session-1', 0, 'user', 'preserved');
                 INSERT INTO message_fts(content, message_id, conversation_id)
                    VALUES ('preserved', 'message-1', 'session-1');
                 INSERT INTO message_embeddings(message_id, dimensions, vector)
                    VALUES ('message-1', 1, X'0000803F');
                 PRAGMA user_version = 1;",
            )
            .expect("seed older schema");
        drop(connection);

        let storage = Storage::open(&path).expect("migrate database");
        let counts = storage.counts().expect("counts");
        assert_eq!(counts.messages, 1);
        assert_eq!(counts.embeddings, 1);
        assert_eq!(
            storage
                .connection
                .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .expect("schema version"),
            SCHEMA_VERSION
        );
        drop(storage);
        Storage::open(&path).expect("idempotent second open");
    }

    #[veritas::claims("storage/newer-schema-is-rejected")]
    #[test]
    fn newer_schema_is_rejected_without_rewriting_version() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("cass.sqlite3");
        let connection = Connection::open(&path).expect("seed database");
        connection
            .pragma_update(None, "user_version", SCHEMA_VERSION + 1)
            .expect("newer version");
        drop(connection);

        let error = Storage::open(&path).err().expect("newer schema rejected");
        assert_eq!(error.error.kind, "schema-incompatible");
        let connection = Connection::open(&path).expect("reopen seed");
        assert_eq!(
            connection
                .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .expect("schema version"),
            SCHEMA_VERSION + 1
        );
    }

    #[veritas::claims("indexing/concurrent-writer-is-rejected")]
    #[test]
    fn concurrent_writer_is_rejected_and_first_writer_can_commit() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("cass.sqlite3");
        let mut first = Storage::open_writer(&path).expect("first writer");
        let error = Storage::open_writer(&path)
            .err()
            .expect("second writer rejected");
        assert_eq!(error.error.kind, "index-busy");
        first.commit_writer().expect("first writer commits");
        Storage::open_writer(&path).expect("writer available after commit");
    }

    #[veritas::claims(
        "indexing/unchanged-source-is-skipped",
        "indexing/only-changed-messages-refresh"
    )]
    #[test]
    fn conversation_reconciliation_writes_only_changed_messages() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("cass.sqlite3");
        let mut storage = Storage::open_writer(&path).expect("writer");
        let first = storage
            .replace_conversation(&conversation("first"))
            .expect("initial insert");
        assert_eq!(first.changed_message_ids, ["message-1"]);
        #[cfg(feature = "semantic")]
        storage
            .replace_embeddings(
                "generation-a",
                &[EmbeddingWrite {
                    message_id: "message-1",
                    vector: &[127, 0],
                    norm: 127.0,
                }],
            )
            .expect("seed embedding");
        let unchanged = storage
            .replace_conversation(&conversation("first"))
            .expect("unchanged refresh");
        assert!(unchanged.unchanged);
        assert_eq!(unchanged.changed_message_ids, Vec::<String>::new());
        #[cfg(feature = "semantic")]
        assert!(
            storage
                .messages_needing_embeddings("generation-a")
                .expect("embedding selection")
                .is_empty()
        );
        let changed = storage
            .replace_conversation(&conversation("second"))
            .expect("changed refresh");
        assert_eq!(changed.changed_message_ids, ["message-1"]);
        #[cfg(feature = "semantic")]
        assert_eq!(
            storage
                .messages_needing_embeddings("generation-a")
                .expect("embedding selection")
                .iter()
                .map(|message| message.id.as_str())
                .collect::<Vec<_>>(),
            ["message-1"]
        );
    }

    #[veritas::claims("indexing/only-changed-messages-refresh")]
    #[test]
    fn conversation_reconciliation_replaces_a_changed_message_id_at_the_same_ordinal() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("cass.sqlite3");
        let mut storage = Storage::open_writer(&path).expect("writer");
        storage
            .replace_conversation(&conversation("first"))
            .expect("initial insert");

        let mut replacement = conversation("second");
        replacement.messages[0].id = "message-2".to_owned();
        let change = storage
            .replace_conversation(&replacement)
            .expect("replace message identity");

        assert_eq!(change.changed_message_ids, ["message-2"]);
        assert_eq!(change.removed_messages, 1);
        assert_eq!(storage.counts().expect("counts").messages, 1);
    }

    #[cfg(feature = "semantic")]
    #[veritas::claims("semantic/stale-embedding-generation-invalidated")]
    #[test]
    fn stale_embedding_generation_is_excluded_and_replaced() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("cass.sqlite3");
        let mut writer = Storage::open_writer(&path).expect("writer");
        writer
            .replace_conversation(&conversation("generation proof"))
            .expect("insert conversation");
        writer
            .replace_embeddings(
                "old-generation",
                &[EmbeddingWrite {
                    message_id: "message-1",
                    vector: &[127, 0],
                    norm: 127.0,
                }],
            )
            .expect("old embedding");
        writer.commit_writer().expect("commit old generation");
        drop(writer);

        let storage = Storage::open(&path).expect("reader");
        assert_eq!(storage.embedding_count("new-generation").expect("count"), 0);
        assert_eq!(
            storage
                .semantic_vectors("new-generation", None, None)
                .expect("semantic documents")
                .message_ids
                .len(),
            0
        );
        drop(storage);

        let mut writer = Storage::open_writer(&path).expect("replacement writer");
        assert_eq!(
            writer
                .invalidate_embedding_generation("new-generation")
                .expect("invalidate"),
            1
        );
        assert_eq!(
            writer
                .messages_needing_embeddings("new-generation")
                .expect("re-embedding selection")
                .iter()
                .map(|message| message.id.as_str())
                .collect::<Vec<_>>(),
            ["message-1"]
        );
        writer
            .replace_embeddings(
                "new-generation",
                &[EmbeddingWrite {
                    message_id: "message-1",
                    vector: &[0, 127],
                    norm: 127.0,
                }],
            )
            .expect("new embedding");
        writer.commit_writer().expect("commit new generation");
        assert_eq!(writer.embedding_count("new-generation").expect("count"), 1);
        let vectors = writer
            .semantic_vectors("new-generation", None, None)
            .expect("stored quantized vectors");
        assert_eq!(vectors.message_ids, ["message-1"]);
        assert_eq!(vectors.values, [0, 127]);
        assert_eq!(vectors.norms, [127.0]);
        assert_eq!(vectors.dimensions, 2);
        assert_eq!(
            writer
                .search_hits(&["message-1"])
                .expect("hydrate semantic hit")[0]
                .content,
            "generation proof"
        );
    }

    #[veritas::claims("storage/forget-persists-through-indexing")]
    #[test]
    fn tombstone_prevents_reinsertion() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("cass.sqlite3");
        let mut writer = Storage::open_writer(&path).expect("writer");
        writer
            .replace_conversation(&conversation("remember me"))
            .expect("insert");
        writer.commit_writer().expect("commit");
        drop(writer);

        let mut storage = Storage::open(&path).expect("open database");
        assert!(storage.forget("session-1").expect("forget"));
        drop(storage);

        let mut writer = Storage::open_writer(&path).expect("second writer");
        let change = writer
            .replace_conversation(&conversation("remember me"))
            .expect("tombstone check");
        assert!(change.tombstoned);
        writer.commit_writer().expect("commit");
        assert_eq!(writer.counts().expect("counts").conversations, 0);
    }
}
