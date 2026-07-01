use forge_core::config::Config;
use std::path::PathBuf;

pub fn corpus_dir() -> PathBuf {
    PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/corpus"
    ))
}

pub fn corpus_config() -> Config {
    Config::load(&corpus_dir().join("forge.toml")).unwrap()
}
