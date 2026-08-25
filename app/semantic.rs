use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use fastembed::{
    EmbeddingModel, RerankInitOptions, RerankerModel, TextEmbedding, TextInitOptions, TextRerank,
};
use serde::{Deserialize, Serialize};
use walkdir::WalkDir;

use crate::AppError;
use crate::storage::{SearchHit, Storage};

const EMBEDDING_MODEL: EmbeddingModel = EmbeddingModel::AllMiniLML6V2Q;
const RERANKER_MODEL: RerankerModel = RerankerModel::JINARerankerV1TurboEn;
const MARKER_NAME: &str = "installed.json";
const CANDIDATE_LIMIT: usize = 50;
const RERANK_LIMIT: usize = 20;
const EMBEDDING_BATCH_SIZE: usize = 32;
const RRF_K: f32 = 60.0;

pub(crate) struct Models {
    root: PathBuf,
}

pub(crate) struct Backend {
    embedding: TextEmbedding,
    reranker: TextRerank,
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

pub(crate) fn rebuild_embeddings(
    storage: &mut Storage,
    backend: &mut Backend,
) -> Result<u64, AppError> {
    let messages = storage.messages_needing_embeddings()?;
    if messages.is_empty() {
        return Ok(0);
    }
    let mut vectors = Vec::with_capacity(messages.len());
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
        vectors.extend(batch_vectors);
    }
    let rows: Vec<_> = messages
        .iter()
        .zip(vectors.iter())
        .map(|(message, vector)| (message.id.as_str(), vector.as_slice()))
        .collect();
    storage.replace_embeddings(&rows)?;
    u64::try_from(rows.len()).map_err(|_| AppError::internal("too many embeddings"))
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
    let mut semantic = storage.semantic_documents(provider, days)?;
    for document in &mut semantic {
        document.hit.semantic_score = Some(cosine_similarity(&query_vector, &document.vector));
    }
    semantic.sort_by(|left, right| {
        right
            .hit
            .semantic_score
            .unwrap_or(f32::NEG_INFINITY)
            .total_cmp(&left.hit.semantic_score.unwrap_or(f32::NEG_INFINITY))
            .then_with(|| left.hit.id.cmp(&right.hit.id))
    });
    semantic.truncate(CANDIDATE_LIMIT);
    if semantic.is_empty() {
        return Ok(lexical.into_iter().take(limit).collect());
    }

    let semantic_hits: Vec<SearchHit> = semantic.into_iter().map(|document| document.hit).collect();
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

fn cosine_similarity(left: &[f32], right: &[f32]) -> f32 {
    if left.len() != right.len() || left.is_empty() {
        return 0.0;
    }
    let mut dot = 0.0;
    let mut left_norm = 0.0;
    let mut right_norm = 0.0;
    for (&left_value, &right_value) in left.iter().zip(right) {
        dot += left_value * right_value;
        left_norm += left_value * left_value;
        right_norm += right_value * right_value;
    }
    let denominator = left_norm.sqrt() * right_norm.sqrt();
    if denominator <= f32::EPSILON {
        0.0
    } else {
        (dot / denominator).clamp(-1.0, 1.0)
    }
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
    fn cosine_similarity_handles_bounds_and_dimension_mismatch() {
        assert!((cosine_similarity(&[1.0, 0.0], &[1.0, 0.0]) - 1.0).abs() < f32::EPSILON);
        assert!((cosine_similarity(&[1.0, 0.0], &[-1.0, 0.0]) + 1.0).abs() < f32::EPSILON);
        assert_eq!(cosine_similarity(&[1.0], &[1.0, 2.0]), 0.0);
        assert_eq!(cosine_similarity(&[0.0], &[1.0]), 0.0);
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
