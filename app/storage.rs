use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;
use std::time::Duration;

use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::AppError;
use crate::ingestion::Conversation;

const SCHEMA_VERSION: i64 = 11;
const FTS_BULK_REBUILD_PERCENT: u64 = 90;
const SEMANTIC_CHUNK_ROWS: i64 = 4_096;
const MISSING_CREATED_AT: i64 = i64::MIN;
const SCHEMA: &str = r"
CREATE TABLE IF NOT EXISTS conversations (
    id TEXT PRIMARY KEY,
    provider TEXT NOT NULL CHECK (
        provider IN (
            'claude-code', 'codex'
        )
    ),
    source_path TEXT NOT NULL UNIQUE,
    title TEXT,
    created_at INTEGER,
    updated_at INTEGER,
    source_fingerprint TEXT NOT NULL DEFAULT ''
);
CREATE TABLE IF NOT EXISTS messages (
    storage_id INTEGER PRIMARY KEY AUTOINCREMENT,
    id TEXT NOT NULL UNIQUE,
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    ordinal INTEGER NOT NULL,
    role TEXT NOT NULL,
    content TEXT NOT NULL,
    search_projection TEXT,
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
CREATE TABLE IF NOT EXISTS semantic_chunks (
    chunk_id INTEGER PRIMARY KEY CHECK (chunk_id >= 0),
    generation TEXT NOT NULL,
    dimensions INTEGER NOT NULL CHECK (dimensions > 0),
    vector_count INTEGER NOT NULL CHECK (vector_count > 0 AND vector_count <= 4096),
    message_rowids BLOB NOT NULL,
    norms BLOB NOT NULL,
    providers BLOB NOT NULL,
    created_ats BLOB NOT NULL,
    vectors BLOB NOT NULL
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
CREATE TABLE IF NOT EXISTS derived_state (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    search_dirty INTEGER NOT NULL CHECK (search_dirty IN (0, 1)),
    semantic_ready_generation TEXT
);
INSERT OR IGNORE INTO derived_state(singleton, search_dirty) VALUES (1, 1);
";

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

struct PackedSemanticChunk {
    dimensions: i64,
    vector_count: i64,
    message_rowids: Vec<u8>,
    norms: Vec<u8>,
    providers: Vec<u8>,
    created_ats: Vec<u8>,
    vectors: Vec<u8>,
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
            self.connection
                .execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
                .map_err(AppError::database)?;
        }
        Ok(())
    }

    pub(crate) fn checkpoint_writer(&mut self) -> Result<(), AppError> {
        self.require_writer()?;
        let threshold = self.measured_fts_bulk_threshold()?;
        self.finalize_pending_fts_updates(threshold)?;
        self.finalize_semantic_chunks()?;
        self.connection
            .execute_batch("COMMIT")
            .map_err(AppError::database)?;
        self.writer_active = false;
        self.connection
            .execute_batch("PRAGMA wal_checkpoint(PASSIVE); BEGIN IMMEDIATE")
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

    pub(crate) fn messages_needing_embeddings(
        &self,
        generation: &str,
    ) -> Result<Vec<SearchableMessage>, AppError> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT messages.id, COALESCE(messages.search_projection, messages.content)
                   FROM messages
                   LEFT JOIN message_embeddings
                     ON message_embeddings.message_id = messages.id
                    AND message_embeddings.generation = ?1
                  WHERE message_embeddings.message_id IS NULL
                    AND COALESCE(messages.search_projection, messages.content) <> ''
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
        self.mark_semantic_index_incomplete()?;
        let removed = self
            .connection
            .execute(
                "DELETE FROM message_embeddings
                  WHERE generation <> ?1
                     OR message_id IN (
                        SELECT id FROM messages
                         WHERE COALESCE(search_projection, content) = ''
                     )",
                [generation],
            )
            .map_err(AppError::database)?;
        if removed != 0 {
            self.connection
                .execute("DELETE FROM semantic_chunks", [])
                .map_err(AppError::database)?;
            self.connection
                .execute(
                    "INSERT INTO dirty_semantic_chunks(chunk_id, generation)
                     SELECT (m.rowid - 1) / ?2, min(e.generation)
                       FROM message_embeddings e
                       JOIN messages m ON m.id = e.message_id
                      WHERE e.generation = ?1
                      GROUP BY (m.rowid - 1) / ?2
                     ON CONFLICT(chunk_id) DO UPDATE SET
                        generation = excluded.generation",
                    params![generation, SEMANTIC_CHUNK_ROWS],
                )
                .map_err(AppError::database)?;
        }
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
    add_column_if_missing(&transaction, "messages", "search_projection", "TEXT")?;
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
    add_column_if_missing(
        &transaction,
        "derived_state",
        "semantic_ready_generation",
        "TEXT",
    )?;
    purge_unsupported_providers(&transaction)?;
    if version < 11 || !provider_schema_is_current(&transaction)? {
        rebuild_provider_schema(&transaction)?;
    }
    if version < 8 {
        transaction
            .execute_batch(
                "DELETE FROM message_fts;
                 DELETE FROM message_embeddings;
                 DELETE FROM source_checkpoints;
                 UPDATE derived_state SET search_dirty = 1 WHERE singleton = 1;",
            )
            .map_err(AppError::database)?;
    }
    if version < 10 {
        backfill_semantic_readiness(&transaction)?;
    }
    if version < 11 {
        rebuild_all_semantic_chunks(&transaction)?;
    }
    transaction
        .pragma_update(None, "user_version", SCHEMA_VERSION)
        .map_err(AppError::database)?;
    transaction.commit().map_err(AppError::database)?;
    connection
        .pragma_update(None, "foreign_keys", true)
        .map_err(AppError::database)
}

fn backfill_semantic_readiness(connection: &Connection) -> Result<(), AppError> {
    connection
        .execute_batch(
            "UPDATE derived_state
                SET semantic_ready_generation = CASE
                    WHEN (SELECT count(DISTINCT generation)
                            FROM message_embeddings) = 1
                     AND NOT EXISTS (
                        SELECT 1
                          FROM messages m
                          LEFT JOIN message_embeddings e ON e.message_id = m.id
                         WHERE COALESCE(m.search_projection, m.content) <> ''
                           AND e.message_id IS NULL
                        UNION ALL
                        SELECT 1
                          FROM message_embeddings e
                          JOIN messages m ON m.id = e.message_id
                         WHERE COALESCE(m.search_projection, m.content) = ''
                     )
                    THEN (SELECT min(generation) FROM message_embeddings)
                    ELSE NULL
                END
              WHERE singleton = 1;",
        )
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
    Ok(sql.contains("'claude-code'")
        && sql.contains("'codex'")
        && !sql.contains("'opencode'")
        && !sql.contains("'github-copilot'")
        && !sql.contains("'hermes'")
        && !sql.contains("'pi'"))
}

fn rebuild_provider_schema(connection: &Connection) -> Result<(), AppError> {
    connection
        .execute_batch(
            "CREATE TABLE conversations_next (
                id TEXT PRIMARY KEY,
                provider TEXT NOT NULL CHECK (provider IN (
                    'claude-code', 'codex'
                )),
                source_path TEXT NOT NULL UNIQUE,
                title TEXT, created_at INTEGER, updated_at INTEGER,
                source_fingerprint TEXT NOT NULL DEFAULT ''
             );
             CREATE TABLE messages_next (
                storage_id INTEGER PRIMARY KEY AUTOINCREMENT,
                id TEXT NOT NULL UNIQUE,
                conversation_id TEXT NOT NULL REFERENCES conversations_next(id) ON DELETE CASCADE,
                ordinal INTEGER NOT NULL, role TEXT NOT NULL, content TEXT NOT NULL,
                search_projection TEXT, created_at INTEGER,
                fingerprint TEXT NOT NULL DEFAULT '',
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
                SELECT rowid, id, conversation_id, ordinal, role, content, search_projection,
                       created_at, fingerprint
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
                SELECT COALESCE(search_projection, content), id, conversation_id
                  FROM messages
                 WHERE COALESCE(search_projection, content) <> '';",
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
                     WHERE provider NOT IN ('claude-code', 'codex')
              );
             DELETE FROM message_embeddings
              WHERE message_id IN (
                    SELECT messages.id
                      FROM messages
                      JOIN conversations ON conversations.id = messages.conversation_id
                     WHERE conversations.provider NOT IN ('claude-code', 'codex')
              );
             DELETE FROM messages
              WHERE conversation_id IN (
                    SELECT id FROM conversations
                     WHERE provider NOT IN ('claude-code', 'codex')
              );
             DELETE FROM conversations
              WHERE provider NOT IN ('claude-code', 'codex');
             DELETE FROM source_checkpoints
              WHERE provider NOT IN ('claude-code', 'codex');
             DELETE FROM tombstones
              WHERE provider NOT IN ('claude-code', 'codex');",
        )
        .map_err(AppError::database)
}

fn rebuild_all_semantic_chunks(connection: &Connection) -> Result<(), AppError> {
    connection
        .execute("DELETE FROM semantic_chunks", [])
        .map_err(AppError::database)?;
    let generation = connection
        .query_row(
            "SELECT semantic_ready_generation FROM derived_state WHERE singleton = 1",
            [],
            |row| row.get::<_, Option<String>>(0),
        )
        .map_err(AppError::database)?;
    let Some(generation) = generation else {
        return Ok(());
    };
    let mut statement = connection
        .prepare(
            "SELECT DISTINCT (m.rowid - 1) / ?2
               FROM message_embeddings e
               JOIN messages m ON m.id = e.message_id
              WHERE e.generation = ?1
                AND COALESCE(m.search_projection, m.content) <> ''
              ORDER BY 1",
        )
        .map_err(AppError::database)?;
    let chunk_ids = statement
        .query_map(params![generation, SEMANTIC_CHUNK_ROWS], |row| row.get(0))
        .map_err(AppError::database)?
        .collect::<Result<Vec<i64>, _>>()
        .map_err(AppError::database)?;
    drop(statement);
    for chunk_id in chunk_ids {
        rebuild_semantic_chunk(connection, chunk_id, Some(&generation))?;
    }
    Ok(())
}

fn rebuild_semantic_chunk(
    connection: &Connection,
    chunk_id: i64,
    requested_generation: Option<&str>,
) -> Result<(), AppError> {
    let (first_rowid, last_rowid) = semantic_chunk_rowid_range(chunk_id)?;
    let generation = match requested_generation {
        Some(generation) => Some(generation.to_owned()),
        None => connection
            .query_row(
                "SELECT e.generation
                   FROM message_embeddings e
                   JOIN messages m ON m.id = e.message_id
                  WHERE m.rowid BETWEEN ?1 AND ?2
                  ORDER BY m.rowid
                  LIMIT 1",
                params![first_rowid, last_rowid],
                |row| row.get(0),
            )
            .optional()
            .map_err(AppError::database)?,
    };
    let packed = match generation.as_deref() {
        Some(generation) => pack_semantic_chunk(connection, first_rowid, last_rowid, generation)?,
        None => None,
    };
    let Some(packed) = packed else {
        return delete_semantic_chunk(connection, chunk_id);
    };
    let generation = generation.ok_or_else(|| {
        AppError::internal("packed semantic chunk is missing its embedding generation")
    })?;
    write_semantic_chunk(connection, chunk_id, &generation, &packed)
}

fn semantic_chunk_rowid_range(chunk_id: i64) -> Result<(i64, i64), AppError> {
    if chunk_id < 0 {
        return Err(AppError::database_data(
            "negative semantic chunk identifier",
        ));
    }
    let first = chunk_id
        .checked_mul(SEMANTIC_CHUNK_ROWS)
        .and_then(|value| value.checked_add(1))
        .ok_or_else(|| AppError::database_data("semantic chunk rowid range overflows"))?;
    let last = first
        .checked_add(SEMANTIC_CHUNK_ROWS - 1)
        .ok_or_else(|| AppError::database_data("semantic chunk rowid range overflows"))?;
    Ok((first, last))
}

fn pack_semantic_chunk(
    connection: &Connection,
    first_rowid: i64,
    last_rowid: i64,
    generation: &str,
) -> Result<Option<PackedSemanticChunk>, AppError> {
    let mut statement = connection
        .prepare(
            "SELECT m.rowid, e.dimensions, e.norm, e.vector, c.provider, m.created_at
               FROM message_embeddings e
               JOIN messages m ON m.id = e.message_id
               JOIN conversations c ON c.id = m.conversation_id
              WHERE m.rowid BETWEEN ?1 AND ?2
                AND e.generation = ?3
                AND COALESCE(m.search_projection, m.content) <> ''
              ORDER BY m.rowid",
        )
        .map_err(AppError::database)?;
    let mut rows = statement
        .query(params![first_rowid, last_rowid, generation])
        .map_err(AppError::database)?;
    let mut dimensions = 0_usize;
    let mut vector_count = 0_usize;
    let mut message_rowids = Vec::new();
    let mut norms = Vec::new();
    let mut providers = Vec::new();
    let mut created_ats = Vec::new();
    let mut vectors = Vec::new();
    while let Some(row) = rows.next().map_err(AppError::database)? {
        let rowid: i64 = row.get(0).map_err(AppError::database)?;
        let row_dimensions: i64 = row.get(1).map_err(AppError::database)?;
        let row_dimensions = usize::try_from(row_dimensions)
            .map_err(|_| AppError::database_data("negative embedding dimensions"))?;
        let norm: f32 = row.get(2).map_err(AppError::database)?;
        let vector: Vec<u8> = row.get(3).map_err(AppError::database)?;
        validate_quantized_vector(row_dimensions, norm, &vector)
            .map_err(AppError::database_data)?;
        if dimensions == 0 {
            dimensions = row_dimensions;
        } else if dimensions != row_dimensions {
            return Err(AppError::database_data(
                "embedding rows have inconsistent dimensions",
            ));
        }
        let provider: String = row.get(4).map_err(AppError::database)?;
        let provider = match provider.as_str() {
            "claude-code" => 1_u8,
            "codex" => 2_u8,
            _ => {
                return Err(AppError::database_data(
                    "unsupported provider in semantic index",
                ));
            }
        };
        let created_at: Option<i64> = row.get(5).map_err(AppError::database)?;
        message_rowids.extend_from_slice(&rowid.to_le_bytes());
        norms.extend_from_slice(&norm.to_le_bytes());
        providers.push(provider);
        created_ats.extend_from_slice(&created_at.unwrap_or(MISSING_CREATED_AT).to_le_bytes());
        vectors.extend_from_slice(&vector);
        vector_count += 1;
    }
    if vector_count == 0 {
        return Ok(None);
    }
    let dimensions = i64::try_from(dimensions)
        .map_err(|_| AppError::internal("embedding has too many dimensions"))?;
    let vector_count = i64::try_from(vector_count)
        .map_err(|_| AppError::internal("semantic chunk has too many vectors"))?;
    Ok(Some(PackedSemanticChunk {
        dimensions,
        vector_count,
        message_rowids,
        norms,
        providers,
        created_ats,
        vectors,
    }))
}

fn write_semantic_chunk(
    connection: &Connection,
    chunk_id: i64,
    generation: &str,
    packed: &PackedSemanticChunk,
) -> Result<(), AppError> {
    connection
        .execute(
            "INSERT INTO semantic_chunks(
                chunk_id, generation, dimensions, vector_count, message_rowids,
                norms, providers, created_ats, vectors
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(chunk_id) DO UPDATE SET
                generation = excluded.generation,
                dimensions = excluded.dimensions,
                vector_count = excluded.vector_count,
                message_rowids = excluded.message_rowids,
                norms = excluded.norms,
                providers = excluded.providers,
                created_ats = excluded.created_ats,
                vectors = excluded.vectors",
            params![
                chunk_id,
                generation,
                packed.dimensions,
                packed.vector_count,
                packed.message_rowids,
                packed.norms,
                packed.providers,
                packed.created_ats,
                packed.vectors
            ],
        )
        .map_err(AppError::database)?;
    Ok(())
}

fn delete_semantic_chunk(connection: &Connection, chunk_id: i64) -> Result<(), AppError> {
    connection
        .execute(
            "DELETE FROM semantic_chunks WHERE chunk_id = ?1",
            [chunk_id],
        )
        .map_err(AppError::database)?;
    Ok(())
}

fn decode_i64_blob(bytes: &[u8], count: usize, label: &str) -> Result<Vec<i64>, AppError> {
    if bytes.len() != count.saturating_mul(8) {
        return Err(AppError::database_data(format!(
            "{label} bytes do not match semantic chunk size"
        )));
    }
    Ok(bytes
        .as_chunks::<8>()
        .0
        .iter()
        .map(|bytes| i64::from_le_bytes(*bytes))
        .collect())
}

fn decode_f32_blob(bytes: &[u8], count: usize, label: &str) -> Result<Vec<f32>, AppError> {
    if bytes.len() != count.saturating_mul(4) {
        return Err(AppError::database_data(format!(
            "{label} bytes do not match semantic chunk size"
        )));
    }
    Ok(bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|bytes| f32::from_le_bytes(*bytes))
        .collect())
}

fn provider_code(provider: &str) -> Result<u8, AppError> {
    match provider {
        "claude-code" => Ok(1),
        "codex" => Ok(2),
        _ => Err(AppError::usage(format!(
            "unsupported provider filter: {provider}"
        ))),
    }
}

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
                search_projection: None,
                created_at: Some(i64::MAX / 2),
            }],
        }
    }

    fn conversation_with_messages(messages: &[(&str, &str)]) -> Conversation {
        let mut value = conversation("");
        value.messages = messages
            .iter()
            .enumerate()
            .map(
                |(ordinal, (id, content))| crate::ingestion::NormalizedMessage {
                    id: (*id).to_owned(),
                    ordinal: i64::try_from(ordinal).expect("test ordinal"),
                    role: "user".to_owned(),
                    content: (*content).to_owned(),
                    search_projection: None,
                    created_at: Some(i64::MAX / 2),
                },
            )
            .collect();
        value.created_at = Some(i64::MAX / 2);
        value.updated_at = Some(i64::MAX / 2);
        value
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
    fn semantic_readiness_changes_atomically_with_canonical_messages() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("cass.sqlite3");
        let mut writer = Storage::open_writer(&path).expect("writer");
        writer
            .replace_conversation(&conversation("original text"))
            .expect("seed conversation");
        writer
            .replace_embeddings(
                "generation",
                &[EmbeddingWrite {
                    message_id: "message-1",
                    vector: &[127],
                    norm: 127.0,
                }],
            )
            .expect("seed embedding");
        writer
            .mark_semantic_index_ready("generation")
            .expect("mark initial index ready");
        writer.commit_writer().expect("commit ready index");

        let storage = Storage::open_existing(&path).expect("open ready database");
        assert!(
            storage
                .semantic_index_is_ready("generation")
                .expect("read initial readiness")
        );
        drop(storage);

        let mut rolled_back = Storage::open_writer(&path).expect("rollback writer");
        rolled_back
            .replace_conversation(&conversation("rolled back change"))
            .expect("change canonical message");
        assert!(
            !rolled_back
                .semantic_index_is_ready("generation")
                .expect("read transactional invalidation")
        );
        drop(rolled_back);

        let storage = Storage::open_existing(&path).expect("reopen after rollback");
        assert!(
            storage
                .semantic_index_is_ready("generation")
                .expect("read readiness after rollback")
        );
        drop(storage);

        let mut committed = Storage::open_writer(&path).expect("commit writer");
        committed
            .replace_conversation(&conversation("committed change"))
            .expect("change canonical message");
        committed.checkpoint_writer().expect("commit invalidation");
        assert!(
            !committed
                .semantic_index_is_ready("generation")
                .expect("read committed invalidation")
        );
        committed.commit_writer().expect("finish writer");

        let storage = Storage::open_existing(&path).expect("reopen incomplete database");
        assert!(
            !storage
                .semantic_index_is_ready("generation")
                .expect("read incomplete readiness")
        );
    }

    #[veritas::claims("indexing/canonical-and-fts-are-atomic")]
    #[test]
    fn incremental_fts_and_canonical_changes_roll_back_together() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("cass.sqlite3");
        let mut writer = Storage::open_writer(&path).expect("writer");
        writer
            .replace_conversation(&conversation("durable old term"))
            .expect("seed conversation");
        writer.commit_writer().expect("commit seed");
        drop(writer);

        let mut writer = Storage::open_writer(&path).expect("replacement writer");
        writer
            .replace_conversation(&conversation("uncommitted new term"))
            .expect("replace conversation");
        assert_eq!(
            writer
                .finalize_pending_fts_updates(u64::MAX)
                .expect("incremental FTS finalization"),
            FtsRefreshStrategy::Incremental
        );
        assert_eq!(
            writer.search("uncommitted", 10, None, None).unwrap().len(),
            1
        );
        drop(writer);

        let storage = Storage::open_existing(&path).expect("reopen database");
        assert_eq!(storage.search("durable", 10, None, None).unwrap().len(), 1);
        assert!(
            storage
                .search("uncommitted", 10, None, None)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            storage.view("message-1", 0).unwrap()[0].content,
            "durable old term"
        );
    }

    #[veritas::claims("indexing/canonical-and-fts-are-atomic")]
    #[test]
    fn bulk_fts_and_canonical_changes_roll_back_together() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("cass.sqlite3");
        let mut writer = Storage::open_writer(&path).expect("writer");
        writer
            .replace_conversation(&conversation("durable bulk term"))
            .expect("seed conversation");
        writer.commit_writer().expect("commit seed");
        drop(writer);

        let mut writer = Storage::open_writer(&path).expect("replacement writer");
        writer
            .replace_conversation(&conversation("uncommitted bulk replacement"))
            .expect("replace conversation");
        assert_eq!(
            writer
                .finalize_pending_fts_updates(1)
                .expect("bulk FTS finalization"),
            FtsRefreshStrategy::Bulk
        );
        assert_eq!(
            writer.search("replacement", 10, None, None).unwrap().len(),
            1
        );
        drop(writer);

        let storage = Storage::open_existing(&path).expect("reopen database");
        assert_eq!(storage.search("durable", 10, None, None).unwrap().len(), 1);
        assert!(
            storage
                .search("replacement", 10, None, None)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            storage.view("message-1", 0).unwrap()[0].content,
            "durable bulk term"
        );
    }

    #[test]
    fn fts_strategy_switches_at_the_declared_boundary() {
        let directory = tempfile::tempdir().expect("temporary directory");
        for (changed, expected) in [
            (8_usize, FtsRefreshStrategy::Incremental),
            (9, FtsRefreshStrategy::Bulk),
        ] {
            let path = directory.path().join(format!("changed-{changed}.sqlite3"));
            let original = (0..10)
                .map(|index| (format!("message-{index}"), format!("original {index}")))
                .collect::<Vec<_>>();
            let original_refs = original
                .iter()
                .map(|(id, content)| (id.as_str(), content.as_str()))
                .collect::<Vec<_>>();
            let mut writer = Storage::open_writer(&path).expect("writer");
            writer
                .replace_conversation(&conversation_with_messages(&original_refs))
                .expect("seed messages");
            writer.commit_writer().expect("commit seed");
            drop(writer);

            let replacement = original
                .iter()
                .enumerate()
                .map(|(index, (id, content))| {
                    let content = if index < changed {
                        format!("changed {index}")
                    } else {
                        content.clone()
                    };
                    (id.clone(), content)
                })
                .collect::<Vec<_>>();
            let replacement_refs = replacement
                .iter()
                .map(|(id, content)| (id.as_str(), content.as_str()))
                .collect::<Vec<_>>();
            let mut writer = Storage::open_writer(&path).expect("replacement writer");
            writer
                .replace_conversation(&conversation_with_messages(&replacement_refs))
                .expect("replace messages");
            let threshold = writer.measured_fts_bulk_threshold().unwrap();
            assert_eq!(threshold, 9);
            assert_eq!(
                writer.finalize_pending_fts_updates(threshold).unwrap(),
                expected,
                "changed messages: {changed}"
            );
        }

        let deletion_path = directory.path().join("deletion.sqlite3");
        let original = (0..10)
            .map(|index| (format!("message-{index}"), format!("original {index}")))
            .collect::<Vec<_>>();
        let original_refs = original
            .iter()
            .map(|(id, content)| (id.as_str(), content.as_str()))
            .collect::<Vec<_>>();
        let mut writer = Storage::open_writer(&deletion_path).expect("deletion writer");
        writer
            .replace_conversation(&conversation_with_messages(&original_refs))
            .expect("seed deletion corpus");
        writer.commit_writer().expect("commit deletion corpus");
        drop(writer);
        let retained_refs = original_refs[..5].to_vec();
        let mut writer = Storage::open_writer(&deletion_path).expect("deletion writer");
        writer
            .replace_conversation(&conversation_with_messages(&retained_refs))
            .expect("delete half the messages");
        let threshold = writer.measured_fts_bulk_threshold().unwrap();
        assert_eq!(threshold, 9, "deletions use the pre-transaction corpus");
        assert_eq!(
            writer.finalize_pending_fts_updates(threshold).unwrap(),
            FtsRefreshStrategy::Incremental
        );
    }

    #[test]
    fn incremental_and_bulk_fts_produce_equivalent_results() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let incremental_path = directory.path().join("incremental.sqlite3");
        let bulk_path = directory.path().join("bulk.sqlite3");
        for (path, cutoff) in [(&incremental_path, u64::MAX), (&bulk_path, 1)] {
            let mut writer = Storage::open_writer(path).expect("writer");
            writer
                .replace_conversation(&conversation_with_messages(&[
                    ("message-1", "alpha shared"),
                    ("message-2", "beta shared"),
                    ("message-3", "removed sentinel"),
                ]))
                .expect("seed messages");
            writer
                .finalize_pending_fts_updates(cutoff)
                .expect("seed FTS");
            writer
                .replace_conversation(&conversation_with_messages(&[
                    ("message-1", "gamma shared"),
                    ("message-4", "delta shared"),
                ]))
                .expect("replace messages");
            writer
                .finalize_pending_fts_updates(cutoff)
                .expect("refresh FTS");
            writer.commit_writer().expect("commit corpus");
        }

        let incremental = Storage::open_existing(&incremental_path).unwrap();
        let bulk = Storage::open_existing(&bulk_path).unwrap();
        for (query, limit, provider, days) in [
            ("shared", 10, None, None),
            ("shared", 1, Some("codex"), None),
            ("shared", 10, Some("codex"), Some(1)),
            ("gamma", 10, Some("codex"), None),
            ("removed", 10, None, None),
        ] {
            let ids = |storage: &Storage| {
                storage
                    .search(query, limit, provider, days)
                    .unwrap()
                    .into_iter()
                    .map(|hit| hit.id)
                    .collect::<Vec<_>>()
            };
            let incremental_ids = ids(&incremental);
            let bulk_ids = ids(&bulk);
            assert_eq!(incremental_ids, bulk_ids, "query {query}");
            if days.is_some() {
                assert!(
                    !incremental_ids.is_empty(),
                    "recency-filter equivalence must exercise matching rows"
                );
            }
        }
        assert!(
            incremental
                .search("alpha", 10, None, None)
                .unwrap()
                .is_empty()
        );
        assert!(
            incremental
                .search("beta", 10, None, None)
                .unwrap()
                .is_empty()
        );
        assert!(
            incremental
                .search("removed", 10, None, None)
                .unwrap()
                .is_empty()
        );
    }

    #[veritas::claims(
        "search/tool-results-are-not-searchable",
        "search/mixed-message-excludes-tool-result-text",
        "view/tool-results-remain-visible"
    )]
    #[test]
    fn search_projection_controls_fts_embeddings_and_rerank_text_not_view() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("cass.sqlite3");
        let mut corpus = conversation_with_messages(&[
            ("tool-only", "private tool payload"),
            ("mixed", "visible request\nprivate mixed payload"),
            ("ordinary", "ordinary searchable text"),
        ]);
        corpus.messages[0].search_projection = Some(String::new());
        corpus.messages[1].search_projection = Some("visible request".to_owned());

        let mut writer = Storage::open_writer(&path).expect("writer");
        writer
            .replace_conversation(&corpus)
            .expect("store projected messages");
        let pending = writer
            .messages_needing_embeddings("generation")
            .expect("embedding selection");
        assert_eq!(
            pending
                .iter()
                .map(|message| (message.id.as_str(), message.content.as_str()))
                .collect::<Vec<_>>(),
            vec![
                ("mixed", "visible request"),
                ("ordinary", "ordinary searchable text")
            ]
        );
        writer
            .replace_embeddings(
                "generation",
                &[
                    EmbeddingWrite {
                        message_id: "tool-only",
                        vector: &[127],
                        norm: 127.0,
                    },
                    EmbeddingWrite {
                        message_id: "mixed",
                        vector: &[126],
                        norm: 126.0,
                    },
                    EmbeddingWrite {
                        message_id: "ordinary",
                        vector: &[125],
                        norm: 125.0,
                    },
                ],
            )
            .expect("seed semantic vectors");
        writer.commit_writer().expect("commit projected messages");

        let storage = Storage::open_existing(&path).expect("open projected corpus");
        let counts = storage.counts().expect("counts");
        assert_eq!(counts.messages, 3);
        assert_eq!(counts.searchable_messages, 2);
        assert!(
            storage
                .search("private", 10, None, None)
                .unwrap()
                .is_empty()
        );
        assert_eq!(storage.search("visible", 10, None, None).unwrap().len(), 1);
        assert_eq!(storage.search("ordinary", 10, None, None).unwrap().len(), 1);

        let vectors = storage
            .semantic_chunks("generation", None, None)
            .expect("semantic vectors");
        assert_eq!(vectors.chunks.len(), 1);
        assert_eq!(vectors.chunks[0].message_rowids, [2, 3]);
        storage
            .connection
            .execute(
                "DELETE FROM message_embeddings WHERE message_id = 'ordinary'",
                [],
            )
            .expect("remove one searchable vector");
        assert_eq!(storage.embedding_count("generation").unwrap(), 2);
        assert_eq!(counts.searchable_messages, 2);
        assert!(
            !storage
                .semantic_coverage_is_complete("generation")
                .expect("exact semantic coverage")
        );
        assert_eq!(
            storage
                .search_documents(&["ordinary", "mixed"])
                .expect("rerank documents"),
            vec![
                "ordinary searchable text".to_owned(),
                "visible request".to_owned()
            ]
        );
        let context = storage.view("mixed", 1).expect("canonical view");
        assert_eq!(context[0].content, "private tool payload");
        assert_eq!(context[1].content, "visible request\nprivate mixed payload");
    }

    #[test]
    fn bulk_fts_refresh_preserves_unchanged_embeddings() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("cass.sqlite3");
        let mut writer = Storage::open_writer(&path).expect("writer");
        writer
            .replace_conversation(&conversation_with_messages(&[
                ("message-1", "first"),
                ("message-2", "second"),
            ]))
            .expect("seed messages");
        writer
            .replace_embeddings(
                "generation-a",
                &[
                    EmbeddingWrite {
                        message_id: "message-1",
                        vector: &[127],
                        norm: 127.0,
                    },
                    EmbeddingWrite {
                        message_id: "message-2",
                        vector: &[127],
                        norm: 127.0,
                    },
                ],
            )
            .expect("seed embeddings");
        writer
            .finalize_pending_fts_updates(1)
            .expect("seed bulk FTS");

        writer
            .replace_conversation(&conversation_with_messages(&[
                ("message-1", "changed first"),
                ("message-2", "second"),
            ]))
            .expect("change one message");
        assert_eq!(
            writer.finalize_pending_fts_updates(1).unwrap(),
            FtsRefreshStrategy::Bulk
        );
        assert_eq!(writer.embedding_count("generation-a").unwrap(), 1);
        assert_eq!(
            writer
                .messages_needing_embeddings("generation-a")
                .unwrap()
                .into_iter()
                .map(|message| message.id)
                .collect::<Vec<_>>(),
            ["message-1"]
        );
    }

    #[ignore = "manual FTS crossover benchmark"]
    #[test]
    fn benchmark_fts_refresh_crossover() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let benchmark_path = directory.path().join("benchmark.sqlite3");
        if let Some(source) = std::env::var_os("CASS_FTS_BENCH_DB") {
            let source = Connection::open(PathBuf::from(source)).expect("open benchmark source");
            source
                .execute(
                    "VACUUM INTO ?1",
                    [benchmark_path.to_string_lossy().as_ref()],
                )
                .expect("copy benchmark database");
        } else {
            let mut writer = Storage::open_writer(&benchmark_path).expect("writer");
            let messages = (0..25_000)
                .map(|index| crate::ingestion::NormalizedMessage {
                    id: format!("message-{index:05}"),
                    ordinal: i64::from(index),
                    role: "user".to_owned(),
                    content: format!("representative searchable text number {index}"),
                    search_projection: None,
                    created_at: Some(1),
                })
                .collect();
            let mut corpus = conversation("");
            corpus.messages = messages;
            writer
                .replace_conversation(&corpus)
                .expect("seed benchmark corpus");
            writer.commit_writer().expect("commit benchmark corpus");
        }

        let reader = Storage::open_existing(&benchmark_path).expect("benchmark reader");
        let total = reader.counts().expect("benchmark counts").messages;
        drop(reader);
        let mut deltas = vec![
            1,
            10,
            100,
            1_000,
            10_000,
            total / 10,
            total / 2,
            total.saturating_mul(75) / 100,
            total.saturating_mul(85) / 100,
            total.saturating_mul(90) / 100,
            total.saturating_mul(95) / 100,
            total,
        ];
        deltas.retain(|delta| *delta > 0 && *delta <= total);
        deltas.sort_unstable();
        deltas.dedup();

        eprintln!("fts benchmark corpus_messages={total}");
        for delta in deltas {
            let mut incremental = Vec::new();
            let mut bulk = Vec::new();
            for repetition in 0..3 {
                let order = if repetition % 2 == 0 {
                    [(u64::MAX, &mut incremental), (1, &mut bulk)]
                } else {
                    [(1, &mut bulk), (u64::MAX, &mut incremental)]
                };
                for (cutoff, timings) in order {
                    let mut writer =
                        Storage::open_writer(&benchmark_path).expect("benchmark writer");
                    writer
                        .connection
                        .execute(
                            "INSERT INTO pending_fts_messages(message_id)
                             SELECT id FROM messages ORDER BY id LIMIT ?1",
                            [i64::try_from(delta).expect("delta")],
                        )
                        .expect("stage benchmark messages");
                    writer
                        .connection
                        .execute(
                            "UPDATE messages SET content = content || ' changed'
                              WHERE id IN (SELECT message_id FROM pending_fts_messages)",
                            [],
                        )
                        .expect("mutate benchmark messages");
                    let started = std::time::Instant::now();
                    writer
                        .finalize_pending_fts_updates(cutoff)
                        .expect("benchmark FTS finalization");
                    timings.push(started.elapsed());
                    drop(writer);
                }
            }
            incremental.sort_unstable();
            bulk.sort_unstable();
            eprintln!(
                "delta={delta} incremental_ms={:.3} bulk_ms={:.3}",
                incremental[1].as_secs_f64() * 1_000.0,
                bulk[1].as_secs_f64() * 1_000.0
            );
        }
    }

    #[test]
    fn dirty_search_state_survives_a_batch_and_rebuilds_after_resume() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("cass.sqlite3");
        let mut writer = Storage::open_writer(&path).expect("writer");
        writer
            .defer_search_updates()
            .expect("defer FTS maintenance");
        writer
            .replace_conversation(&conversation("resumable needle"))
            .expect("message batch");
        writer.checkpoint_writer().expect("durable message batch");
        drop(writer);

        let mut resumed = Storage::open_writer(&path).expect("resumed writer");
        assert!(
            resumed
                .derived_search_is_dirty()
                .expect("dirty search marker")
        );
        assert!(
            resumed
                .search("resumable", 10, None, None)
                .expect("stale FTS is readable")
                .is_empty()
        );
        resumed
            .rebuild_derived_search_state()
            .expect("bulk FTS rebuild");
        assert!(
            !resumed
                .derived_search_is_dirty()
                .expect("clean search marker")
        );
        assert_eq!(
            resumed
                .search("resumable", 10, None, None)
                .expect("rebuilt search")
                .len(),
            1
        );
        resumed.commit_writer().expect("commit rebuilt state");
    }

    #[veritas::claims("indexing/partial-embeddings-resume")]
    #[test]
    fn committed_embedding_checkpoint_resumes_from_only_missing_rows() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("cass.sqlite3");
        let mut writer = Storage::open_writer(&path).expect("writer");
        writer
            .replace_conversation(&conversation_with_messages(&[
                ("message-1", "first searchable message"),
                ("message-2", "second searchable message"),
            ]))
            .expect("seed canonical messages");
        writer
            .checkpoint_writer()
            .expect("commit canonical and FTS state");
        writer
            .replace_embeddings(
                "generation",
                &[EmbeddingWrite {
                    message_id: "message-1",
                    vector: &[127, 0],
                    norm: 127.0,
                }],
            )
            .expect("first derived batch");
        writer
            .checkpoint_writer()
            .expect("commit first derived checkpoint");
        writer
            .replace_embeddings(
                "generation",
                &[EmbeddingWrite {
                    message_id: "message-2",
                    vector: &[0, 127],
                    norm: 127.0,
                }],
            )
            .expect("uncommitted second derived batch");
        drop(writer);

        let mut resumed = Storage::open_writer(&path).expect("resumed writer");
        assert_eq!(
            resumed
                .search("searchable", 10, None, None)
                .expect("durable FTS rows")
                .len(),
            2
        );
        assert!(
            !resumed
                .semantic_coverage_is_complete("generation")
                .expect("partial coverage")
        );
        let missing = resumed
            .messages_needing_embeddings("generation")
            .expect("missing embeddings");
        assert_eq!(
            missing
                .iter()
                .map(|message| message.id.as_str())
                .collect::<Vec<_>>(),
            ["message-2"]
        );
        resumed
            .replace_embeddings(
                "generation",
                &[EmbeddingWrite {
                    message_id: "message-2",
                    vector: &[0, 127],
                    norm: 127.0,
                }],
            )
            .expect("resumed derived batch");
        assert!(
            resumed
                .semantic_coverage_is_complete("generation")
                .expect("complete coverage")
        );
        resumed.commit_writer().expect("commit resumed embedding");
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

    #[test]
    fn malformed_semantic_chunk_metadata_is_rejected() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("cass.sqlite3");
        let mut writer = Storage::open_writer(&path).expect("writer");
        writer
            .replace_conversation(&conversation("packed vector"))
            .expect("seed message");
        writer
            .replace_embeddings(
                "generation",
                &[EmbeddingWrite {
                    message_id: "message-1",
                    vector: &[127, 0],
                    norm: 127.0,
                }],
            )
            .expect("seed embedding");
        writer
            .mark_semantic_index_ready("generation")
            .expect("publish semantic chunk");
        writer.commit_writer().expect("commit semantic chunk");
        writer
            .connection
            .execute("UPDATE semantic_chunks SET norms = X'00'", [])
            .expect("corrupt norm bytes");

        assert!(writer.semantic_chunks("generation", None, None).is_err());
    }

    #[test]
    fn semantic_chunks_bound_incremental_rewrites_to_affected_rowid_ranges() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("cass.sqlite3");
        let mut writer = Storage::open_writer(&path).expect("writer");
        writer
            .replace_conversation(&conversation("first chunk"))
            .expect("seed first message");
        writer
            .connection
            .execute(
                "INSERT INTO messages(
                    rowid, id, conversation_id, ordinal, role, content,
                    search_projection, created_at, fingerprint
                 ) VALUES (4097, 'message-4097', 'session-1', 4097, 'assistant',
                           'second chunk', NULL, 123, 'fingerprint')",
                [],
            )
            .expect("seed second chunk message");
        writer
            .replace_embeddings(
                "generation",
                &[
                    EmbeddingWrite {
                        message_id: "message-1",
                        vector: &[127, 0],
                        norm: 127.0,
                    },
                    EmbeddingWrite {
                        message_id: "message-4097",
                        vector: &[0, 127],
                        norm: 127.0,
                    },
                ],
            )
            .expect("seed embeddings");
        writer
            .mark_semantic_index_ready("generation")
            .expect("publish semantic chunks");
        writer.commit_writer().expect("commit chunks");

        let chunk_count: i64 = writer
            .connection
            .query_row("SELECT count(*) FROM semantic_chunks", [], |row| row.get(0))
            .expect("count chunks");
        assert_eq!(chunk_count, 2);
        let untouched_before: Vec<u8> = writer
            .connection
            .query_row(
                "SELECT vectors FROM semantic_chunks WHERE chunk_id = 1",
                [],
                |row| row.get(0),
            )
            .expect("second chunk before update");

        let mut writer = Storage::open_writer(&path).expect("update writer");
        writer
            .replace_embeddings(
                "generation",
                &[EmbeddingWrite {
                    message_id: "message-1",
                    vector: &[0, 126],
                    norm: 126.0,
                }],
            )
            .expect("update first chunk embedding");
        writer
            .mark_semantic_index_ready("generation")
            .expect("republish semantic chunks");
        writer
            .commit_writer()
            .expect("commit incremental chunk update");

        let untouched_after: Vec<u8> = writer
            .connection
            .query_row(
                "SELECT vectors FROM semantic_chunks WHERE chunk_id = 1",
                [],
                |row| row.get(0),
            )
            .expect("second chunk after update");
        assert_eq!(untouched_after, untouched_before);
        let updated: Vec<u8> = writer
            .connection
            .query_row(
                "SELECT vectors FROM semantic_chunks WHERE chunk_id = 0",
                [],
                |row| row.get(0),
            )
            .expect("updated first chunk");
        assert_eq!(updated, [0, 126]);
    }

    #[test]
    fn current_embedding_generation_cleanup_preserves_packed_chunks() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("cass.sqlite3");
        let mut writer = Storage::open_writer(&path).expect("writer");
        writer
            .replace_conversation(&conversation("current generation"))
            .expect("seed message");
        writer
            .replace_embeddings(
                "generation",
                &[EmbeddingWrite {
                    message_id: "message-1",
                    vector: &[127, 0],
                    norm: 127.0,
                }],
            )
            .expect("seed embedding");
        writer.commit_writer().expect("commit semantic chunk");

        let mut writer = Storage::open_writer(&path).expect("no-op generation writer");
        assert_eq!(
            writer
                .invalidate_embedding_generation("generation")
                .expect("retain current generation"),
            0
        );
        writer
            .commit_writer()
            .expect("commit no-op generation check");
        let preserved_chunks: i64 = writer
            .connection
            .query_row("SELECT count(*) FROM semantic_chunks", [], |row| row.get(0))
            .expect("count preserved chunks");
        assert_eq!(preserved_chunks, 1);
    }

    #[test]
    fn explicit_semantic_storage_identifiers_survive_vacuum() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("cass.sqlite3");
        let mut writer = Storage::open_writer(&path).expect("writer");
        writer
            .replace_conversation(&conversation("stable identifier"))
            .expect("seed message");
        writer
            .replace_embeddings(
                "generation",
                &[EmbeddingWrite {
                    message_id: "message-1",
                    vector: &[127, 0],
                    norm: 127.0,
                }],
            )
            .expect("seed embedding");
        writer
            .mark_semantic_index_ready("generation")
            .expect("publish semantic chunk");
        writer.commit_writer().expect("commit semantic chunk");
        drop(writer);
        let maintenance = Connection::open(&path).expect("maintenance connection");
        maintenance
            .execute_batch("VACUUM")
            .expect("vacuum database");
        drop(maintenance);
        let storage = Storage::open_existing(&path).expect("open vacuumed database");
        let chunks = storage
            .semantic_chunks("generation", None, None)
            .expect("read chunks after vacuum");
        assert_eq!(chunks.chunks[0].message_rowids, [1]);
        assert_eq!(
            storage
                .semantic_chunks("generation", Some("claude-code"), None)
                .expect("apply provider metadata filter")
                .chunks[0]
                .eligible,
            [false]
        );
        assert_eq!(
            storage
                .semantic_chunks("generation", None, Some(90))
                .expect("apply timestamp metadata filter")
                .chunks[0]
                .eligible,
            [true]
        );
        assert_eq!(
            storage
                .search_hits(&[1])
                .expect("hydrate stable storage identifiers")
                .iter()
                .map(|hit| hit.id.as_str())
                .collect::<Vec<_>>(),
            ["message-1"]
        );
    }

    const VERSION_SEVEN_SCHEMA_FIXTURE: &str = "CREATE TABLE conversations (
        id TEXT PRIMARY KEY,
        provider TEXT NOT NULL CHECK (provider IN (
            'claude-code', 'codex', 'opencode', 'github-copilot', 'hermes', 'pi'
        )),
        source_path TEXT NOT NULL UNIQUE, title TEXT, created_at INTEGER, updated_at INTEGER,
        source_fingerprint TEXT NOT NULL DEFAULT ''
     );
     CREATE TABLE messages (
        id TEXT PRIMARY KEY,
        conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
        ordinal INTEGER NOT NULL, role TEXT NOT NULL, content TEXT NOT NULL,
        created_at INTEGER, fingerprint TEXT NOT NULL DEFAULT '',
        UNIQUE(conversation_id, ordinal)
     );
     CREATE TABLE message_embeddings (
        message_id TEXT PRIMARY KEY REFERENCES messages(id) ON DELETE CASCADE,
        generation TEXT NOT NULL DEFAULT '', dimensions INTEGER NOT NULL,
        norm REAL NOT NULL, vector BLOB NOT NULL
     );
     CREATE VIRTUAL TABLE message_fts USING fts5(
        content, message_id UNINDEXED, conversation_id UNINDEXED, tokenize = 'unicode61'
     );
     CREATE TABLE tombstones (
        provider TEXT NOT NULL, conversation_id TEXT NOT NULL,
        forgotten_at INTEGER NOT NULL, PRIMARY KEY(provider, conversation_id)
     );
     CREATE TABLE source_checkpoints (
        provider TEXT NOT NULL, source_path TEXT NOT NULL,
        size_bytes INTEGER NOT NULL, modified_ns INTEGER NOT NULL,
        PRIMARY KEY(provider, source_path)
     );
     CREATE TABLE derived_state (
        singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
        search_dirty INTEGER NOT NULL CHECK(search_dirty IN (0, 1))
     );
     INSERT INTO derived_state VALUES (1, 0);
     INSERT INTO conversations(id, provider, source_path, source_fingerprint)
        VALUES ('session-1', 'codex', '/tmp/session-1.jsonl', 'old-source');
     INSERT INTO messages(id, conversation_id, ordinal, role, content, fingerprint)
        VALUES ('message-1', 'session-1', 0, 'user', 'preserved', 'old-message');
     INSERT INTO message_fts(content, message_id, conversation_id)
        VALUES ('preserved', 'message-1', 'session-1');
     INSERT INTO message_embeddings(message_id, generation, dimensions, norm, vector)
        VALUES ('message-1', 'old-generation', 1, 1.0, X'7F');
     INSERT INTO source_checkpoints(provider, source_path, size_bytes, modified_ns)
        VALUES ('codex', '/tmp/session-1.jsonl', 10, 20);
     PRAGMA user_version = 7;";

    fn version_eight_provider_fixture() -> String {
        VERSION_SEVEN_SCHEMA_FIXTURE
            .replace(
                "created_at INTEGER, fingerprint TEXT NOT NULL DEFAULT ''",
                "search_projection TEXT, created_at INTEGER, fingerprint TEXT NOT NULL DEFAULT ''",
            )
            .replace(
                "PRAGMA user_version = 7;",
                "INSERT INTO tombstones(provider, conversation_id, forgotten_at)
                    VALUES ('codex', 'forgotten-codex', 25);
                 INSERT INTO conversations(id, provider, source_path, source_fingerprint)
                    VALUES ('unsupported-session', 'opencode', '/tmp/opencode.jsonl', 'source');
                 INSERT INTO messages(
                    id, conversation_id, ordinal, role, content, search_projection, fingerprint
                 ) VALUES (
                    'unsupported-message', 'unsupported-session', 0, 'user',
                    'unsupported sentinel', NULL, 'message'
                 );
                 INSERT INTO message_fts(content, message_id, conversation_id)
                    VALUES ('unsupported sentinel', 'unsupported-message', 'unsupported-session');
                 INSERT INTO message_embeddings(message_id, generation, dimensions, norm, vector)
                    VALUES ('unsupported-message', 'generation', 1, 1.0, X'7F');
                 INSERT INTO source_checkpoints(provider, source_path, size_bytes, modified_ns)
                    VALUES ('opencode', '/tmp/opencode.jsonl', 10, 20);
                 INSERT INTO tombstones(provider, conversation_id, forgotten_at)
                    VALUES ('opencode', 'forgotten-opencode', 30);
                 PRAGMA user_version = 8;",
            )
    }

    #[veritas::claims(
        "storage/supported-schema-migrates",
        "storage/tool-search-projection-migrates"
    )]
    #[test]
    fn supported_schema_migrates_once_and_preserves_rows() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("cass.sqlite3");
        let connection = Connection::open(&path).expect("seed database");
        connection
            .execute_batch(VERSION_SEVEN_SCHEMA_FIXTURE)
            .expect("seed older schema");
        drop(connection);

        let storage = Storage::open(&path).expect("migrate database");
        let counts = storage.counts().expect("counts");
        assert_eq!(counts.messages, 1);
        assert_eq!(counts.embeddings, 0);
        assert_eq!(counts.searchable_messages, 1);
        assert!(
            storage
                .derived_search_is_dirty()
                .expect("dirty derived state")
        );
        assert_eq!(
            storage
                .connection
                .query_row("SELECT count(*) FROM message_fts", [], |row| row
                    .get::<_, i64>(0))
                .expect("cleared FTS"),
            0
        );
        assert_eq!(
            storage
                .connection
                .query_row(
                    "SELECT search_projection FROM messages WHERE id = 'message-1'",
                    [],
                    |row| row.get::<_, Option<String>>(0)
                )
                .expect("projection column"),
            None
        );
        assert_eq!(
            storage.view("message-1", 0).expect("canonical view")[0].content,
            "preserved"
        );
        assert_eq!(
            storage
                .connection
                .query_row("SELECT count(*) FROM source_checkpoints", [], |row| {
                    row.get::<_, i64>(0)
                })
                .expect("cleared checkpoints"),
            0
        );
        assert_eq!(
            storage
                .connection
                .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .expect("schema version"),
            SCHEMA_VERSION
        );
        drop(storage);
        let reopened = Storage::open(&path).expect("idempotent second open");
        assert_eq!(reopened.counts().expect("reopened counts").messages, 1);
        assert!(reopened.derived_search_is_dirty().expect("still dirty"));
    }

    #[veritas::claims("storage/unsupported-provider-data-is-removed")]
    #[test]
    fn version_eight_migration_removes_every_unsupported_provider_surface() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("cass.sqlite3");
        let connection = Connection::open(&path).expect("seed database");
        let fixture = version_eight_provider_fixture();
        connection
            .execute_batch(&fixture)
            .expect("seed version eight");
        drop(connection);

        let storage = Storage::open(&path).expect("migrate database");
        for table in ["conversations", "source_checkpoints", "tombstones"] {
            let query = format!("SELECT count(*) FROM {table} WHERE provider = 'opencode'");
            assert_eq!(
                storage
                    .connection
                    .query_row(&query, [], |row| row.get::<_, i64>(0))
                    .expect("unsupported provider count"),
                0,
                "unsupported rows remain in {table}"
            );
        }
        assert_eq!(
            storage
                .connection
                .query_row(
                    "SELECT count(*) FROM source_checkpoints WHERE provider = 'codex'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .expect("supported checkpoint count"),
            1
        );
        assert_eq!(
            storage
                .connection
                .query_row(
                    "SELECT count(*) FROM tombstones WHERE provider = 'codex'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .expect("supported tombstone count"),
            1
        );
        assert_eq!(storage.counts().expect("counts").conversations, 1);
        assert_eq!(storage.counts().expect("counts").messages, 1);
        assert_eq!(storage.counts().expect("counts").embeddings, 1);
        assert!(
            storage
                .semantic_index_is_ready("old-generation")
                .expect("backfilled semantic readiness")
        );
        assert_eq!(
            storage
                .connection
                .query_row(
                    "SELECT count(*) FROM message_fts WHERE content = 'unsupported sentinel'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .expect("unsupported FTS count"),
            0
        );
        let schema: String = storage
            .connection
            .query_row(
                "SELECT sql FROM sqlite_schema WHERE name = 'conversations'",
                [],
                |row| row.get(0),
            )
            .expect("provider schema");
        assert!(schema.contains("'claude-code'"));
        assert!(schema.contains("'codex'"));
        assert!(!schema.contains("'opencode'"));
        for provider in ["opencode", "github-copilot", "hermes", "pi"] {
            let error = storage
                .connection
                .execute(
                    "INSERT INTO conversations(id, provider, source_path)
                     VALUES (?1, ?2, ?3)",
                    params![
                        format!("rejected-{provider}"),
                        provider,
                        format!("/tmp/rejected-{provider}.jsonl")
                    ],
                )
                .expect_err("unsupported provider rejected");
            assert!(matches!(error, rusqlite::Error::SqliteFailure(_, _)));
        }
        assert_eq!(
            storage
                .connection
                .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .expect("schema version"),
            SCHEMA_VERSION
        );
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
        storage.commit_writer().expect("commit replacement");
        assert!(storage.search("first", 10, None, None).unwrap().is_empty());
        assert_eq!(
            storage.search("second", 10, None, None).unwrap()[0].id,
            "message-2"
        );
    }

    #[test]
    fn purging_a_conversation_removes_its_staged_fts_rows() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("cass.sqlite3");
        let mut writer = Storage::open_writer(&path).expect("writer");
        writer
            .replace_conversation(&conversation("disappearing sentinel"))
            .expect("seed conversation");
        writer.commit_writer().expect("commit seed");
        drop(writer);

        let mut writer = Storage::open_writer(&path).expect("purge writer");
        assert_eq!(
            writer
                .purge_missing_sources(
                    "codex",
                    &BTreeSet::new(),
                    &[std::path::PathBuf::from("/tmp")],
                    None,
                )
                .expect("purge missing source"),
            1
        );
        writer.commit_writer().expect("commit purge");
        assert!(
            writer
                .search("disappearing", 10, None, None)
                .unwrap()
                .is_empty()
        );
        assert_eq!(writer.counts().unwrap().messages, 0);
    }

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
                .semantic_chunks("new-generation", None, None)
                .expect("semantic documents")
                .chunks
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
            .semantic_chunks("new-generation", None, None)
            .expect("stored quantized vectors");
        assert_eq!(vectors.chunks.len(), 1);
        assert_eq!(vectors.chunks[0].message_rowids, [1]);
        assert_eq!(vectors.chunks[0].values, [0, 127]);
        assert_eq!(vectors.chunks[0].norms, [127.0]);
        assert_eq!(vectors.dimensions, 2);
        assert_eq!(
            writer.search_hits(&[1]).expect("hydrate semantic hit")[0].content,
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
