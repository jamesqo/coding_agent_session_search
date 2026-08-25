use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use fastembed::{
    EmbeddingModel, RerankInitOptions, RerankerModel, TextEmbedding, TextInitOptions, TextRerank,
};
use serde::{Deserialize, Serialize};
use walkdir::WalkDir;

use crate::AppError;
use crate::storage::{EmbeddingWrite, SearchHit, SemanticVectors, Storage};

const EMBEDDING_MODEL: EmbeddingModel = EmbeddingModel::AllMiniLML6V2Q;
const RERANKER_MODEL: RerankerModel = RerankerModel::JINARerankerV1TurboEn;
const MARKER_NAME: &str = "installed.json";
const CANDIDATE_LIMIT: usize = 50;
const RERANK_LIMIT: usize = 20;
const EMBEDDING_BATCH_SIZE: usize = 32;
const RRF_K: f32 = 60.0;
static EMBEDDING_GENERATION: OnceLock<String> = OnceLock::new();

pub(crate) struct Models {
    root: PathBuf,
}

pub(crate) struct Backend {
    embedding: TextEmbedding,
    reranker: TextRerank,
}

struct QuantizedVector {
    values: Vec<u8>,
    norm: f32,
}

#[derive(Debug, Serialize)]
pub(crate) struct InstallSummary {
    embedding_model: &'static str,
    reranker_model: &'static str,
    embedding_dimensions: usize,
    files: usize,
}

#[derive(Debug, Serialize, Deserialize)]
struct InstallMarker {
    schema: u32,
    embedding_model: String,
    reranker_model: String,
    embedding_dimensions: usize,
    files: Vec<InstalledFile>,
}

#[derive(Debug, Serialize, Deserialize)]
struct InstalledFile {
    path: String,
    size: u64,
}

impl Models {
    pub(crate) fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub(crate) fn is_installed(&self) -> bool {
        self.read_valid_marker().is_ok()
    }

    pub(crate) fn install(&self) -> Result<InstallSummary, AppError> {
        fs::create_dir_all(&self.root).map_err(AppError::io)?;
        let mut backend = Backend::load(&self.root)?;
        let embeddings = backend.embed(&["semantic model installation check"])?;
        let embedding_dimensions = embeddings
            .first()
            .map(Vec::len)
            .ok_or_else(|| AppError::model("embedding model returned no smoke result"))?;
        let reranked = backend.rerank(
            "semantic model installation check",
            &["unrelated", "semantic model installation check"],
        )?;
        if reranked.len() != 2 {
            return Err(AppError::model(
                "reranker returned an unexpected smoke result count",
            ));
        }

        let files = inventory(&self.root)?;
        if files.is_empty() {
            return Err(AppError::model("model installation produced no assets"));
        }
        let marker = InstallMarker {
            schema: 1,
            embedding_model: EMBEDDING_MODEL.to_string(),
            reranker_model: RERANKER_MODEL.to_string(),
            embedding_dimensions,
            files,
        };
        write_marker(&self.root, &marker)?;
        Ok(InstallSummary {
            embedding_model: "sentence-transformers/all-MiniLM-L6-v2 (quantized ONNX)",
            reranker_model: "jinaai/jina-reranker-v1-turbo-en",
            embedding_dimensions,
            files: marker.files.len(),
        })
    }

    pub(crate) fn load(&self) -> Result<Option<Backend>, AppError> {
        if !self.root.join(MARKER_NAME).is_file() {
            return Ok(None);
        }
        self.read_valid_marker()?;
        Backend::load(&self.root).map(Some)
    }

    fn read_valid_marker(&self) -> Result<InstallMarker, AppError> {
        let bytes = fs::read(self.root.join(MARKER_NAME)).map_err(AppError::io)?;
        let marker: InstallMarker = serde_json::from_slice(&bytes)
            .map_err(|error| AppError::model(format!("invalid model marker: {error}")))?;
        if marker.schema != 1
            || marker.embedding_model != EMBEDDING_MODEL.to_string()
            || marker.reranker_model != RERANKER_MODEL.to_string()
            || marker.files.is_empty()
        {
            return Err(AppError::model(
                "model marker does not match this CASS build",
            ));
        }
        for file in &marker.files {
            let path = self.root.join(&file.path);
            let metadata = fs::metadata(path).map_err(AppError::io)?;
            if !metadata.is_file() || metadata.len() != file.size {
                return Err(AppError::model("installed model assets are incomplete"));
            }
        }
        Ok(marker)
    }
}

impl Backend {
    fn load(root: &Path) -> Result<Self, AppError> {
        let embedding = TextEmbedding::try_new(
            TextInitOptions::new(EMBEDDING_MODEL)
                .with_cache_dir(root.to_path_buf())
                .with_show_download_progress(false),
        )
        .map_err(|error| AppError::model(format!("failed to load embedding model: {error}")))?;
        let reranker = TextRerank::try_new(
            RerankInitOptions::new(RERANKER_MODEL)
                .with_cache_dir(root.to_path_buf())
                .with_show_download_progress(false),
        )
        .map_err(|error| AppError::model(format!("failed to load reranking model: {error}")))?;
        Ok(Self {
            embedding,
            reranker,
        })
    }

    pub(crate) fn embed<S: AsRef<str> + Send + Sync>(
        &mut self,
        texts: &[S],
    ) -> Result<Vec<Vec<f32>>, AppError> {
        self.embedding
            .embed(texts, None)
            .map_err(|error| AppError::model(format!("embedding inference failed: {error}")))
    }

    fn rerank<S: AsRef<str> + Send + Sync>(
        &mut self,
        query: S,
        documents: &[S],
    ) -> Result<Vec<fastembed::RerankResult>, AppError> {
        self.reranker
            .rerank(query, documents, false, None)
            .map_err(|error| AppError::model(format!("reranking inference failed: {error}")))
    }
}

pub(crate) fn embedding_generation() -> &'static str {
    EMBEDDING_GENERATION.get_or_init(|| {
        let specification = format!(
            "fastembed=6.0.1;model={EMBEDDING_MODEL};\
             vector=i8-per-vector-symmetric;cosine=quantized-flat-exact;schema=2"
        );
        blake3::hash(specification.as_bytes()).to_hex().to_string()
    })
}

pub(crate) fn rebuild_embeddings(
    storage: &mut Storage,
    backend: &mut Backend,
) -> Result<u64, AppError> {
    let generation = embedding_generation();
    let messages = storage.messages_needing_embeddings(generation)?;
    if messages.is_empty() {
        return Ok(0);
    }
    let mut stored = 0_u64;
    for batch in messages.chunks(EMBEDDING_BATCH_SIZE) {
        let texts: Vec<&str> = batch
            .iter()
            .map(|message| message.content.as_str())
            .collect();
        let batch_vectors = backend.embed(&texts)?;
        if batch.len() != batch_vectors.len() {
            return Err(AppError::model(
                "embedding model returned an unexpected result count",
            ));
        }
        let quantized: Vec<QuantizedVector> = batch_vectors
            .iter()
            .map(|vector| quantize_vector(vector))
            .collect();
        let rows: Vec<EmbeddingWrite<'_>> = batch
            .iter()
            .zip(&quantized)
            .map(|(message, vector)| EmbeddingWrite {
                message_id: &message.id,
                vector: &vector.values,
                norm: vector.norm,
            })
            .collect();
        storage.replace_embeddings(generation, &rows)?;
        stored = stored
            .checked_add(
                u64::try_from(rows.len()).map_err(|_| AppError::internal("too many embeddings"))?,
            )
            .ok_or_else(|| AppError::internal("too many embeddings"))?;
    }
    Ok(stored)
}

pub(crate) fn hybrid_search(
    storage: &Storage,
    backend: &mut Backend,
    query: &str,
    limit: usize,
    provider: Option<&str>,
    days: Option<u32>,
) -> Result<Vec<SearchHit>, AppError> {
    let lexical = storage.search(query, CANDIDATE_LIMIT, provider, days)?;
    let query_vector = backend
        .embed(&[query])?
        .pop()
        .ok_or_else(|| AppError::model("embedding model returned no query vector"))?;
    let query_vector = quantize_vector(&query_vector);
    let semantic = storage.semantic_vectors(embedding_generation(), provider, days)?;
    let ranked = rank_quantized(&query_vector, &semantic, CANDIDATE_LIMIT);
    if ranked.is_empty() {
        return Ok(lexical.into_iter().take(limit).collect());
    }
    let message_ids: Vec<&str> = ranked
        .iter()
        .map(|(index, _)| semantic.message_ids[*index].as_str())
        .collect();
    let mut semantic_hits = storage.search_hits(&message_ids)?;
    for (hit, (_, score)) in semantic_hits.iter_mut().zip(&ranked) {
        hit.semantic_score = Some(*score);
    }
    let mut fused = fuse(lexical, semantic_hits);
    let rerank_count = fused.len().min(RERANK_LIMIT);
    let documents: Vec<&str> = fused[..rerank_count]
        .iter()
        .map(|hit| hit.content.as_str())
        .collect();
    let reranked = backend.rerank(query, &documents)?;
    for result in reranked {
        if let Some(hit) = fused.get_mut(result.index) {
            hit.rerank_score = Some(result.score);
        }
    }
    fused[..rerank_count].sort_by(|left, right| {
        right
            .rerank_score
            .unwrap_or(f32::NEG_INFINITY)
            .total_cmp(&left.rerank_score.unwrap_or(f32::NEG_INFINITY))
            .then_with(|| right.fusion_score.total_cmp(&left.fusion_score))
            .then_with(|| left.id.cmp(&right.id))
    });
    fused.truncate(limit);
    Ok(fused)
}

fn fuse(lexical: Vec<SearchHit>, semantic: Vec<SearchHit>) -> Vec<SearchHit> {
    let mut hits = HashMap::new();
    let mut scores = HashMap::<String, f32>::new();
    for (rank, hit) in lexical.into_iter().enumerate() {
        let id = hit.id.clone();
        scores.insert(id.clone(), reciprocal_rank(rank));
        hits.insert(id, hit);
    }
    for (rank, hit) in semantic.into_iter().enumerate() {
        let id = hit.id.clone();
        *scores.entry(id.clone()).or_default() += reciprocal_rank(rank);
        if let Some(existing) = hits.get_mut(&id) {
            existing.semantic_score = hit.semantic_score;
        } else {
            hits.insert(id, hit);
        }
    }
    let mut fused: Vec<SearchHit> = hits
        .into_iter()
        .map(|(id, mut hit)| {
            hit.fusion_score = scores.get(&id).copied().unwrap_or_default();
            hit
        })
        .collect();
    fused.sort_by(|left, right| {
        right
            .fusion_score
            .total_cmp(&left.fusion_score)
            .then_with(|| left.id.cmp(&right.id))
    });
    fused
}

fn reciprocal_rank(zero_based_rank: usize) -> f32 {
    let rank = u16::try_from(zero_based_rank).unwrap_or(u16::MAX);
    1.0 / (RRF_K + f32::from(rank) + 1.0)
}

fn quantize_vector(vector: &[f32]) -> QuantizedVector {
    let max_abs = vector
        .iter()
        .map(|value| value.abs())
        .fold(0.0_f32, f32::max);
    if max_abs <= f32::EPSILON {
        return QuantizedVector {
            values: vec![0; vector.len()],
            norm: 0.0,
        };
    }
    let scale = 127.0 / max_abs;
    let mut norm_squared = 0_i32;
    let values = vector
        .iter()
        .map(|value| {
            let quantized = quantize_component(*value, scale);
            let quantized_i32 = i32::from(quantized);
            norm_squared += quantized_i32 * quantized_i32;
            quantized.cast_unsigned()
        })
        .collect();
    QuantizedVector {
        values,
        norm: exactly_representable_f32(norm_squared).sqrt(),
    }
}

fn quantized_cosine(left: &QuantizedVector, right: &[u8], right_norm: f32) -> f32 {
    if left.values.len() != right.len() || left.values.is_empty() {
        return 0.0;
    }
    let denominator = left.norm * right_norm;
    if denominator <= f32::EPSILON {
        return 0.0;
    }
    let product = left
        .values
        .iter()
        .zip(right)
        .map(|(&left, &right)| i32::from(left.cast_signed()) * i32::from(right.cast_signed()))
        .sum::<i32>();
    (exactly_representable_f32(product) / denominator).clamp(-1.0, 1.0)
}

fn quantize_component(value: f32, scale: f32) -> i8 {
    let rounded = (value * scale).round().clamp(-127.0, 127.0);
    debug_assert!((-127.0..=127.0).contains(&rounded));
    #[allow(clippy::cast_possible_truncation)]
    {
        rounded as i8
    }
}

fn exactly_representable_f32(value: i32) -> f32 {
    // A 384-dimensional signed-byte dot product is bounded by 6,193,536,
    // below f32's exact-integer limit of 2^24.
    debug_assert!(value.unsigned_abs() <= 1 << f32::MANTISSA_DIGITS);
    #[allow(clippy::cast_precision_loss)]
    {
        value as f32
    }
}

fn rank_quantized(
    query: &QuantizedVector,
    vectors: &SemanticVectors,
    limit: usize,
) -> Vec<(usize, f32)> {
    if limit == 0
        || vectors.dimensions == 0
        || query.values.len() != vectors.dimensions
        || vectors.norms.len() != vectors.message_ids.len()
        || vectors.values.len() != vectors.message_ids.len().saturating_mul(vectors.dimensions)
    {
        return Vec::new();
    }
    let mut ranked: Vec<(usize, f32)> = vectors
        .values
        .chunks_exact(vectors.dimensions)
        .zip(&vectors.norms)
        .enumerate()
        .map(|(index, (vector, norm))| (index, quantized_cosine(query, vector, *norm)))
        .collect();
    let compare = |left: &(usize, f32), right: &(usize, f32)| {
        right
            .1
            .total_cmp(&left.1)
            .then_with(|| vectors.message_ids[left.0].cmp(&vectors.message_ids[right.0]))
    };
    if ranked.len() > limit {
        ranked.select_nth_unstable_by(limit, compare);
        ranked.truncate(limit);
    }
    ranked.sort_unstable_by(compare);
    ranked
}

fn inventory(root: &Path) -> Result<Vec<InstalledFile>, AppError> {
    let mut files = Vec::new();
    for entry in WalkDir::new(root).follow_links(false) {
        let entry = entry.map_err(|error| AppError::model(error.to_string()))?;
        if entry.path() == root.join(MARKER_NAME)
            || !fs::metadata(entry.path()).is_ok_and(|m| m.is_file())
        {
            continue;
        }
        let path = entry
            .path()
            .strip_prefix(root)
            .map_err(|error| AppError::model(error.to_string()))?;
        let metadata = fs::metadata(entry.path()).map_err(AppError::io)?;
        files.push(InstalledFile {
            path: path.to_string_lossy().into_owned(),
            size: metadata.len(),
        });
    }
    files.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(files)
}

fn write_marker(root: &Path, marker: &InstallMarker) -> Result<(), AppError> {
    let path = root.join(MARKER_NAME);
    let temporary = root.join("installed.json.tmp");
    let bytes = serde_json::to_vec_pretty(marker).map_err(|error| {
        AppError::internal(format!("failed to serialize model marker: {error}"))
    })?;
    fs::write(&temporary, bytes).map_err(AppError::io)?;
    fs::rename(temporary, path).map_err(AppError::io)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quantized_cosine_handles_bounds_and_preserves_direction() {
        let positive = quantize_vector(&[1.0, 0.0]);
        let negative = quantize_vector(&[-1.0, 0.0]);
        let zero = quantize_vector(&[0.0, 0.0]);
        assert!(
            (quantized_cosine(&positive, &positive.values, positive.norm) - 1.0).abs()
                < f32::EPSILON
        );
        assert!(
            (quantized_cosine(&positive, &negative.values, negative.norm) + 1.0).abs()
                < f32::EPSILON
        );
        assert_eq!(quantized_cosine(&positive, &[127], 127.0), 0.0);
        assert_eq!(
            quantized_cosine(&zero, &positive.values, positive.norm),
            0.0
        );
    }

    #[test]
    fn quantized_ranking_selects_the_best_candidates_deterministically() {
        let query = quantize_vector(&[1.0, 0.0]);
        let first = quantize_vector(&[1.0, 0.0]);
        let second = quantize_vector(&[0.8, 0.2]);
        let third = quantize_vector(&[-1.0, 0.0]);
        let vectors = SemanticVectors {
            message_ids: vec!["b".to_owned(), "a".to_owned(), "c".to_owned()],
            values: [first.values, second.values, third.values].concat(),
            norms: vec![first.norm, second.norm, third.norm],
            dimensions: 2,
        };

        let ranked = rank_quantized(&query, &vectors, 2);
        assert_eq!(
            ranked.iter().map(|(index, _)| *index).collect::<Vec<_>>(),
            [0, 1]
        );
        assert!(ranked[0].1 > ranked[1].1);
    }

    #[test]
    fn models_are_not_ready_without_a_valid_marker() {
        let directory = tempfile::tempdir().expect("temporary model directory");
        let models = Models::new(directory.path().to_path_buf());
        assert!(!models.is_installed());
        fs::write(directory.path().join(MARKER_NAME), b"not json").expect("invalid marker");
        assert!(!models.is_installed());
    }

    #[test]
    fn rrf_fusion_rewards_overlap_and_breaks_ties_by_id() {
        fn hit(id: &str) -> SearchHit {
            SearchHit {
                id: id.to_owned(),
                conversation_id: "conversation".to_owned(),
                provider: "codex".to_owned(),
                role: "user".to_owned(),
                content: id.to_owned(),
                lexical_score: None,
                semantic_score: None,
                fusion_score: 0.0,
                rerank_score: None,
            }
        }

        let mut semantic_b = hit("b");
        semantic_b.semantic_score = Some(0.8);
        let fused = fuse(vec![hit("a"), hit("b")], vec![hit("c"), semantic_b]);
        let ids: Vec<&str> = fused
            .iter()
            .map(|candidate| candidate.id.as_str())
            .collect();
        assert_eq!(ids, ["b", "a", "c"]);
        assert!(fused[0].fusion_score > fused[1].fusion_score);
        assert_eq!(fused[0].semantic_score, Some(0.8));
        assert_eq!(fused[1].fusion_score, fused[2].fusion_score);
    }
}
