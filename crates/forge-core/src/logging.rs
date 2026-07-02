use std::path::PathBuf;
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::layer::{Layer, SubscriberExt};
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;

pub fn init_subscriber(level: &str, format: &str, log_file: Option<&PathBuf>) {
    let env_filter = EnvFilter::builder()
        .with_default_directive(
            level
                .parse::<LevelFilter>()
                .unwrap_or(LevelFilter::INFO)
                .into(),
        )
        .from_env_lossy();

    let format_is_json = std::env::var("FORGE_LOG_FORMAT")
        .map(|v| v.to_lowercase() == "json")
        .unwrap_or(format == "json");

    let stderr_layer = if format_is_json {
        tracing_subscriber::fmt::layer()
            .with_writer(std::io::stderr)
            .json()
            .boxed()
    } else {
        tracing_subscriber::fmt::layer()
            .with_writer(std::io::stderr)
            .compact()
            .boxed()
    };

    let file_layer = log_file.and_then(|path| {
        let file = match std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            Ok(f) => f,
            Err(e) => {
                eprintln!("Failed to open log file {}: {}", path.display(), e);
                return None;
            }
        };
        Some(
            tracing_subscriber::fmt::layer()
                .with_writer(file)
                .json()
                .boxed(),
        )
    });

    tracing_subscriber::registry()
        .with(env_filter)
        .with(stderr_layer)
        .with(file_layer)
        .try_init()
        .unwrap_or_else(|e| eprintln!("tracing subscriber already set: {e}"));
}
