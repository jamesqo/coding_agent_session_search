use rusqlite::{Connection, OptionalExtension, params};

use crate::AppError;

pub(super) const SEMANTIC_CHUNK_ROWS: i64 = 4_096;
pub(super) const MISSING_CREATED_AT: i64 = i64::MIN;

struct PackedSemanticChunk {
    dimensions: i64,
    vector_count: i64,
    message_rowids: Vec<u8>,
    norms: Vec<u8>,
    providers: Vec<u8>,
    created_ats: Vec<u8>,
    vectors: Vec<u8>,
}

pub(super) fn rebuild_all_semantic_chunks(connection: &Connection) -> Result<(), AppError> {
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

pub(super) fn rebuild_semantic_chunk(
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

pub(super) fn decode_i64_blob(
    bytes: &[u8],
    count: usize,
    label: &str,
) -> Result<Vec<i64>, AppError> {
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

pub(super) fn decode_f32_blob(
    bytes: &[u8],
    count: usize,
    label: &str,
) -> Result<Vec<f32>, AppError> {
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

pub(super) fn provider_code(provider: &str) -> Result<u8, AppError> {
    match provider {
        "claude-code" => Ok(1),
        "codex" => Ok(2),
        _ => Err(AppError::usage(format!(
            "unsupported provider filter: {provider}"
        ))),
    }
}

pub(super) fn validate_quantized_vector(
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
