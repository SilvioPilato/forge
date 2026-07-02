use std::path::{Path, PathBuf};

use serde::Deserialize;

#[derive(Debug, Clone)]
pub struct Config {
    pub roots: Vec<PathBuf>,
    pub dedup: Dedup,
    pub embedding: Embedding,
    pub log: LogConfig,
    pub cache_dir: PathBuf,
    pub write_dir: PathBuf,
}

#[derive(Debug, Clone)]
pub struct Dedup {
    pub reuse: f32,
    pub warn: f32,
}

#[derive(Debug, Clone)]
pub struct Embedding {
    pub model: String,
}

#[derive(Debug, Clone)]
pub struct LogConfig {
    pub level: String,
    pub format: String,
    pub file: Option<PathBuf>,
}

impl Default for LogConfig {
    fn default() -> Self {
        Self {
            level: "info".to_string(),
            format: "pretty".to_string(),
            file: None,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("parse error: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("missing required field: roots")]
    MissingRoots,
    #[error("roots list is empty")]
    EmptyRoots,
    #[error("{0}")]
    Other(String),
}

#[derive(Debug, Deserialize)]
struct ConfigToml {
    roots: Option<Vec<String>>,
    #[serde(default)]
    dedup: DedupToml,
    #[serde(default)]
    embedding: EmbeddingToml,
    #[serde(default)]
    log: LogToml,
}

#[derive(Debug, Deserialize)]
#[serde(default)]
struct DedupToml {
    #[serde(default = "default_reuse")]
    reuse: f32,
    #[serde(default = "default_warn")]
    warn: f32,
}

impl Default for DedupToml {
    fn default() -> Self {
        Self {
            reuse: default_reuse(),
            warn: default_warn(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default)]
struct EmbeddingToml {
    #[serde(default = "default_model")]
    model: String,
}

impl Default for EmbeddingToml {
    fn default() -> Self {
        Self {
            model: default_model(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default)]
struct LogToml {
    #[serde(default = "default_log_level")]
    level: String,
    #[serde(default = "default_log_format")]
    format: String,
    file: Option<String>,
}

impl Default for LogToml {
    fn default() -> Self {
        Self {
            level: default_log_level(),
            format: default_log_format(),
            file: None,
        }
    }
}

fn default_log_level() -> String {
    "info".to_string()
}

fn default_log_format() -> String {
    "pretty".to_string()
}

fn default_reuse() -> f32 {
    0.90
}

fn default_warn() -> f32 {
    0.75
}

fn default_model() -> String {
    "intfloat/multilingual-e5-small".to_string()
}

impl Config {
    pub fn load(path: &Path) -> Result<Config, ConfigError> {
        let contents = std::fs::read_to_string(path)?;
        let file_dir = path
            .parent()
            .ok_or_else(|| ConfigError::Other("config file has no parent directory".to_string()))?;
        Config::from_str(&contents, file_dir)
    }

    pub(crate) fn from_str(s: &str, file_dir: &Path) -> Result<Config, ConfigError> {
        let parsed: ConfigToml = toml::from_str(s)?;

        let roots: Vec<String> = parsed.roots.ok_or(ConfigError::MissingRoots)?;
        if roots.is_empty() {
            return Err(ConfigError::EmptyRoots);
        }

        let roots: Vec<PathBuf> = roots.iter().map(|r| file_dir.join(r)).collect();
        let roots: Result<Vec<PathBuf>, _> = roots.into_iter().map(|r| r.canonicalize()).collect();
        let roots = roots.map_err(ConfigError::Io)?;

        let write_dir = roots.first().cloned().unwrap();

        let cache_dir = default_cache_dir();

        Ok(Config {
            roots,
            dedup: Dedup {
                reuse: parsed.dedup.reuse,
                warn: parsed.dedup.warn,
            },
            embedding: Embedding {
                model: parsed.embedding.model,
            },
            log: LogConfig {
                level: parsed.log.level,
                format: parsed.log.format,
                file: parsed.log.file.map(|f| file_dir.join(f)),
            },
            cache_dir,
            write_dir,
        })
    }
}

fn default_cache_dir() -> PathBuf {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".cache").join("forge")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_fixture_config_with_defaults() {
        let dir = std::path::Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/corpus"
        ));
        let cfg = Config::load(&dir.join("forge.toml")).unwrap();
        let expected_roots = vec![
            dir.join("decisions").canonicalize().unwrap(),
            dir.join("forces").canonicalize().unwrap(),
        ];
        assert_eq!(cfg.roots, expected_roots);
        assert!((cfg.dedup.reuse - 0.90).abs() < 1e-4);
        assert!((cfg.dedup.warn - 0.75).abs() < 1e-4);
        assert_eq!(cfg.embedding.model, "fake-bucket");
        assert_eq!(cfg.write_dir, dir.join("decisions").canonicalize().unwrap());
    }

    #[test]
    fn missing_roots_is_an_error() {
        let result = Config::from_str("[dedup]\nreuse=0.9", std::path::Path::new("."));
        let err = result.unwrap_err();
        assert!(matches!(err, ConfigError::MissingRoots));
    }

    #[test]
    fn loads_log_config_with_defaults() {
        let tmp = std::env::temp_dir();
        let root = tmp.join("forge-test-decisions");
        let _ = std::fs::create_dir_all(&root);
        let root_str = root.to_string_lossy().replace('\\', "/");
        let toml_str = format!(
            r#"
roots = ["{}"]
[log]
level = "debug"
format = "json"
file = "forge.log"
"#,
            root_str
        );
        let cfg = Config::from_str(&toml_str, std::path::Path::new("/tmp/test")).unwrap();
        assert_eq!(cfg.log.level, "debug");
        assert_eq!(cfg.log.format, "json");
        assert_eq!(
            cfg.log.file,
            Some(std::path::PathBuf::from("/tmp/test/forge.log"))
        );
    }

    #[test]
    fn log_config_has_sensible_defaults() {
        let tmp = std::env::temp_dir();
        let root = tmp.join("forge-test-decisions-default");
        let _ = std::fs::create_dir_all(&root);
        let root_str = root.to_string_lossy().replace('\\', "/");
        let toml_str = format!(r#"roots = ["{}"]"#, root_str);
        let cfg = Config::from_str(&toml_str, std::path::Path::new("/tmp/test")).unwrap();
        assert_eq!(cfg.log.level, "info");
        assert_eq!(cfg.log.format, "pretty");
        assert_eq!(cfg.log.file, None);
    }
}
