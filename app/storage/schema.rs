use rusqlite::Connection;

use super::semantic_chunks::rebuild_all_semantic_chunks;
use crate::AppError;

pub(super) const SCHEMA_VERSION: i64 = 12;

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
CREATE TABLE IF NOT EXISTS pending_embeddings (
    message_id TEXT PRIMARY KEY REFERENCES messages(id) ON DELETE CASCADE
) WITHOUT ROWID;
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

pub(super) fn initialize(connection: &Connection) -> Result<(), AppError> {
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
    if version < 12 {
        backfill_pending_embeddings(&transaction)?;
    }
    transaction
        .pragma_update(None, "user_version", SCHEMA_VERSION)
        .map_err(AppError::database)?;
    transaction.commit().map_err(AppError::database)?;
    connection
        .pragma_update(None, "foreign_keys", true)
        .map_err(AppError::database)
}

fn backfill_pending_embeddings(connection: &Connection) -> Result<(), AppError> {
    connection
        .execute_batch(
            "DELETE FROM pending_embeddings
              WHERE message_id NOT IN (SELECT id FROM messages);
             INSERT OR IGNORE INTO pending_embeddings(message_id)
             SELECT m.id
               FROM messages m
               LEFT JOIN message_embeddings e ON e.message_id = m.id
              WHERE COALESCE(m.search_projection, m.content) <> ''
                AND e.message_id IS NULL;",
        )
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
             DELETE FROM pending_embeddings
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
