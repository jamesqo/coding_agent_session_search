use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
use fastembed::QuantizationMode;
use fastembed::{
    EmbeddingModel, InitOptionsUserDefined, RerankInitOptions, RerankInitOptionsUserDefined,
    RerankerModel, TextEmbedding, TextInitOptions, TextRerank, TokenizerFiles,
    UserDefinedEmbeddingModel, UserDefinedRerankingModel,
};
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
use hf_hub::api::sync::ApiBuilder;
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
use ort::ep;
use serde::{Deserialize, Serialize};
use walkdir::WalkDir;

use crate::AppError;
use crate::storage::{EmbeddingWrite, SearchHit, SemanticChunks, Storage};

const EMBEDDING_MODEL: EmbeddingModel = EmbeddingModel::AllMiniLML6V2Q;
const RERANKER_MODEL: RerankerModel = RerankerModel::JINARerankerV1TurboEn;
const MARKER_NAME: &str = "installed.json";
const EMBEDDING_REPOSITORY: &str = "models--Xenova--all-MiniLM-L6-v2";
const RERANKER_REPOSITORY: &str = "models--jinaai--jina-reranker-v1-turbo-en";
const CANDIDATE_LIMIT: usize = 50;
const RERANK_LIMIT: usize = 10;
const EMBEDDING_BATCH_SIZE: usize = 8;
const EMBEDDING_WORKERS: usize = 8;
const EMBEDDING_THREADS_PER_WORKER: usize = 2;
#[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
const EMBEDDING_WAVE_SIZE: usize = EMBEDDING_BATCH_SIZE * EMBEDDING_WORKERS;
const COREML_BATCH_SIZE: usize = 32;
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
const COREML_SESSION_BATCH_SIZE: usize = COREML_BATCH_SIZE + 1;
const COREML_SEQUENCE_LENGTH: usize = 512;
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
const COREML_MODEL_FILE: &str = "onnx/model.onnx";
const COREML_CACHE_DIRECTORY: &str = "coreml-cache";
const EMBEDDING_CHECKPOINT_ROWS: u64 = 4_096;
const EMBEDDING_PROGRESS_INTERVAL: Duration = Duration::from_secs(1);
const RRF_K: f32 = 60.0;
static EMBEDDING_GENERATION: OnceLock<String> = OnceLock::new();

pub(crate) struct Models {
    root: PathBuf,
}

pub(crate) struct Backend {
    embedding: TextEmbedding,
    reranker: TextRerank,
}

pub(crate) struct EmbeddingPool {
    backend: EmbeddingPoolBackend,
}

enum EmbeddingPoolBackend {
    #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
    Cpu(Vec<TextEmbedding>),
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    CoreMl(TextEmbedding),
}

#[derive(Debug, Eq, PartialEq)]
struct EmbeddingGroup {
    content: String,
    message_ids: Vec<String>,
}

#[derive(Debug, Default, Eq, PartialEq)]
pub(crate) struct EmbeddingSummary {
    pub(crate) stored_vectors: u64,
    pub(crate) model_inferences: u64,
    pub(crate) reused_vectors: u64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct EmbeddingProgress {
    pub(crate) stored_vectors: u64,
    pub(crate) total_vectors: u64,
    pub(crate) model_inferences: u64,
    pub(crate) reused_vectors: u64,
    pub(crate) elapsed_milliseconds: u64,
    pub(crate) stored_vectors_per_second: f64,
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
    #[serde(default)]
    coreml_embedding: bool,
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

    pub(crate) fn require_installed(&self) -> Result<(), AppError> {
        if !self.root.join(MARKER_NAME).is_file() {
            return Err(AppError::model(
                "semantic models are not installed; run `cass models install`",
            ));
        }
        self.read_valid_marker().map(|_| ())
    }

    pub(crate) fn install(&self) -> Result<InstallSummary, AppError> {
        fs::create_dir_all(&self.root).map_err(AppError::io)?;
        let mut backend = Backend::download_and_load(&self.root)?;
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
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        {
            let mut indexer = EmbeddingPool::load_local(&self.root)?;
            let smoke = [EmbeddingGroup {
                content: "semantic Core ML installation check".to_owned(),
                message_ids: vec!["smoke".to_owned()],
            }];
            if indexer.embed_wave(&smoke)?.len() != 1 {
                return Err(AppError::model(
                    "Core ML embedding model returned an unexpected smoke result count",
                ));
            }
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
            coreml_embedding: coreml_embedding_enabled(),
            files,
        };
        write_marker(&self.root, &marker)?;
        Ok(InstallSummary {
            embedding_model: embedding_model_description(),
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
        Backend::load_local(&self.root).map(Some)
    }

    pub(crate) fn load_indexer(&self) -> Result<Option<EmbeddingPool>, AppError> {
        if !self.root.join(MARKER_NAME).is_file() {
            return Ok(None);
        }
        self.read_valid_marker()?;
        EmbeddingPool::load_local(&self.root).map(Some)
    }

    fn read_valid_marker(&self) -> Result<InstallMarker, AppError> {
        let bytes = fs::read(self.root.join(MARKER_NAME)).map_err(|error| {
            AppError::model(format!("cannot read installed model marker: {error}"))
        })?;
        let marker: InstallMarker = serde_json::from_slice(&bytes)
            .map_err(|error| AppError::model(format!("invalid model marker: {error}")))?;
        if marker.schema != 1
            || marker.embedding_model != EMBEDDING_MODEL.to_string()
            || marker.reranker_model != RERANKER_MODEL.to_string()
            || marker.coreml_embedding != coreml_embedding_enabled()
            || marker.files.is_empty()
        {
            return Err(AppError::model(
                "model marker does not match this CASS build",
            ));
        }
        for file in &marker.files {
            let path = self.root.join(&file.path);
            let metadata = fs::metadata(path).map_err(|error| {
                AppError::model(format!("installed model asset is unavailable: {error}"))
            })?;
            if !metadata.is_file() || metadata.len() != file.size {
                return Err(AppError::model("installed model assets are incomplete"));
            }
        }
        Ok(marker)
    }
}

impl Backend {
    fn download_and_load(root: &Path) -> Result<Self, AppError> {
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
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        download_coreml_embedding(root)?;
        Ok(Self {
            embedding,
            reranker,
        })
    }

    fn load_local(root: &Path) -> Result<Self, AppError> {
        let embedding = load_local_embedding(root, None)?;
        let reranker_snapshot = snapshot(root, RERANKER_REPOSITORY)?;
        let reranker_info = TextRerank::get_model_info(&RERANKER_MODEL);
        let reranker_model = UserDefinedRerankingModel::new(
            reranker_snapshot.join(reranker_info.model_file),
            read_tokenizer_files(&reranker_snapshot)?,
        );
        let reranker = TextRerank::try_new_from_user_defined(
            reranker_model,
            RerankInitOptionsUserDefined::default(),
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

impl EmbeddingPool {
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    fn load_local(root: &Path) -> Result<Self, AppError> {
        Ok(Self {
            backend: EmbeddingPoolBackend::CoreMl(load_coreml_embedding(root)?),
        })
    }

    #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
    fn load_local(root: &Path) -> Result<Self, AppError> {
        let workers = (0..EMBEDDING_WORKERS)
            .map(|_| load_local_embedding(root, Some(EMBEDDING_THREADS_PER_WORKER)))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            backend: EmbeddingPoolBackend::Cpu(workers),
        })
    }

    fn embed_wave(&mut self, groups: &[EmbeddingGroup]) -> Result<Vec<Vec<f32>>, AppError> {
        match &mut self.backend {
            #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
            EmbeddingPoolBackend::Cpu(workers) => embed_cpu_wave(workers, groups),
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            EmbeddingPoolBackend::CoreMl(embedding) => embed_coreml_wave(embedding, groups),
        }
    }

    const fn batch_size(&self) -> usize {
        match self.backend {
            #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
            EmbeddingPoolBackend::Cpu(_) => EMBEDDING_BATCH_SIZE,
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            EmbeddingPoolBackend::CoreMl(_) => COREML_BATCH_SIZE,
        }
    }

    const fn wave_size(&self) -> usize {
        match self.backend {
            #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
            EmbeddingPoolBackend::Cpu(_) => EMBEDDING_WAVE_SIZE,
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            EmbeddingPoolBackend::CoreMl(_) => COREML_BATCH_SIZE,
        }
    }
}

#[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
fn embed_cpu_wave(
    workers: &mut [TextEmbedding],
    groups: &[EmbeddingGroup],
) -> Result<Vec<Vec<f32>>, AppError> {
    let worker_batches = groups.chunks(EMBEDDING_BATCH_SIZE);
    let results = std::thread::scope(|scope| {
        workers
            .iter_mut()
            .zip(worker_batches)
            .map(|(worker, batch)| {
                scope.spawn(move || {
                    let texts = batch
                        .iter()
                        .map(|group| group.content.as_str())
                        .collect::<Vec<_>>();
                    worker.embed(texts, None).map_err(|error| {
                        AppError::model(format!("embedding inference failed: {error}"))
                    })
                })
            })
            .collect::<Vec<_>>()
            .into_iter()
            .map(|handle| {
                handle
                    .join()
                    .map_err(|_| AppError::internal("embedding worker panicked"))?
            })
            .collect::<Result<Vec<_>, _>>()
    })?;
    Ok(results.into_iter().flatten().collect())
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn embed_coreml_wave(
    embedding: &mut TextEmbedding,
    groups: &[EmbeddingGroup],
) -> Result<Vec<Vec<f32>>, AppError> {
    if groups.len() > COREML_BATCH_SIZE {
        return Err(AppError::internal("Core ML embedding wave is too large"));
    }
    let real_count = groups.len();
    let mut texts = groups
        .iter()
        .map(|group| group.content.clone())
        .collect::<Vec<_>>();
    texts.push("hello ".repeat(COREML_SEQUENCE_LENGTH + 1));
    texts.resize(COREML_SESSION_BATCH_SIZE, String::new());
    let mut vectors = embedding
        .embed(texts, None)
        .map_err(|error| AppError::model(format!("Core ML embedding inference failed: {error}")))?;
    if vectors.len() != COREML_SESSION_BATCH_SIZE {
        return Err(AppError::model(
            "Core ML embedding model returned an unexpected result count",
        ));
    }
    vectors.truncate(real_count);
    Ok(vectors)
}

fn load_local_embedding(
    root: &Path,
    intra_threads: Option<usize>,
) -> Result<TextEmbedding, AppError> {
    let embedding_snapshot = snapshot(root, EMBEDDING_REPOSITORY)?;
    let embedding_info = TextEmbedding::get_model_info(&EMBEDDING_MODEL)
        .map_err(|error| AppError::model(format!("unknown embedding model: {error}")))?;
    let mut embedding_model = UserDefinedEmbeddingModel::new(
        read_model_file(&embedding_snapshot.join(&embedding_info.model_file))?,
        read_tokenizer_files(&embedding_snapshot)?,
    )
    .with_quantization(TextEmbedding::get_quantization_mode(&EMBEDDING_MODEL));
    embedding_model.pooling = TextEmbedding::get_default_pooling_method(&EMBEDDING_MODEL);
    embedding_model
        .output_key
        .clone_from(&embedding_info.output_key);
    let options = intra_threads.map_or_else(InitOptionsUserDefined::default, |threads| {
        InitOptionsUserDefined::default().with_intra_threads(threads)
    });
    TextEmbedding::try_new_from_user_defined(embedding_model, options)
        .map_err(|error| AppError::model(format!("failed to load embedding model: {error}")))
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn load_coreml_embedding(root: &Path) -> Result<TextEmbedding, AppError> {
    let embedding_snapshot = snapshot(root, EMBEDDING_REPOSITORY)?;
    let embedding_info = TextEmbedding::get_model_info(&EMBEDDING_MODEL)
        .map_err(|error| AppError::model(format!("unknown embedding model: {error}")))?;
    let mut embedding_model = UserDefinedEmbeddingModel::new(
        read_model_file(&embedding_snapshot.join(COREML_MODEL_FILE))?,
        read_tokenizer_files(&embedding_snapshot)?,
    )
    .with_quantization(QuantizationMode::None);
    embedding_model.pooling = TextEmbedding::get_default_pooling_method(&EMBEDDING_MODEL);
    embedding_model
        .output_key
        .clone_from(&embedding_info.output_key);

    let cache_dir = root.join(COREML_CACHE_DIRECTORY);
    fs::create_dir_all(&cache_dir).map_err(AppError::io)?;
    let provider = ep::CoreML::default()
        .with_model_format(ep::coreml::ModelFormat::MLProgram)
        .with_compute_units(ep::coreml::ComputeUnits::All)
        .with_specialization_strategy(ep::coreml::SpecializationStrategy::FastPrediction)
        .with_model_cache_dir(cache_dir.to_string_lossy())
        .build()
        .error_on_failure();
    let options = InitOptionsUserDefined::default()
        .with_execution_providers(vec![provider])
        .with_disable_cpu_fallback(true)
        .with_dimension_override("batch_size", 33)
        .with_dimension_override("sequence_length", 512);
    TextEmbedding::try_new_from_user_defined(embedding_model, options).map_err(|error| {
        AppError::model(format!("failed to load Core ML embedding model: {error}"))
    })
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn download_coreml_embedding(root: &Path) -> Result<(), AppError> {
    let api = ApiBuilder::new()
        .with_cache_dir(root.to_path_buf())
        .with_progress(false)
        .build()
        .map_err(|error| {
            AppError::model(format!("failed to initialize model download: {error}"))
        })?;
    api.model("Xenova/all-MiniLM-L6-v2".to_owned())
        .get(COREML_MODEL_FILE)
        .map(|_| ())
        .map_err(|error| AppError::model(format!("failed to download Core ML model: {error}")))
}

const fn coreml_embedding_enabled() -> bool {
    cfg!(all(target_os = "macos", target_arch = "aarch64"))
}

const fn embedding_model_description() -> &'static str {
    if coreml_embedding_enabled() {
        "sentence-transformers/all-MiniLM-L6-v2 (Core ML FP32)"
    } else {
        "sentence-transformers/all-MiniLM-L6-v2 (quantized ONNX)"
    }
}

fn snapshot(root: &Path, repository: &str) -> Result<PathBuf, AppError> {
    let revision =
        fs::read_to_string(root.join(repository).join("refs/main")).map_err(|error| {
            AppError::model(format!("cannot read installed model revision: {error}"))
        })?;
    let revision = revision.trim();
    if revision.len() != 40 || !revision.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(AppError::model("installed model revision is invalid"));
    }
    let snapshot = root.join(repository).join("snapshots").join(revision);
    if !snapshot.is_dir() {
        return Err(AppError::model("installed model snapshot is unavailable"));
    }
    Ok(snapshot)
}

fn read_model_file(path: &Path) -> Result<Vec<u8>, AppError> {
    fs::read(path).map_err(|error| AppError::model(format!("cannot read installed model: {error}")))
}

fn read_tokenizer_files(snapshot: &Path) -> Result<TokenizerFiles, AppError> {
    Ok(TokenizerFiles {
        tokenizer_file: read_model_file(&snapshot.join("tokenizer.json"))?,
        config_file: read_model_file(&snapshot.join("config.json"))?,
        special_tokens_map_file: read_model_file(&snapshot.join("special_tokens_map.json"))?,
        tokenizer_config_file: read_model_file(&snapshot.join("tokenizer_config.json"))?,
    })
}

pub(crate) fn embedding_generation() -> &'static str {
    EMBEDDING_GENERATION.get_or_init(|| {
        let specification = if coreml_embedding_enabled() {
            format!(
                "fastembed=6.0.1;model={EMBEDDING_MODEL};backend=coreml-fp32;\
                 batch={COREML_BATCH_SIZE};sequence={COREML_SEQUENCE_LENGTH};\
                 vector=i8-per-vector-symmetric;cosine=quantized-flat-exact;schema=4"
            )
        } else {
            format!(
                "fastembed=6.0.1;model={EMBEDDING_MODEL};batch={EMBEDDING_BATCH_SIZE};\
                 workers={EMBEDDING_WORKERS};threads={EMBEDDING_THREADS_PER_WORKER};\
                 vector=i8-per-vector-symmetric;cosine=quantized-flat-exact;schema=3"
            )
        };
        blake3::hash(specification.as_bytes()).to_hex().to_string()
    })
}

pub(crate) fn rebuild_embeddings(
    storage: &mut Storage,
    pool: &mut EmbeddingPool,
    mut on_progress: impl FnMut(EmbeddingProgress),
) -> Result<EmbeddingSummary, AppError> {
    let generation = embedding_generation();
    let groups = plan_embedding_groups(storage.messages_needing_embeddings()?);
    if groups.is_empty() {
        return Ok(EmbeddingSummary::default());
    }
    let total_vectors = groups.iter().try_fold(0_u64, |total, group| {
        total
            .checked_add(count_embeddings(group.message_ids.len())?)
            .ok_or_else(|| AppError::internal("too many embeddings"))
    })?;
    let started = Instant::now();
    let mut last_progress = started;
    let mut summary = EmbeddingSummary::default();
    let mut rows_since_checkpoint = 0_u64;
    let wave_size = pool.wave_size();
    let batch_size = pool.batch_size();
    for wave in groups.chunks(wave_size) {
        let wave_vectors = pool.embed_wave(wave)?;
        if wave.len() != wave_vectors.len() {
            return Err(AppError::model(
                "embedding model returned an unexpected result count",
            ));
        }
        for (batch, batch_vectors) in wave.chunks(batch_size).zip(wave_vectors.chunks(batch_size)) {
            let quantized = batch_vectors
                .iter()
                .map(|vector| quantize_vector(vector))
                .collect::<Vec<_>>();
            let mut rows = Vec::new();
            for (group, vector) in batch.iter().zip(&quantized) {
                for message_id in &group.message_ids {
                    rows.push(EmbeddingWrite {
                        message_id,
                        vector: &vector.values,
                        norm: vector.norm,
                    });
                }
            }
            storage.replace_embeddings(generation, &rows)?;
            let batch_rows = count_embeddings(rows.len())?;
            summary.stored_vectors = summary
                .stored_vectors
                .checked_add(batch_rows)
                .ok_or_else(|| AppError::internal("too many embeddings"))?;
            summary.model_inferences = summary
                .model_inferences
                .checked_add(count_embeddings(batch.len())?)
                .ok_or_else(|| AppError::internal("too many model inferences"))?;
            rows_since_checkpoint = rows_since_checkpoint
                .checked_add(batch_rows)
                .ok_or_else(|| AppError::internal("too many embeddings"))?;
            if rows_since_checkpoint >= EMBEDDING_CHECKPOINT_ROWS {
                storage.checkpoint_writer()?;
                rows_since_checkpoint = 0;
            }
            let now = Instant::now();
            if now.duration_since(last_progress) >= EMBEDDING_PROGRESS_INTERVAL
                || summary.stored_vectors == total_vectors
            {
                on_progress(embedding_progress(&summary, total_vectors, started));
                last_progress = now;
            }
        }
    }
    summary.reused_vectors = summary
        .stored_vectors
        .checked_sub(summary.model_inferences)
        .ok_or_else(|| AppError::internal("model inference count exceeds stored embeddings"))?;
    Ok(summary)
}

fn embedding_progress(
    summary: &EmbeddingSummary,
    total_vectors: u64,
    started: Instant,
) -> EmbeddingProgress {
    let elapsed = started.elapsed();
    #[allow(clippy::cast_precision_loss)]
    let stored_vectors_per_second = if elapsed.is_zero() {
        0.0
    } else {
        summary.stored_vectors as f64 / elapsed.as_secs_f64()
    };
    EmbeddingProgress {
        stored_vectors: summary.stored_vectors,
        total_vectors,
        model_inferences: summary.model_inferences,
        reused_vectors: summary
            .stored_vectors
            .saturating_sub(summary.model_inferences),
        elapsed_milliseconds: u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX),
        stored_vectors_per_second,
    }
}

fn plan_embedding_groups(messages: Vec<crate::storage::SearchableMessage>) -> Vec<EmbeddingGroup> {
    let mut grouped = HashMap::<String, Vec<String>>::new();
    for message in messages {
        grouped.entry(message.content).or_default().push(message.id);
    }
    let mut groups = grouped
        .into_iter()
        .map(|(content, mut message_ids)| {
            message_ids.sort_unstable();
            EmbeddingGroup {
                content,
                message_ids,
            }
        })
        .collect::<Vec<_>>();
    groups.sort_unstable_by(|left, right| {
        left.content
            .len()
            .cmp(&right.content.len())
            .then_with(|| left.content.cmp(&right.content))
    });
    groups
}

fn count_embeddings(count: usize) -> Result<u64, AppError> {
    u64::try_from(count).map_err(|_| AppError::internal("too many embeddings"))
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
    let semantic = storage.semantic_chunks(embedding_generation(), provider, days)?;
    let ranked = rank_quantized(&query_vector, &semantic, CANDIDATE_LIMIT);
    if ranked.is_empty() {
        return Ok(lexical.into_iter().take(limit).collect());
    }
    let message_rowids = ranked
        .iter()
        .map(|(message_rowid, _)| *message_rowid)
        .collect::<Vec<_>>();
    let mut semantic_hits = storage.search_hits(&message_rowids)?;
    for (hit, (_, score)) in semantic_hits.iter_mut().zip(&ranked) {
        hit.semantic_score = Some(*score);
    }
    let mut fused = fuse(lexical, semantic_hits);
    let rerank_count = fused.len().min(RERANK_LIMIT);
    let rerank_ids = fused[..rerank_count]
        .iter()
        .map(|hit| hit.id.as_str())
        .collect::<Vec<_>>();
    let documents = storage.search_documents(&rerank_ids)?;
    let document_refs = documents.iter().map(String::as_str).collect::<Vec<_>>();
    let reranked = backend.rerank(query, &document_refs)?;
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
    vectors: &SemanticChunks,
    limit: usize,
) -> Vec<(i64, f32)> {
    if limit == 0 || vectors.dimensions == 0 || query.values.len() != vectors.dimensions {
        return Vec::new();
    }
    let mut ranked = Vec::new();
    for chunk in &vectors.chunks {
        if chunk.dimensions != vectors.dimensions
            || chunk.norms.len() != chunk.message_rowids.len()
            || chunk.eligible.len() != chunk.message_rowids.len()
            || chunk.values.len() != chunk.message_rowids.len().saturating_mul(chunk.dimensions)
        {
            return Vec::new();
        }
        for (index, vector) in chunk.values.chunks_exact(chunk.dimensions).enumerate() {
            if chunk.eligible[index] {
                ranked.push((
                    chunk.message_rowids[index],
                    quantized_cosine(query, vector, chunk.norms[index]),
                ));
            }
        }
    }
    let compare = |left: &(i64, f32), right: &(i64, f32)| {
        right
            .1
            .total_cmp(&left.1)
            .then_with(|| left.0.cmp(&right.0))
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
        if path.starts_with(COREML_CACHE_DIRECTORY) {
            continue;
        }
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
    use crate::storage::SearchableMessage;
    use veritas_test_macros as veritas;

    #[test]
    fn reranking_is_bounded_to_ten_documents() {
        assert_eq!(RERANK_LIMIT, 10);
    }

    #[veritas::claims("semantic-indexing/repeated-text-reuses-inference")]
    #[test]
    fn embedding_plan_groups_identical_text_and_sorts_message_ids() {
        let groups = plan_embedding_groups(vec![
            SearchableMessage {
                id: "message-c".to_owned(),
                content: "same text".to_owned(),
            },
            SearchableMessage {
                id: "message-b".to_owned(),
                content: "different".to_owned(),
            },
            SearchableMessage {
                id: "message-a".to_owned(),
                content: "same text".to_owned(),
            },
        ]);

        assert_eq!(groups.len(), 2);
        let repeated = groups
            .iter()
            .find(|group| group.content == "same text")
            .expect("repeated text group");
        assert_eq!(repeated.message_ids, ["message-a", "message-c"]);
    }

    #[veritas::claims("semantic-indexing/batching-preserves-vectors")]
    #[test]
    fn embedding_plan_is_length_ordered_and_input_order_independent() {
        fn contents(groups: &[EmbeddingGroup]) -> Vec<&str> {
            groups
                .iter()
                .map(|group| group.content.as_str())
                .collect::<Vec<_>>()
        }

        fn messages(order: &[usize]) -> Vec<SearchableMessage> {
            let values = [
                ("message-long", "longest text"),
                ("message-short-b", "b"),
                ("message-short-a", "a"),
                ("message-medium", "medium"),
            ];
            order
                .iter()
                .map(|index| SearchableMessage {
                    id: values[*index].0.to_owned(),
                    content: values[*index].1.to_owned(),
                })
                .collect()
        }

        let forward = plan_embedding_groups(messages(&[0, 1, 2, 3]));
        let reverse = plan_embedding_groups(messages(&[3, 2, 1, 0]));

        assert_eq!(contents(&forward), ["a", "b", "medium", "longest text"]);
        assert_eq!(contents(&forward), contents(&reverse));
    }

    #[veritas::claims("semantic-indexing/batching-preserves-vectors")]
    #[test]
    #[ignore = "requires CASS_TEST_MODELS_DIR containing an explicit model installation"]
    fn real_model_length_aware_batches_preserve_quantized_vectors() {
        let models = std::env::var_os("CASS_TEST_MODELS_DIR")
            .map(PathBuf::from)
            .expect("explicit model directory");
        let mut backend = Backend::load_local(&models).expect("load reference models");
        let mut pool = EmbeddingPool::load_local(&models).expect("load embedding pool");
        let samples = [
            "brief status".to_owned(),
            "a somewhat longer explanation of a database transaction boundary".to_owned(),
            "x ".repeat(300),
            "semantic retrieval with reciprocal rank fusion and bounded reranking".to_owned(),
        ];

        let reference = samples
            .iter()
            .map(|text| {
                let vector = backend
                    .embed(&[text])
                    .expect("reference embedding")
                    .pop()
                    .expect("reference vector");
                (text.clone(), quantize_vector(&vector))
            })
            .collect::<HashMap<_, _>>();
        let messages = samples
            .iter()
            .rev()
            .enumerate()
            .map(|(index, text)| crate::storage::SearchableMessage {
                id: format!("message-{index}"),
                content: text.clone(),
            })
            .collect();
        let groups = plan_embedding_groups(messages);
        let candidate = pool.embed_wave(&groups).expect("length-aware embeddings");
        let repeated = pool
            .embed_wave(&groups)
            .expect("repeated length-aware embeddings");

        for ((group, vector), repeated_vector) in groups.iter().zip(candidate).zip(repeated) {
            let actual = quantize_vector(&vector);
            let repeated = quantize_vector(&repeated_vector);
            let expected = &reference[&group.content];
            let similarity = quantized_cosine(expected, &actual.values, actual.norm);
            assert!(
                similarity >= 0.98,
                "text length {} had reference cosine {similarity}",
                group.content.len()
            );
            assert_eq!(actual.values, repeated.values, "{}", group.content);
            assert_eq!(actual.norm, repeated.norm, "{}", group.content);
        }
    }

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
        let vectors = SemanticChunks {
            chunks: vec![crate::storage::SemanticChunk {
                message_rowids: vec![10, 20, 30],
                values: [first.values, second.values, third.values].concat(),
                norms: vec![first.norm, second.norm, third.norm],
                eligible: vec![true; 3],
                dimensions: 2,
            }],
            dimensions: 2,
        };

        let ranked = rank_quantized(&query, &vectors, 2);
        assert_eq!(
            ranked.iter().map(|(index, _)| *index).collect::<Vec<_>>(),
            [10, 20]
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
                origins: Vec::new(),
                federated_score: None,
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
