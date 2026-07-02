pub mod cache;
pub mod fake;

#[cfg(feature = "onnx")]
pub mod e5;

pub type Vector = Vec<f32>;

pub trait Embedder: Send + Sync {
    fn model_id(&self) -> &str;
    fn embed_passages(&self, texts: &[String]) -> Result<Vec<Vector>, EmbedError>;
    fn embed_query(&self, text: &str) -> Result<Vector, EmbedError>;
}

#[derive(Debug)]
pub struct EmbedError(pub String);

impl std::fmt::Display for EmbedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}
impl std::error::Error for EmbedError {}

pub struct NullEmbedder;

impl Embedder for NullEmbedder {
    fn model_id(&self) -> &str {
        "null"
    }
    fn embed_passages(&self, _texts: &[String]) -> Result<Vec<Vector>, EmbedError> {
        Ok(vec![vec![]; _texts.len()])
    }
    fn embed_query(&self, _text: &str) -> Result<Vector, EmbedError> {
        Ok(vec![])
    }
}

pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if na == 0.0 || nb == 0.0 {
        0.0
    } else {
        dot / (na * nb)
    }
}

pub fn default_embedder(cfg: &crate::config::Config) -> Result<Box<dyn Embedder>, EmbedError> {
    match cfg.embedding.model.as_str() {
        "fake-bucket" => Ok(Box::new(fake::BucketEmbedder::default())),
        "fake-pinned" => Ok(Box::new(fake::PinnedEmbedder::new())),
        #[cfg(feature = "onnx")]
        other => Ok(Box::new(e5::E5Embedder::load(&cfg.cache_dir, other)?)),
        #[cfg(not(feature = "onnx"))]
        other => Err(EmbedError(format!("unknown model '{}' (onnx feature not enabled)", other))),
    }
}
