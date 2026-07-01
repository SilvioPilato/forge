use super::Vector;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::PathBuf;

pub struct VectorCache {
    dir: PathBuf,
    model_id: String,
    entries: HashMap<String, Vector>,
}

impl VectorCache {
    pub fn new(dir: PathBuf, model_id: &str) -> Self {
        let mut entries = HashMap::new();
        let file_path = dir.join(format!("{}.json", sanitize_filename(model_id)));
        if let Ok(data) = std::fs::read_to_string(&file_path) {
            if let Ok(deserialized) = serde_json::from_str::<HashMap<String, Vec<f32>>>(&data) {
                entries = deserialized;
            }
        }
        VectorCache {
            dir,
            model_id: model_id.to_string(),
            entries,
        }
    }

    pub fn content_hash(&self, text: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(text.as_bytes());
        format!("{:x}", hasher.finalize())
    }

    pub fn get(&self, hash: &str) -> Option<&Vector> {
        self.entries.get(hash)
    }

    pub fn put(&mut self, hash: &str, vector: &[f32]) {
        self.entries.insert(hash.to_string(), vector.to_vec());
    }

    pub fn save(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        let file_path = self
            .dir
            .join(format!("{}.json", sanitize_filename(&self.model_id)));
        let tmp_path = self
            .dir
            .join(format!("{}.json.tmp", sanitize_filename(&self.model_id)));
        let json = serde_json::to_string(&self.entries).unwrap();
        std::fs::write(&tmp_path, json)?;
        let _ = std::fs::remove_file(&file_path);
        std::fs::rename(&tmp_path, &file_path)?;
        Ok(())
    }
}

fn sanitize_filename(model_id: &str) -> String {
    model_id.replace(|c: char| !c.is_alphanumeric() && c != '-' && c != '_', "_")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_cache_dir(test_name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("forge-cache-test-{}", test_name));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn cache_round_trip() {
        let dir = temp_cache_dir("round_trip");
        let mut cache = VectorCache::new(PathBuf::from(&dir), "test-model");
        let hash = cache.content_hash("some text to embed");
        let vec = vec![1.0_f32, 2.0, 3.0];
        cache.put(&hash, &vec);
        assert_eq!(cache.get(&hash).unwrap(), &vec);
        cache.save().unwrap();
        let cache2 = VectorCache::new(PathBuf::from(&dir), "test-model");
        assert_eq!(cache2.get(&hash).unwrap(), &vec);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn different_model_id_misses() {
        let dir = temp_cache_dir("model_id");
        let mut cache = VectorCache::new(PathBuf::from(&dir), "model-a");
        let hash = cache.content_hash("text");
        cache.put(&hash, &[1.0]);
        cache.save().unwrap();
        let cache2 = VectorCache::new(PathBuf::from(&dir), "model-b");
        assert!(cache2.get(&hash).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn corrupt_cache_file_is_ignored_and_recomputed() {
        let dir = temp_cache_dir("corrupt");
        let file_path = dir.join("model-x.json");
        std::fs::write(&file_path, "not valid json").unwrap();
        let cache2 = VectorCache::new(PathBuf::from(&dir), "model-x");
        let hash = cache2.content_hash("hello");
        assert!(cache2.get(&hash).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
