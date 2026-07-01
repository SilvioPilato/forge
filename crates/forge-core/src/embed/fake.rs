use super::{EmbedError, Embedder, Vector};
use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};

pub struct BucketEmbedder {
    model_id: String,
}

impl BucketEmbedder {
    pub fn new() -> Self {
        BucketEmbedder {
            model_id: "fake-bucket".to_string(),
        }
    }

    fn embed_text(&self, text: &str) -> Vector {
        let mut buckets = [0u32; 256];
        for word in text.to_lowercase().split(|c: char| !c.is_alphanumeric()) {
            if word.is_empty() {
                continue;
            }
            let mut hasher = DefaultHasher::new();
            word.hash(&mut hasher);
            let idx = (hasher.finish() % 256) as usize;
            buckets[idx] += 1;
        }
        let sum_sq: f32 = buckets.iter().map(|&c| (c as f32) * (c as f32)).sum();
        let norm = sum_sq.sqrt();
        if norm == 0.0 {
            vec![0.0f32; 256]
        } else {
            buckets.iter().map(|&c| c as f32 / norm).collect()
        }
    }
}

impl Default for BucketEmbedder {
    fn default() -> Self {
        Self::new()
    }
}

impl Embedder for BucketEmbedder {
    fn model_id(&self) -> &str {
        &self.model_id
    }
    fn embed_passages(&self, texts: &[String]) -> Result<Vec<Vector>, EmbedError> {
        Ok(texts.iter().map(|t| self.embed_text(t)).collect())
    }
    fn embed_query(&self, text: &str) -> Result<Vector, EmbedError> {
        Ok(self.embed_text(text))
    }
}

pub struct PinnedEmbedder {
    model_id: String,
    map: HashMap<String, Vector>,
    fallback: BucketEmbedder,
}

impl PinnedEmbedder {
    pub fn new() -> Self {
        PinnedEmbedder {
            model_id: "fake-pinned".to_string(),
            map: HashMap::new(),
            fallback: BucketEmbedder::new(),
        }
    }

    pub fn pin(&mut self, text: &str, vector: Vector) {
        self.map.insert(text.to_string(), vector);
    }
}

impl Default for PinnedEmbedder {
    fn default() -> Self {
        Self::new()
    }
}

impl Embedder for PinnedEmbedder {
    fn model_id(&self) -> &str {
        &self.model_id
    }
    fn embed_passages(&self, texts: &[String]) -> Result<Vec<Vector>, EmbedError> {
        texts
            .iter()
            .map(|t| {
                if let Some(v) = self.map.get(t) {
                    Ok(v.clone())
                } else {
                    self.fallback.embed_query(t)
                }
            })
            .collect()
    }
    fn embed_query(&self, text: &str) -> Result<Vector, EmbedError> {
        if let Some(v) = self.map.get(text) {
            Ok(v.clone())
        } else {
            self.fallback.embed_query(text)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::cosine;
    use super::*;

    #[test]
    fn bucket_embedder_is_deterministic() {
        let e = BucketEmbedder::default();
        let v1 = e.embed_query("rust is fast").unwrap();
        let v2 = e.embed_query("rust is fast").unwrap();
        assert_eq!(v1.len(), 256);
        assert_eq!(v1, v2);
    }

    #[test]
    fn bucket_similarity_tracks_token_overlap() {
        let e = BucketEmbedder::default();
        let a = e.embed_query("rust is fast and safe").unwrap();
        let b = e.embed_query("rust is fast").unwrap();
        let c = e.embed_query("yaml parsing rules").unwrap();
        let sim_ab = cosine(&a, &b);
        let sim_ac = cosine(&a, &c);
        assert!(
            sim_ab > sim_ac,
            "similar texts should have higher cosine similarity"
        );
    }

    #[test]
    fn pinned_embedder_returns_exact_vectors() {
        let mut e = PinnedEmbedder::new();
        e.pin("a", vec![1.0_f32, 0.0, 0.0]);
        e.pin("b", vec![0.96, 0.28, 0.0]);
        let va = e.embed_query("a").unwrap();
        let vb = e.embed_query("b").unwrap();
        assert_eq!(va, vec![1.0, 0.0, 0.0]);
        assert!((cosine(&va, &vb) - 0.96).abs() < 1e-6);
    }

    #[test]
    fn pinned_embedder_falls_back_to_bucket_for_unknown() {
        let mut e = PinnedEmbedder::new();
        e.pin("known", vec![1.0, 0.0]);
        let v1 = e.embed_query("known").unwrap();
        let v2 = e.embed_query("unknown text").unwrap();
        assert_ne!(v1, v2);
        assert!(!v2.is_empty());
    }

    #[test]
    fn cosine_edge_cases() {
        assert_eq!(cosine(&[], &[1.0, 2.0]), 0.0);
        assert_eq!(cosine(&[1.0, 2.0], &[]), 0.0);
        assert_eq!(cosine(&[0.0, 0.0], &[1.0, 2.0]), 0.0);
        let identical_a = vec![0.6, 0.8];
        let identical_b = vec![0.6, 0.8];
        assert!((cosine(&identical_a, &identical_b) - 1.0).abs() < 1e-6);
    }
}
