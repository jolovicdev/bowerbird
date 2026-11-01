//! Search by meaning with EmbeddingGemma. Chunk vectors are filled in the
//! background after a source is indexed; until then, and without the model,
//! search is by keyword alone. The model is downloaded once, on first use.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use fastembed::{
    InitOptionsUserDefined, OutputKey, Pooling, TextEmbedding, TokenizerFiles,
    UserDefinedEmbeddingModel,
};

use crate::index::{ChunkRow, Index};
use crate::{Error, Result, download, settings};

/// A fixed revision, so a change upstream can't swap the model under stored vectors.
const REPOSITORY: &str = "https://huggingface.co/onnx-community/embeddinggemma-300m-ONNX/resolve/5090578d9565bb06545b4552f76e6bc2c93e4a66";
const MODEL: &str = "onnx/model_q4.onnx";
const WEIGHTS: &str = "onnx/model_q4.onnx_data";
const TOKENIZER: [&str; 4] = [
    "tokenizer.json",
    "config.json",
    "special_tokens_map.json",
    "tokenizer_config.json",
];
/// Every file the model needs, with its SHA-256.
const FILES: [(&str, &str); 6] = [
    (
        MODEL,
        "ad1dfee81a70f7944b9b9d1cc6e48075b832881cf33fab2f2b248be78f3f0043",
    ),
    (
        WEIGHTS,
        "599962c3143b040de2dd05e5975be3e9091dd067cacc6a8f7186e3203bab9e02",
    ),
    (
        TOKENIZER[0],
        "4dda02faaf32bc91031dc8c88457ac272b00c1016cc679757d1c441b248b9c47",
    ),
    (
        TOKENIZER[1],
        "6e1f06404b7163e0325ed2ea3e6781cde50f4a50b31780a95ad0d30e8404d77b",
    ),
    (
        TOKENIZER[2],
        "2f7b0adf4fb469770bb1490e3e35df87b1dc578246c5e7e6fc76ecf33213a397",
    ),
    (
        TOKENIZER[3],
        "3ca953eea6c3c9fcda9cf3df22949ff18b216f7c74bd6459230f3f1013953f3a",
    ),
];
/// Chunks embedded per call. Small, so a question waiting for the model
/// isn't held up long by a fill in progress.
pub const BATCH: usize = 8;

static SHARED: Mutex<Option<Arc<Embedder>>> = Mutex::new(None);

pub fn dir() -> PathBuf {
    settings::models_dir().join("embeddinggemma")
}

/// Whether the model is already in `dir`.
pub fn ready(dir: &Path) -> bool {
    FILES.iter().all(|(file, _)| dir.join(file).is_file())
}

/// Downloads the model into `dir`, reporting as `download::files`.
pub async fn download(dir: &Path, progress: impl Fn(u64, u64)) -> Result<()> {
    download::files(dir, REPOSITORY, &FILES, progress).await
}

pub struct Embedder(Mutex<TextEmbedding>);

impl Embedder {
    pub fn load(dir: &Path) -> Result<Embedder> {
        let read = |file: &str| std::fs::read(dir.join(file));
        let [
            tokenizer_file,
            config_file,
            special_tokens_map_file,
            tokenizer_config_file,
        ] = TOKENIZER.map(read);
        let tokenizer = TokenizerFiles {
            tokenizer_file: tokenizer_file?,
            config_file: config_file?,
            special_tokens_map_file: special_tokens_map_file?,
            tokenizer_config_file: tokenizer_config_file?,
        };
        let weights_name = Path::new(WEIGHTS)
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let mut model = UserDefinedEmbeddingModel::new(read(MODEL)?, tokenizer)
            .with_pooling(Pooling::Mean)
            .with_external_initializer(weights_name, read(WEIGHTS)?);
        model.output_key = Some(OutputKey::ByName("sentence_embedding"));
        let model = TextEmbedding::try_new_from_user_defined(model, InitOptionsUserDefined::new())
            .map_err(|e| Error::Model(format!("The search model failed to load: {e}")))?;
        Ok(Embedder(Mutex::new(model)))
    }

    /// The model loaded once per process, or None until it is downloaded.
    pub fn shared() -> Option<Arc<Embedder>> {
        let mut shared = SHARED.lock().unwrap_or_else(PoisonError::into_inner);
        if shared.is_none() && ready(&dir()) {
            match Embedder::load(&dir()) {
                Ok(embedder) => *shared = Some(Arc::new(embedder)),
                Err(e) => eprintln!("{e}"),
            }
        }
        shared.clone()
    }

    fn embed(&self, texts: Vec<String>) -> Result<Vec<Vec<f32>>> {
        let mut model = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let mut vectors = model
            .embed(texts, Some(BATCH))
            .map_err(|e| Error::Model(e.to_string()))?;
        vectors.iter_mut().for_each(|v| normalize(v));
        Ok(vectors)
    }

    /// EmbeddingGemma was trained with these prompts for queries and documents.
    pub fn query(&self, question: &str) -> Result<Vec<f32>> {
        Ok(self
            .embed(vec![format!("task: search result | query: {question}")])?
            .remove(0))
    }

    pub fn documents(&self, rows: &[ChunkRow]) -> Result<Vec<Vec<f32>>> {
        self.embed(
            rows.iter()
                .map(|r| format!("title: none | text: {}", r.text))
                .collect(),
        )
    }
}

/// Embeds every chunk of the index that has no vector yet, reporting
/// (embedded, total) after each batch.
pub fn fill(index: &mut Index, embedder: &Embedder, progress: impl Fn(u64, u64)) -> Result<()> {
    loop {
        let rows = index.unembedded(BATCH)?;
        if rows.is_empty() {
            return Ok(());
        }
        let vectors = embedder.documents(&rows)?;
        index.store_vectors(&rows, &vectors)?;
        progress(index.vector_count()?, index.chunk_count()?);
    }
}

fn normalize(v: &mut [f32]) {
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        v.iter_mut().for_each(|x| *x /= norm);
    }
}

/// Downloads the model for a live test. Tests run in parallel, and two
/// downloads of one file write the same partial file, so the slower one
/// finds it already renamed; the tests take turns instead.
#[cfg(test)]
pub(crate) async fn download_for_tests() {
    static TURN: futures::lock::Mutex<()> = futures::lock::Mutex::new(());
    let _turn = TURN.lock().await;
    download(&dir(), |_, _| {}).await.unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(text: &str) -> ChunkRow {
        ChunkRow {
            id: 0,
            source_id: String::new(),
            ord: 0,
            start: 0,
            page_start: None,
            page_end: None,
            section: String::new(),
            text: text.into(),
        }
    }

    /// Downloads the model (about 200 MB), so it runs only with BOWERBIRD_LIVE=1.
    #[tokio::test]
    async fn the_search_model_downloads_and_ranks_by_meaning() {
        if std::env::var("BOWERBIRD_LIVE").as_deref() != Ok("1") {
            eprintln!("skipped: set BOWERBIRD_LIVE=1 to download the search model");
            return;
        }
        download_for_tests().await;
        let embedder = Embedder::load(&dir()).unwrap();
        let docs = embedder
            .documents(&[
                row("The male gathers blue bottle caps and berries to decorate his bower."),
                row("Value added tax is charged at each stage of production."),
            ])
            .unwrap();
        let query = embedder
            .query("Which colours does the bird collect?")
            .unwrap();
        let score = |v: &[f32]| v.iter().zip(&query).map(|(a, b)| a * b).sum::<f32>();
        assert!(score(&docs[0]) > score(&docs[1]) + 0.1);
        assert!((score(&query) - 1.0).abs() < 1e-3, "vectors are normalized");
    }
}
