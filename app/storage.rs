use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;
use std::time::Duration;

use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::AppError;
use crate::ingestion::Conversation;

mod schema;
mod semantic_chunks;

use schema::{SCHEMA_VERSION, initialize};
#[cfg(test)]
use semantic_chunks::validate_quantized_vector;
use semantic_chunks::{
    MISSING_CREATED_AT, SEMANTIC_CHUNK_ROWS, decode_f32_blob, decode_i64_blob, provider_code,
    rebuild_semantic_chunk,
};

const FTS_BULK_REBUILD_PERCENT: u64 = 90;

pub(crate) struct Storage {
    connection: Connection,
    writer_active: bool,
    defer_search_updates: bool,
    transaction_base_messages: u64,
}

#[derive(Debug, Eq, PartialEq)]
enum FtsRefreshStrategy {
    None,
    Incremental,
    Bulk,
    Deferred,
}

pub(crate) struct Counts {
    pub(crate) conversations: u64,
    pub(crate) messages: u64,
    pub(crate) searchable_messages: u64,
    pub(crate) embeddings: u64,
}

pub(crate) struct StatusSnapshot {
    pub(crate) counts: Counts,
    pub(crate) current_embeddings: u64,
    pub(crate) derived_clean: bool,
    pub(crate) exact_semantic_coverage: bool,
}

#[derive(Debug, Deserialize, Serialize)]
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) origins: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) federated_score: Option<f64>,
}

#[derive(Debug, Deserialize, Serialize)]
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

pub(crate) struct EmbeddingWrite<'a> {
    pub(crate) message_id: &'a str,
    pub(crate) vector: &'a [u8],
    pub(crate) norm: f32,
}

pub(crate) struct SemanticChunk {
    pub(crate) message_rowids: Vec<i64>,
    pub(crate) values: Vec<u8>,
    pub(crate) norms: Vec<f32>,
    pub(crate) eligible: Vec<bool>,
    pub(crate) dimensions: usize,
}

pub(crate) struct SemanticChunks {
    pub(crate) chunks: Vec<SemanticChunk>,
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
        connection
            .execute_batch(
                "CREATE TEMP TABLE IF NOT EXISTS pending_fts_messages (
                    message_id TEXT PRIMARY KEY
                 ) WITHOUT ROWID;
                 CREATE TEMP TABLE IF NOT EXISTS dirty_semantic_chunks (
                    chunk_id INTEGER PRIMARY KEY,
                    generation TEXT
                 ) WITHOUT ROWID;",
            )
            .map_err(AppError::database)?;
        Ok(Self {
            connection,
            writer_active: false,
            defer_search_updates: false,
            transaction_base_messages: 0,
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
        storage.transaction_base_messages = storage.message_count()?;
        Ok(storage)
    }

    pub(crate) fn commit_writer(&mut self) -> Result<(), AppError> {
        if self.writer_active {
            let threshold = self.measured_fts_bulk_threshold()?;
            self.finalize_pending_fts_updates(threshold)?;
            self.finalize_semantic_chunks()?;
            self.connection
                .execute_batch("COMMIT")
                .map_err(AppError::database)?;
            self.writer_active = false;
        }
        Ok(())
    }

    pub(crate) fn truncate_wal(&self) -> Result<(), AppError> {
        self.connection
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
            .map_err(AppError::database)
    }

    pub(crate) fn checkpoint_writer(&mut self) -> Result<(), AppError> {
        self.commit_and_restart_writer("PRAGMA wal_checkpoint(PASSIVE); BEGIN IMMEDIATE")
    }

    pub(crate) fn commit_and_continue_writer(&mut self) -> Result<(), AppError> {
        self.commit_and_restart_writer("BEGIN IMMEDIATE")
    }

    fn commit_and_restart_writer(&mut self, continuation: &str) -> Result<(), AppError> {
        self.require_writer()?;
        let threshold = self.measured_fts_bulk_threshold()?;
        self.finalize_pending_fts_updates(threshold)?;
        self.finalize_semantic_chunks()?;
        self.connection
            .execute_batch("COMMIT")
            .map_err(AppError::database)?;
        self.writer_active = false;
        self.connection
            .execute_batch(continuation)
            .map_err(AppError::database)?;
        self.writer_active = true;
        self.transaction_base_messages = self.message_count()?;
        Ok(())
    }

    pub(crate) fn begin_provider_scan(&self) -> Result<(), AppError> {
        self.require_writer()?;
        self.connection
            .execute_batch("SAVEPOINT cass_provider_scan")
            .map_err(AppError::database)
    }

    pub(crate) fn finish_provider_scan(&self) -> Result<(), AppError> {
        self.connection
            .execute_batch("RELEASE cass_provider_scan")
            .map_err(AppError::database)
    }

    pub(crate) fn rollback_provider_scan(&self) -> Result<(), AppError> {
        self.connection
            .execute_batch(
                "ROLLBACK TO cass_provider_scan;
                 RELEASE cass_provider_scan;",
            )
            .map_err(AppError::database)
    }

    fn measured_fts_bulk_threshold(&self) -> Result<u64, AppError> {
        let messages = self.message_count()?.max(self.transaction_base_messages);
        Ok(messages
            .saturating_mul(FTS_BULK_REBUILD_PERCENT)
            .saturating_add(99)
            / 100)
    }

    fn message_count(&self) -> Result<u64, AppError> {
        let messages: i64 = self
            .connection
            .query_row("SELECT count(*) FROM messages", [], |row| row.get(0))
            .map_err(AppError::database)?;
        u64::try_from(messages).map_err(|_| AppError::internal("negative message count"))
    }

    fn finalize_pending_fts_updates(
        &mut self,
        bulk_rebuild_threshold: u64,
    ) -> Result<FtsRefreshStrategy, AppError> {
        self.require_writer()?;
        if self.defer_search_updates {
            return Ok(FtsRefreshStrategy::Deferred);
        }
        let changed: i64 = self
            .connection
            .query_row("SELECT count(*) FROM pending_fts_messages", [], |row| {
                row.get(0)
            })
            .map_err(AppError::database)?;
        let changed = u64::try_from(changed)
            .map_err(|_| AppError::internal("negative pending FTS message count"))?;
        if changed == 0 {
            return Ok(FtsRefreshStrategy::None);
        }
        let strategy = if changed >= bulk_rebuild_threshold.max(1) {
            self.rebuild_fts(false)?;
            FtsRefreshStrategy::Bulk
        } else {
            self.connection
                .execute_batch(
                    "DELETE FROM message_fts
                      WHERE message_id IN (SELECT message_id FROM pending_fts_messages);
                     INSERT INTO message_fts(content, message_id, conversation_id)
                     SELECT COALESCE(search_projection, content), id, conversation_id
                       FROM messages
                      WHERE id IN (SELECT message_id FROM pending_fts_messages)
                        AND COALESCE(search_projection, content) <> '';
                     DELETE FROM pending_fts_messages;",
                )
                .map_err(AppError::database)?;
            FtsRefreshStrategy::Incremental
        };
        Ok(strategy)
    }

    fn stage_semantic_message(
        &self,
        message_id: &str,
        generation: Option<&str>,
    ) -> Result<(), AppError> {
        self.connection
            .execute(
                "INSERT INTO dirty_semantic_chunks(chunk_id, generation)
                 SELECT (m.rowid - 1) / ?2, COALESCE(?3, e.generation)
                   FROM messages m
                   LEFT JOIN message_embeddings e ON e.message_id = m.id
                  WHERE m.id = ?1
                 ON CONFLICT(chunk_id) DO UPDATE SET
                    generation = COALESCE(excluded.generation, generation)",
                params![message_id, SEMANTIC_CHUNK_ROWS, generation],
            )
            .map_err(AppError::database)?;
        Ok(())
    }

    fn stage_semantic_conversation(&self, conversation_id: &str) -> Result<(), AppError> {
        self.connection
            .execute(
                "INSERT INTO dirty_semantic_chunks(chunk_id, generation)
                 SELECT (m.rowid - 1) / ?2, min(e.generation)
                   FROM messages m
                   LEFT JOIN message_embeddings e ON e.message_id = m.id
                  WHERE m.conversation_id = ?1
                  GROUP BY (m.rowid - 1) / ?2
                 ON CONFLICT(chunk_id) DO UPDATE SET
                    generation = COALESCE(excluded.generation, generation)",
                params![conversation_id, SEMANTIC_CHUNK_ROWS],
            )
            .map_err(AppError::database)?;
        Ok(())
    }

    fn finalize_semantic_chunks(&self) -> Result<(), AppError> {
        let mut statement = self
            .connection
            .prepare("SELECT chunk_id, generation FROM dirty_semantic_chunks ORDER BY chunk_id")
            .map_err(AppError::database)?;
        let dirty = statement
            .query_map([], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, Option<String>>(1)?))
            })
            .map_err(AppError::database)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(AppError::database)?;
        drop(statement);
        for (chunk_id, generation) in dirty {
            rebuild_semantic_chunk(&self.connection, chunk_id, generation.as_deref())?;
        }
        self.connection
            .execute("DELETE FROM dirty_semantic_chunks", [])
            .map_err(AppError::database)?;
        Ok(())
    }

    pub(crate) fn defer_search_updates(&mut self) -> Result<(), AppError> {
        self.require_writer()?;
        self.defer_search_updates = true;
        Ok(())
    }

    pub(crate) fn mark_derived_search_dirty(&self) -> Result<(), AppError> {
        self.require_writer()?;
        self.connection
            .execute(
                "UPDATE derived_state SET search_dirty = 1 WHERE singleton = 1",
                [],
            )
            .map_err(AppError::database)?;
        Ok(())
    }

    pub(crate) fn derived_search_is_dirty(&self) -> Result<bool, AppError> {
        self.connection
            .query_row(
                "SELECT search_dirty FROM derived_state WHERE singleton = 1",
                [],
                |row| row.get::<_, bool>(0),
            )
            .map_err(AppError::database)
    }

    pub(crate) fn open_existing(path: &Path) -> Result<Self, AppError> {
        if !path.is_file() {
            return Err(AppError::missing_database(path));
        }
        Self::open(path)
    }

    pub(crate) fn status_snapshot(
        path: &Path,
        generation: &str,
    ) -> Result<StatusSnapshot, AppError> {
        let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(AppError::database)?;
        let version: i64 = connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .map_err(AppError::database)?;
        if version > SCHEMA_VERSION {
            return Err(AppError::schema(format!(
                "database schema version {version} is newer than supported version {SCHEMA_VERSION}"
            )));
        }
        if version < SCHEMA_VERSION {
            return Ok(StatusSnapshot {
                counts: Counts {
                    conversations: readonly_table_count(&connection, "conversations")?,
                    messages: readonly_table_count(&connection, "messages")?,
                    searchable_messages: readonly_table_count(&connection, "messages")?,
                    embeddings: readonly_table_count(&connection, "message_embeddings")?,
                },
                current_embeddings: 0,
                derived_clean: false,
                exact_semantic_coverage: false,
            });
        }
        let storage = Self {
            connection,
            writer_active: false,
            defer_search_updates: false,
            transaction_base_messages: 0,
        };
        Ok(StatusSnapshot {
            counts: storage.counts()?,
            current_embeddings: storage.embedding_count(generation)?,
            derived_clean: !storage.derived_search_is_dirty()?,
            exact_semantic_coverage: storage.semantic_index_is_ready(generation)?,
        })
    }

    pub(crate) fn has_messages(&self) -> Result<bool, AppError> {
        self.connection
            .query_row("SELECT EXISTS(SELECT 1 FROM messages LIMIT 1)", [], |row| {
                row.get(0)
            })
            .map_err(AppError::database)
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
        let searchable_messages: i64 = self
            .connection
            .query_row(
                "SELECT count(*) FROM messages
                  WHERE COALESCE(search_projection, content) <> ''",
                [],
                |row| row.get(0),
            )
            .map_err(AppError::database)?;
        Ok(Counts {
            conversations: u64::try_from(conversations)
                .map_err(|_| AppError::internal("negative conversation count"))?,
            messages: u64::try_from(messages)
                .map_err(|_| AppError::internal("negative message count"))?,
            searchable_messages: u64::try_from(searchable_messages)
                .map_err(|_| AppError::internal("negative searchable message count"))?,
            embeddings: u64::try_from(embeddings)
                .map_err(|_| AppError::internal("negative embedding count"))?,
        })
    }

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

    pub(crate) fn semantic_coverage_is_complete(&self, generation: &str) -> Result<bool, AppError> {
        self.connection
            .query_row(
                "SELECT NOT EXISTS (
                    SELECT 1
                      FROM messages m
                      LEFT JOIN message_embeddings e
                        ON e.message_id = m.id AND e.generation = ?1
                     WHERE COALESCE(m.search_projection, m.content) <> ''
                       AND e.message_id IS NULL
                    UNION ALL
                    SELECT 1
                      FROM message_embeddings e
                      JOIN messages m ON m.id = e.message_id
                     WHERE e.generation = ?1
                       AND COALESCE(m.search_projection, m.content) = ''
                 )",
                [generation],
                |row| row.get(0),
            )
            .map_err(AppError::database)
    }

    pub(crate) fn semantic_index_is_ready(&self, generation: &str) -> Result<bool, AppError> {
        let ready_generation = self
            .connection
            .query_row(
                "SELECT semantic_ready_generation
                   FROM derived_state
                  WHERE singleton = 1",
                [],
                |row| row.get::<_, Option<String>>(0),
            )
            .map_err(AppError::database)?;
        if ready_generation.as_deref() == Some(generation) {
            return self
                .connection
                .query_row(
                    "SELECT
                        (SELECT count(*) FROM messages
                          WHERE COALESCE(search_projection, content) <> '') =
                        (SELECT COALESCE(sum(vector_count), 0)
                           FROM semantic_chunks WHERE generation = ?1)",
                    [generation],
                    |row| row.get(0),
                )
                .map_err(AppError::database);
        }
        self.connection
            .query_row(
                "SELECT NOT EXISTS (
                    SELECT 1
                      FROM messages
                     WHERE COALESCE(search_projection, content) <> ''
                     LIMIT 1
                 )",
                [],
                |row| row.get(0),
            )
            .map_err(AppError::database)
    }

    pub(crate) fn mark_semantic_index_ready(&mut self, generation: &str) -> Result<(), AppError> {
        self.require_writer()?;
        if !self.semantic_coverage_is_complete(generation)? {
            return Err(AppError::search_not_ready(
                "semantic embedding coverage is incomplete",
            ));
        }
        self.finalize_semantic_chunks()?;
        let expected: i64 = self
            .connection
            .query_row(
                "SELECT count(*) FROM messages
                  WHERE COALESCE(search_projection, content) <> ''",
                [],
                |row| row.get(0),
            )
            .map_err(AppError::database)?;
        let packed: i64 = self
            .connection
            .query_row(
                "SELECT COALESCE(sum(vector_count), 0)
                   FROM semantic_chunks WHERE generation = ?1",
                [generation],
                |row| row.get(0),
            )
            .map_err(AppError::database)?;
        if packed != expected {
            return Err(AppError::search_not_ready(
                "packed semantic index coverage is incomplete",
            ));
        }
        self.connection
            .execute(
                "UPDATE derived_state
                    SET semantic_ready_generation = ?1
                  WHERE singleton = 1",
                [generation],
            )
            .map_err(AppError::database)?;
        Ok(())
    }

    fn mark_semantic_index_incomplete(&self) -> Result<(), AppError> {
        self.require_writer()?;
        self.connection
            .execute(
                "UPDATE derived_state
                    SET semantic_ready_generation = NULL
                  WHERE singleton = 1",
                [],
            )
            .map_err(AppError::database)?;
        Ok(())
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
        if existing_source_fingerprint.is_some() && !self.defer_search_updates {
            self.stage_semantic_conversation(&conversation.id)?;
            self.mark_semantic_index_incomplete()?;
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
        if !changed_message_ids.is_empty() {
            self.mark_semantic_index_incomplete()?;
        }
        if self.defer_search_updates && (!changed_message_ids.is_empty() || removed_messages != 0) {
            self.mark_derived_search_dirty()?;
        }
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
                    id, conversation_id, ordinal, role, content, search_projection,
                    created_at, fingerprint
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                 ON CONFLICT(id) DO UPDATE SET
                    conversation_id = excluded.conversation_id,
                    ordinal = excluded.ordinal,
                    role = excluded.role,
                    content = excluded.content,
                    search_projection = excluded.search_projection,
                    created_at = excluded.created_at,
                    fingerprint = excluded.fingerprint",
            )
            .map_err(AppError::database)?;
        let mut stage_fts = self
            .connection
            .prepare_cached("INSERT OR IGNORE INTO pending_fts_messages(message_id) VALUES (?1)")
            .map_err(AppError::database)?;
        let mut delete_embedding = self
            .connection
            .prepare_cached("DELETE FROM message_embeddings WHERE message_id = ?1")
            .map_err(AppError::database)?;
        let mut queue_embedding = self
            .connection
            .prepare_cached("INSERT OR IGNORE INTO pending_embeddings(message_id) VALUES (?1)")
            .map_err(AppError::database)?;
        let mut unqueue_embedding = self
            .connection
            .prepare_cached("DELETE FROM pending_embeddings WHERE message_id = ?1")
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
                stage_fts.execute([id]).map_err(AppError::database)?;
                self.stage_semantic_message(id, None)?;
            }
            delete_message.execute([id]).map_err(AppError::database)?;
        }

        for message in &conversation.messages {
            let fingerprint = message_fingerprint(message);
            if existing_messages.get(&message.id) == Some(&fingerprint) {
                continue;
            }
            if !self.defer_search_updates {
                self.stage_semantic_message(&message.id, None)?;
            }
            upsert_message
                .execute(params![
                    message.id,
                    conversation.id,
                    message.ordinal,
                    message.role,
                    message.content,
                    message.search_projection,
                    message.created_at,
                    fingerprint,
                ])
                .map_err(AppError::database)?;
            if !self.defer_search_updates {
                stage_fts
                    .execute([&message.id])
                    .map_err(AppError::database)?;
                delete_embedding
                    .execute([&message.id])
                    .map_err(AppError::database)?;
                if message_search_text(message).is_empty() {
                    unqueue_embedding
                        .execute([&message.id])
                        .map_err(AppError::database)?;
                } else {
                    queue_embedding
                        .execute([&message.id])
                        .map_err(AppError::database)?;
                }
                self.stage_semantic_message(&message.id, None)?;
            }
            changed.push(message.id.clone());
        }
        let removed = u64::try_from(removed.len())
            .map_err(|_| AppError::internal("too many removed messages"))?;
        Ok((changed, removed))
    }

    pub(crate) fn rebuild_derived_search_state(&mut self) -> Result<(), AppError> {
        self.rebuild_fts(true)?;
        self.connection
            .execute_batch(
                "DELETE FROM message_embeddings;
                 DELETE FROM semantic_chunks;
                 DELETE FROM dirty_semantic_chunks;
                 DELETE FROM pending_embeddings;
                 INSERT INTO pending_embeddings(message_id)
                    SELECT id FROM messages
                     WHERE COALESCE(search_projection, content) <> '';
                 UPDATE derived_state
                    SET search_dirty = 0,
                        semantic_ready_generation = NULL
                  WHERE singleton = 1;",
            )
            .map_err(AppError::database)?;
        self.defer_search_updates = false;
        Ok(())
    }

    fn rebuild_fts(&mut self, optimize: bool) -> Result<(), AppError> {
        self.connection
            .execute_batch(
                "DELETE FROM message_fts;
                 INSERT INTO message_fts(content, message_id, conversation_id)
                 SELECT COALESCE(search_projection, content), id, conversation_id
                   FROM messages
                  WHERE COALESCE(search_projection, content) <> '';
                 DELETE FROM pending_fts_messages;",
            )
            .map_err(AppError::database)?;
        if optimize {
            self.connection
                .execute(
                    "INSERT INTO message_fts(message_fts) VALUES ('optimize')",
                    [],
                )
                .map_err(AppError::database)?;
        }
        Ok(())
    }

    pub(crate) fn purge_missing_sources(
        &mut self,
        provider: &str,
        observed_paths: &BTreeSet<String>,
        authoritative_roots: &[std::path::PathBuf],
        cutoff_modified_ns: Option<i64>,
    ) -> Result<u64, AppError> {
        self.require_writer()?;
        let mut statement = self
            .connection
            .prepare(
                "SELECT conversations.id, conversations.source_path,
                        source_checkpoints.modified_ns
                   FROM conversations
                   LEFT JOIN source_checkpoints
                     ON source_checkpoints.provider = conversations.provider
                    AND source_checkpoints.source_path = conversations.source_path
                  WHERE conversations.provider = ?1",
            )
            .map_err(AppError::database)?;
        let rows = statement
            .query_map([provider], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                ))
            })
            .map_err(AppError::database)?;
        let existing = rows
            .collect::<Result<Vec<_>, _>>()
            .map_err(AppError::database)?;
        drop(statement);
        let mut removed = 0_u64;
        for (id, source_path, modified_ns) in existing {
            if observed_paths.contains(&source_path)
                || !source_is_in_reconciliation_scope(
                    &source_path,
                    modified_ns,
                    authoritative_roots,
                    cutoff_modified_ns,
                )
            {
                continue;
            }
            if !self.defer_search_updates {
                self.stage_conversation_fts(&id)?;
                self.stage_semantic_conversation(&id)?;
                self.mark_semantic_index_incomplete()?;
            }
            self.connection
                .execute("DELETE FROM conversations WHERE id = ?1", [&id])
                .map_err(AppError::database)?;
            if self.defer_search_updates {
                self.mark_derived_search_dirty()?;
            }
            removed += 1;
        }
        let mut statement = self
            .connection
            .prepare(
                "SELECT source_path, modified_ns
                   FROM source_checkpoints WHERE provider = ?1",
            )
            .map_err(AppError::database)?;
        let checkpoint_paths = statement
            .query_map([provider], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })
            .map_err(AppError::database)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(AppError::database)?;
        drop(statement);
        for (source_path, modified_ns) in checkpoint_paths {
            if !observed_paths.contains(&source_path)
                && source_is_in_reconciliation_scope(
                    &source_path,
                    Some(modified_ns),
                    authoritative_roots,
                    cutoff_modified_ns,
                )
            {
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
        if !self.defer_search_updates {
            self.stage_conversation_fts(&id)?;
            self.stage_semantic_conversation(&id)?;
            self.mark_semantic_index_incomplete()?;
        }
        if self.defer_search_updates {
            self.mark_derived_search_dirty()?;
        }
        self.connection
            .execute("DELETE FROM conversations WHERE id = ?1", [&id])
            .map_err(AppError::database)?;
        Ok(true)
    }

    fn stage_conversation_fts(&self, conversation_id: &str) -> Result<(), AppError> {
        self.connection
            .execute(
                "INSERT OR IGNORE INTO pending_fts_messages(message_id)
                 SELECT id FROM messages WHERE conversation_id = ?1",
                [conversation_id],
            )
            .map_err(AppError::database)?;
        Ok(())
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
                    origins: Vec::new(),
                    federated_score: None,
                })
            })
            .map_err(AppError::database)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(AppError::database)
    }

    pub(crate) fn messages_needing_embeddings(&self) -> Result<Vec<SearchableMessage>, AppError> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT messages.id, COALESCE(messages.search_projection, messages.content)
                   FROM pending_embeddings
                   JOIN messages ON messages.id = pending_embeddings.message_id
                  ORDER BY pending_embeddings.message_id",
            )
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

    pub(crate) fn has_pending_embeddings(&self) -> Result<bool, AppError> {
        self.connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM pending_embeddings LIMIT 1)",
                [],
                |row| row.get(0),
            )
            .map_err(AppError::database)
    }

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
        let mut stage_chunk = self
            .connection
            .prepare_cached(
                "INSERT INTO dirty_semantic_chunks(chunk_id, generation)
                 SELECT (rowid - 1) / ?2, ?3 FROM messages WHERE id = ?1
                 ON CONFLICT(chunk_id) DO UPDATE SET generation = excluded.generation",
            )
            .map_err(AppError::database)?;
        let mut dequeue = self
            .connection
            .prepare_cached("DELETE FROM pending_embeddings WHERE message_id = ?1")
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
            stage_chunk
                .execute(params![
                    embedding.message_id,
                    SEMANTIC_CHUNK_ROWS,
                    generation
                ])
                .map_err(AppError::database)?;
            dequeue
                .execute([embedding.message_id])
                .map_err(AppError::database)?;
        }
        Ok(())
    }

    pub(crate) fn semantic_chunks(
        &self,
        generation: &str,
        provider: Option<&str>,
        days: Option<u32>,
    ) -> Result<SemanticChunks, AppError> {
        let cutoff = cutoff_timestamp(days)?;
        let requested_provider = provider.map(provider_code).transpose()?;
        let mut statement = self
            .connection
            .prepare(
                "SELECT dimensions, vector_count, message_rowids, norms,
                        providers, created_ats, vectors
                   FROM semantic_chunks
                  WHERE generation = ?1
                  ORDER BY chunk_id",
            )
            .map_err(AppError::database)?;
        let mut rows = statement.query([generation]).map_err(AppError::database)?;
        let mut result = SemanticChunks {
            chunks: Vec::new(),
            dimensions: 0,
        };
        while let Some(row) = rows.next().map_err(AppError::database)? {
            let dimensions: i64 = row.get(0).map_err(AppError::database)?;
            let dimensions = usize::try_from(dimensions)
                .map_err(|_| AppError::database_data("negative embedding dimensions"))?;
            let vector_count: i64 = row.get(1).map_err(AppError::database)?;
            let vector_count = usize::try_from(vector_count)
                .map_err(|_| AppError::database_data("negative semantic chunk size"))?;
            let message_rowids = decode_i64_blob(
                &row.get::<_, Vec<u8>>(2).map_err(AppError::database)?,
                vector_count,
                "semantic chunk rowid",
            )?;
            let norms = decode_f32_blob(
                &row.get::<_, Vec<u8>>(3).map_err(AppError::database)?,
                vector_count,
                "semantic chunk norm",
            )?;
            if norms.iter().any(|norm| !norm.is_finite() || *norm < 0.0) {
                return Err(AppError::database_data(
                    "semantic chunk contains an invalid norm",
                ));
            }
            let providers: Vec<u8> = row.get(4).map_err(AppError::database)?;
            if providers.len() != vector_count
                || providers.iter().any(|code| !matches!(code, 1 | 2))
            {
                return Err(AppError::database_data(
                    "semantic chunk contains invalid provider metadata",
                ));
            }
            let created_ats = decode_i64_blob(
                &row.get::<_, Vec<u8>>(5).map_err(AppError::database)?,
                vector_count,
                "semantic chunk timestamp",
            )?;
            let values: Vec<u8> = row.get(6).map_err(AppError::database)?;
            let expected_values = vector_count
                .checked_mul(dimensions)
                .ok_or_else(|| AppError::database_data("semantic chunk size overflows"))?;
            if values.len() != expected_values {
                return Err(AppError::database_data(
                    "semantic chunk vector bytes do not match its dimensions",
                ));
            }
            if result.dimensions == 0 {
                result.dimensions = dimensions;
            } else if result.dimensions != dimensions {
                return Err(AppError::database_data(
                    "embedding rows have inconsistent dimensions",
                ));
            }
            let eligible = providers
                .iter()
                .zip(&created_ats)
                .map(|(stored_provider, created_at)| {
                    requested_provider.is_none_or(|wanted| wanted == *stored_provider)
                        && cutoff.is_none_or(|minimum| {
                            *created_at != MISSING_CREATED_AT && *created_at >= minimum
                        })
                })
                .collect();
            result.chunks.push(SemanticChunk {
                message_rowids,
                values,
                norms,
                eligible,
                dimensions,
            });
        }
        Ok(result)
    }

    pub(crate) fn search_hits(&self, message_rowids: &[i64]) -> Result<Vec<SearchHit>, AppError> {
        let mut statement = self
            .connection
            .prepare_cached(
                "SELECT m.id, m.conversation_id, c.provider, m.role, m.content
                   FROM messages m
                   JOIN conversations c ON c.id = m.conversation_id
                  WHERE m.rowid = ?1",
            )
            .map_err(AppError::database)?;
        let mut hits = Vec::with_capacity(message_rowids.len());
        for message_rowid in message_rowids {
            let hit = statement
                .query_row([message_rowid], |row| {
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
                        origins: Vec::new(),
                        federated_score: None,
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

    pub(crate) fn search_documents(&self, message_ids: &[&str]) -> Result<Vec<String>, AppError> {
        let mut statement = self
            .connection
            .prepare_cached(
                "SELECT COALESCE(search_projection, content)
                   FROM messages
                  WHERE id = ?1",
            )
            .map_err(AppError::database)?;
        message_ids
            .iter()
            .map(|message_id| {
                statement
                    .query_row([message_id], |row| row.get(0))
                    .map_err(AppError::database)
            })
            .collect()
    }

    pub(crate) fn invalidate_embedding_generation(
        &mut self,
        generation: &str,
    ) -> Result<u64, AppError> {
        self.require_writer()?;
        let ready_generation = self
            .connection
            .query_row(
                "SELECT semantic_ready_generation FROM derived_state WHERE singleton = 1",
                [],
                |row| row.get::<_, Option<String>>(0),
            )
            .map_err(AppError::database)?;
        if ready_generation.as_deref() == Some(generation) && !self.has_pending_embeddings()? {
            return Ok(0);
        }
        self.mark_semantic_index_incomplete()?;
        let stored_generation = self
            .connection
            .query_row(
                "SELECT generation FROM message_embeddings LIMIT 1",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(AppError::database)?;
        if stored_generation
            .as_deref()
            .is_none_or(|stored| stored == generation)
        {
            return Ok(0);
        }
        let removed = self
            .connection
            .execute("DELETE FROM message_embeddings", [])
            .map_err(AppError::database)?;
        self.connection
            .execute_batch(
                "DELETE FROM semantic_chunks;
                 DELETE FROM dirty_semantic_chunks;
                 DELETE FROM pending_embeddings;
                 INSERT INTO pending_embeddings(message_id)
                    SELECT id FROM messages
                     WHERE COALESCE(search_projection, content) <> '';",
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
        let semantic_generation = transaction
            .query_row(
                "SELECT semantic_ready_generation FROM derived_state WHERE singleton = 1",
                [],
                |row| row.get::<_, Option<String>>(0),
            )
            .map_err(AppError::database)?;
        let mut statement = transaction
            .prepare(
                "SELECT DISTINCT (rowid - 1) / ?2
                   FROM messages WHERE conversation_id = ?1 ORDER BY 1",
            )
            .map_err(AppError::database)?;
        let semantic_chunk_ids = statement
            .query_map(params![id, SEMANTIC_CHUNK_ROWS], |row| row.get(0))
            .map_err(AppError::database)?
            .collect::<Result<Vec<i64>, _>>()
            .map_err(AppError::database)?;
        drop(statement);
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
        for chunk_id in semantic_chunk_ids {
            rebuild_semantic_chunk(&transaction, chunk_id, semantic_generation.as_deref())?;
        }
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

fn readonly_table_count(connection: &Connection, table: &str) -> Result<u64, AppError> {
    let exists: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type = 'table' AND name = ?1)",
            [table],
            |row| row.get(0),
        )
        .map_err(AppError::database)?;
    if !exists {
        return Ok(0);
    }
    let sql = format!("SELECT count(*) FROM {table}");
    let count: i64 = connection
        .query_row(&sql, [], |row| row.get(0))
        .map_err(AppError::database)?;
    u64::try_from(count).map_err(|_| AppError::internal("negative table count"))
}

fn source_is_in_reconciliation_scope(
    source_path: &str,
    modified_ns: Option<i64>,
    authoritative_roots: &[std::path::PathBuf],
    cutoff_modified_ns: Option<i64>,
) -> bool {
    let source_path = Path::new(source_path);
    if !authoritative_roots
        .iter()
        .any(|root| source_path.starts_with(root))
    {
        return false;
    }
    cutoff_modified_ns.is_none_or(|cutoff| modified_ns.is_some_and(|value| value >= cutoff))
}

impl Drop for Storage {
    fn drop(&mut self) {
        if self.writer_active {
            let _ = self.connection.execute_batch("ROLLBACK");
        }
    }
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
    match &message.search_projection {
        None => update_hash(&mut hasher, "canonical"),
        Some(projection) => {
            update_hash(&mut hasher, "projected");
            update_hash(&mut hasher, projection);
        }
    }
    update_hash(
        &mut hasher,
        &message.created_at.unwrap_or_default().to_string(),
    );
    hasher.finalize().to_hex().to_string()
}

fn message_search_text(message: &crate::ingestion::NormalizedMessage) -> &str {
    message
        .search_projection
        .as_deref()
        .unwrap_or(&message.content)
}

fn update_hash(hasher: &mut blake3::Hasher, value: &str) {
    hasher.update(&(value.len() as u64).to_le_bytes());
    hasher.update(value.as_bytes());
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
mod tests;
