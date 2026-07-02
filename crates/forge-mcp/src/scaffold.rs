use std::path::{Path, PathBuf};

const FORGE_TOML_TEMPLATE: &str = r#"roots = ["decisions", "forces"]

[dedup]
reuse = 0.90  # cosine threshold: silently reuse existing force
warn = 0.75   # cosine threshold: warn about near-duplicate

[embedding]
# "fake-bucket" is a deterministic test embedder. The real model below
# downloads ~120MB from Hugging Face on first run, then uses the local cache.
model = "intfloat/multilingual-e5-small"
"#;

/// Scaffold a new forge corpus in `dir`: forge.toml + decisions/ + forces/.
/// Creates `dir` (and intermediates) if missing. Refuses to overwrite an
/// existing forge.toml. Returns the path of the created forge.toml.
pub fn init(dir: &Path) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir)
        .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let config_path = dir.join("forge.toml");
    if config_path.exists() {
        return Err(format!(
            "{} already exists; refusing to overwrite",
            config_path.display()
        ));
    }
    for sub in ["decisions", "forces"] {
        let d = dir.join(sub);
        std::fs::create_dir_all(&d)
            .map_err(|e| format!("cannot create {}: {e}", d.display()))?;
    }
    std::fs::write(&config_path, FORGE_TOML_TEMPLATE)
        .map_err(|e| format!("cannot write {}: {e}", config_path.display()))?;
    Ok(config_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_scaffolds_config_and_dirs_that_load() {
        let tmp = tempfile::tempdir().unwrap();
        let config_path = init(tmp.path()).unwrap();
        assert_eq!(config_path, tmp.path().join("forge.toml"));
        assert!(tmp.path().join("decisions").is_dir());
        assert!(tmp.path().join("forces").is_dir());
        // Load-bearing: Config::load canonicalizes roots and fails if the
        // directories are missing, so this proves the scaffold is coherent.
        let cfg = forge_core::config::Config::load(&config_path).unwrap();
        assert_eq!(cfg.embedding.model, "intfloat/multilingual-e5-small");
        assert_eq!(cfg.roots.len(), 2);
        assert!((cfg.dedup.reuse - 0.90).abs() < 1e-4);
        assert!((cfg.dedup.warn - 0.75).abs() < 1e-4);
    }

    #[test]
    fn init_creates_missing_target_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("new").join("nested");
        let config_path = init(&target).unwrap();
        assert!(config_path.is_file());
        assert!(target.join("decisions").is_dir());
    }

    #[test]
    fn init_refuses_existing_config() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("forge.toml"), "# existing").unwrap();
        let err = init(tmp.path()).unwrap_err();
        assert!(err.contains("already exists"), "got: {err}");
        let content = std::fs::read_to_string(tmp.path().join("forge.toml")).unwrap();
        assert_eq!(content, "# existing");
    }
}
