mod discover;
mod scaffold;
use forge_core::config::Config;
use forge_core::embed::default_embedder;
use forge_core::guardian::{Engine, ForceInput, ProposeInput};
use forge_core::logging::init_subscriber;
use forge_core::recall::{search, Scope};
use tracing::info;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::{tool, tool_handler, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use tokio::sync::Mutex;
use anyhow::Context;
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::sync::Arc;

enum EngineState {
    Empty,
    Loading,
    Ready(Engine),
    Failed(String),
}

struct ForgeServer {
    state: Arc<Mutex<EngineState>>,
    project_dir: PathBuf,
}

impl ForgeServer {
    fn new(state: Arc<Mutex<EngineState>>, project_dir: PathBuf) -> Self {
        Self { state, project_dir }
    }

    fn no_corpus_hits() -> String {
        serde_json::to_string(&serde_json::json!({
            "hits": [],
            "hint": "No forge.toml in this project. Call the `init` tool (after user assent) to scaffold a corpus.",
        }))
        .unwrap()
    }

    fn no_corpus_not_found() -> String {
        serde_json::to_string(&serde_json::json!({
            "error": "not found",
            "hint": "No forge.toml in this project. Call the `init` tool (after user assent) to scaffold a corpus.",
        }))
        .unwrap()
    }

    fn no_corpus_stale_report() -> String {
        serde_json::to_string(&serde_json::json!({
            "stale": [],
            "diagnostics_summary": {"count": 0, "kinds": []},
            "hint": "No forge.toml in this project. Call the `init` tool (after user assent) to scaffold a corpus.",
        }))
        .unwrap()
    }

    fn no_corpus_write_refused() -> String {
        serde_json::to_string(&serde_json::json!({
            "error": "no corpus; call `init` first (after user assent)",
        }))
        .unwrap()
    }

    fn loading_response() -> String {
        serde_json::to_string(&serde_json::json!({
            "status": "loading",
            "hint": "The corpus is still loading (the embedding model may be downloading). Retry in a few seconds.",
        }))
        .unwrap()
    }

    fn failed_response(err: &str) -> String {
        serde_json::to_string(&serde_json::json!({
            "error": format!("corpus failed to load: {err}"),
            "hint": "Fix the problem and restart the forge-mcp server.",
        }))
        .unwrap()
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct SearchParams {
    query: String,
    #[serde(default)]
    scope: Option<String>,
    #[serde(default = "default_limit")]
    limit: u32,
}

fn default_limit() -> u32 {
    10
}

#[derive(Debug, Deserialize, JsonSchema)]
struct GetParams {
    id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct WhyParams {
    id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct ProposeParams {
    title: String,
    body: Option<String>,
    forces: Vec<ForceSpec>,
    supersedes: Option<Vec<String>>,
    relates: Option<Vec<String>>,
    tags: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(untagged)]
enum ForceSpec {
    New {
        title: String,
        body: Option<String>,
        force_new: Option<bool>,
    },
    Existing {
        existing_id: String,
    },
}

#[derive(Debug, Deserialize, JsonSchema)]
struct CommitParams {
    proposed: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct SetStatusParams {
    id: String,
    status: String,
}

#[tool_router]
impl ForgeServer {
    #[tool(
        description = "Search for decisions and forces on the frontier by semantic similarity. Scope can be 'force', 'decision', or 'both' (default)."
    )]
    async fn search(&self, Parameters(params): Parameters<SearchParams>) -> String {
        let state = self.state.lock().await;
        let engine = match &*state {
            EngineState::Ready(e) => e,
            EngineState::Empty => return Self::no_corpus_hits(),
            EngineState::Loading => return Self::loading_response(),
            EngineState::Failed(err) => return Self::failed_response(err),
        };
        let snap = engine.snapshot();
        let scope = match params.scope.as_deref() {
            Some("force") => Scope::Force,
            Some("decision") => Scope::Decision,
            _ => Scope::Both,
        };

        let embedder = match forge_core::embed::default_embedder(&engine.cfg) {
            Ok(e) => e,
            Err(e) => {
                return serde_json::to_string(
                    &serde_json::json!({"error": format!("embedder error: {}", e.0)}),
                )
                .unwrap()
            }
        };

        match search(
            &snap,
            embedder.as_ref(),
            &params.query,
            scope,
            params.limit as usize,
        ) {
            Ok(hits) => serde_json::to_string_pretty(&serde_json::json!({
                "hits": hits
                    .iter()
                    .map(|h| {
                        serde_json::json!({
                            "id": h.id,
                            "title": h.title,
                            "score": h.score,
                            "kind": h.kind,
                            "status": h.status,
                        })
                    })
                    .collect::<Vec<_>>()
            }))
            .unwrap(),
            Err(e) => {
                serde_json::to_string(&serde_json::json!({"error": format!("{}", e.0)})).unwrap()
            }
        }
    }

    #[tool(
        description = "Get a decision or force record by ID, including its neighborhood (edges both ways) and verdict."
    )]
    async fn get(&self, Parameters(params): Parameters<GetParams>) -> String {
        let state = self.state.lock().await;
        let engine = match &*state {
            EngineState::Ready(e) => e,
            EngineState::Empty => return Self::no_corpus_not_found(),
            EngineState::Loading => return Self::loading_response(),
            EngineState::Failed(err) => return Self::failed_response(err),
        };
        let snap = engine.snapshot();
        match snap.graph.get(&params.id) {
            Some(record) => {
                let verdict = snap.verdicts.premise.get(&params.id);
                let is_superseded = snap.verdicts.superseded.contains(&params.id);
                let on_frontier = snap.frontier().contains(&params.id);

                let result = match record {
                    forge_core::graph::Record::Decision(d) => serde_json::json!({
                        "id": d.id,
                        "type": "decision",
                        "title": d.title,
                        "status": format!("{:?}", d.status).to_lowercase(),
                        "date": d.date,
                        "cites": d.cites,
                        "supersedes": d.supersedes,
                        "body": d.body,
                        "verdict": verdict.map(|v| format!("{:?}", v)),
                        "superseded": is_superseded,
                        "on_frontier": on_frontier,
                    }),
                    forge_core::graph::Record::Force(f) => serde_json::json!({
                        "id": f.id,
                        "type": "force",
                        "title": f.title,
                        "current_status": format!("{:?}", f.current_status()).to_lowercase(),
                        "depends_on": f.depends_on,
                        "superseded_by": f.superseded_by,
                        "body": f.body,
                    }),
                };
                serde_json::to_string_pretty(&result).unwrap()
            }
            None => serde_json::to_string(&serde_json::json!({"error": "not found"})).unwrap(),
        }
    }

    #[tool(
        description = "Explain why a decision's premises are stale. Shows the dependency chain from the decision back to each fallen force."
    )]
    async fn why(&self, Parameters(params): Parameters<WhyParams>) -> String {
        let state = self.state.lock().await;
        let engine = match &*state {
            EngineState::Ready(e) => e,
            EngineState::Empty => return Self::no_corpus_not_found(),
            EngineState::Loading => return Self::loading_response(),
            EngineState::Failed(err) => return Self::failed_response(err),
        };
        let snap = engine.snapshot();
        match snap.why(&params.id) {
            Some(chain) => serde_json::to_string_pretty(&serde_json::json!({
                "chain": chain
                    .iter()
                    .map(|e| {
                        serde_json::json!({
                            "force_id": e.force_id,
                            "force_title": e.force_title,
                            "current_status": e.current_status,
                            "path": e.path,
                        })
                    })
                    .collect::<Vec<_>>()
            }))
            .unwrap(),
            None => serde_json::to_string(&serde_json::json!({"error": "not found"})).unwrap(),
        }
    }

    #[tool(
        description = "Report all stale frontier decisions, ordered by severity (retired before changed) then by distance."
    )]
    async fn stale_report(&self) -> String {
        let state = self.state.lock().await;
        let engine = match &*state {
            EngineState::Ready(e) => e,
            EngineState::Empty => return Self::no_corpus_stale_report(),
            EngineState::Loading => return Self::loading_response(),
            EngineState::Failed(err) => return Self::failed_response(err),
        };
        let snap = engine.snapshot();
        let report = snap.stale_report();
        serde_json::to_string_pretty(&serde_json::json!({
            "stale": report
                .iter()
                .map(|e| {
                    serde_json::json!({
                        "decision_id": e.decision_id,
                        "decision_title": e.decision_title,
                        "fallen_force_id": e.fallen_force_id,
                        "fallen_status": e.fallen_status,
                        "distance": e.distance,
                        "since": e.since,
                    })
                })
                .collect::<Vec<_>>(),
            "diagnostics_summary": {
                "count": snap.diagnostics.len(),
                "kinds": snap
                    .diagnostics
                    .iter()
                    .map(|d| format!("{:?}", d))
                    .collect::<Vec<_>>(),
            }
        }))
        .unwrap()
    }

    #[tool(
        description = "Propose a new decision with its supporting forces. This is PURE — it does not write any files. Use this freely to preview what would be created. Forces can be new (with title/body) or existing (by id). Returns the composed records, any validation problems, and near-duplicate force matches."
    )]
    async fn propose_decision(&self, Parameters(params): Parameters<ProposeParams>) -> String {
        let state = self.state.lock().await;
        let engine = match &*state {
            EngineState::Ready(e) => e,
            EngineState::Empty => return Self::no_corpus_write_refused(),
            EngineState::Loading => return Self::loading_response(),
            EngineState::Failed(err) => return Self::failed_response(err),
        };
        let input = ProposeInput {
            title: params.title,
            body: params.body.unwrap_or_default(),
            forces: params
                .forces
                .into_iter()
                .map(|f| match f {
                    ForceSpec::New {
                        title,
                        body,
                        force_new,
                    } => ForceInput::New {
                        title,
                        body: body.unwrap_or_default(),
                        force_new: force_new.unwrap_or(false),
                    },
                    ForceSpec::Existing { existing_id } => ForceInput::Existing { id: existing_id },
                })
                .collect(),
            supersedes: params.supersedes.unwrap_or_default(),
            relates: params.relates.unwrap_or_default(),
            tags: params.tags.unwrap_or_default(),
        };
        match engine.propose_decision(input) {
            Ok(proposed) => serde_json::to_string_pretty(&proposed).unwrap(),
            Err(e) => {
                serde_json::to_string(&serde_json::json!({"error": format!("{}", e.0)})).unwrap()
            }
        }
    }

    #[tool(
        description = "Commit a proposed decision to disk. Call only after the user has assented in conversation. Writes force and decision files, then synchronously rebuilds the index. To backfill a historical decision, edit `decision.date` (YYYY-MM-DD) on the proposed object before committing — it is the only honored edit; anything else requires a fresh propose_decision."
    )]
    async fn commit(&self, Parameters(params): Parameters<CommitParams>) -> String {
        let proposed: forge_core::guardian::Proposed =
            match serde_json::from_value(serde_json::Value::Object(params.proposed)) {
                Ok(p) => p,
                Err(e) => {
                    return serde_json::to_string(
                        &serde_json::json!({"error": format!("invalid proposed: {}", e)}),
                    )
                    .unwrap()
                }
            };
        let mut state = self.state.lock().await;
        let engine = match &mut *state {
            EngineState::Ready(e) => e,
            EngineState::Empty => return Self::no_corpus_write_refused(),
            EngineState::Loading => return Self::loading_response(),
            EngineState::Failed(err) => return Self::failed_response(&err.clone()),
        };
        match engine.commit(proposed) {
            Ok(receipt) => serde_json::to_string_pretty(&receipt).unwrap(),
            Err(e) => serde_json::to_string(&serde_json::json!({"error": e})).unwrap(),
        }
    }

    #[tool(
        description = "Set the status of a force or decision. Forces: changed or retired (forward-only). Decisions: deprecated. Returns which decisions became newly stale due to this change — this is the feedback to tell the user 'this wobbles N decisions.'"
    )]
    async fn set_status(&self, Parameters(params): Parameters<SetStatusParams>) -> String {
        let mut state = self.state.lock().await;
        let engine = match &mut *state {
            EngineState::Ready(e) => e,
            EngineState::Empty => return Self::no_corpus_write_refused(),
            EngineState::Loading => return Self::loading_response(),
            EngineState::Failed(err) => return Self::failed_response(&err.clone()),
        };
        match engine.set_status(&params.id, &params.status) {
            Ok(receipt) => serde_json::to_string_pretty(&receipt).unwrap(),
            Err(e) => serde_json::to_string(&serde_json::json!({"error": e})).unwrap(),
        }
    }

    #[tool(
        description = "Re-scan the corpus roots from disk and rebuild the index. Use after manually editing or adding record files so get/search reflect the on-disk state."
    )]
    async fn reindex(&self) -> String {
        let mut state = self.state.lock().await;
        let engine = match &mut *state {
            EngineState::Ready(e) => e,
            EngineState::Empty => return Self::no_corpus_write_refused(),
            EngineState::Loading => return Self::loading_response(),
            EngineState::Failed(err) => return Self::failed_response(&err.clone()),
        };
        match engine.rebuild() {
            Ok(()) => {
                let snap = engine.snapshot();
                serde_json::to_string_pretty(&serde_json::json!({
                    "status": "reindexed",
                    "decisions": snap.graph.decisions().len(),
                    "forces": snap.graph.forces().len(),
                    "diagnostics": snap.diagnostics.len(),
                }))
                .unwrap()
            }
            Err(e) => serde_json::to_string(&serde_json::json!({"error": e})).unwrap(),
        }
    }

    #[tool(
        description = "Scaffold a forge corpus (forge.toml + decisions/ + forces/) in this project's root and load it. Call only after the user has assented. Refuses to overwrite an existing forge.toml."
    )]
    async fn init(&self) -> String {
        let mut state = self.state.lock().await;
        match &*state {
            EngineState::Ready(_) => {
                return serde_json::to_string(&serde_json::json!({
                    "status": "already loaded",
                }))
                .unwrap()
            }
            EngineState::Loading => return Self::loading_response(),
            EngineState::Empty | EngineState::Failed(_) => {}
        }

        let config_path = match scaffold::ensure_corpus(&self.project_dir) {
            Ok(p) => p,
            Err(e) => {
                return serde_json::to_string(&serde_json::json!({
                    "error": format!("failed to scaffold corpus: {e}"),
                }))
                .unwrap();
            }
        };

        let cfg = match Config::load(&config_path) {
            Ok(c) => c,
            Err(e) => {
                return serde_json::to_string(&serde_json::json!({
                    "error": format!("failed to load config: {e}"),
                }))
                .unwrap();
            }
        };

        let embedder = match default_embedder(&cfg) {
            Ok(e) => e,
            Err(e) => {
                return serde_json::to_string(&serde_json::json!({
                    "error": format!("failed to create embedder: {}", e.0),
                }))
                .unwrap();
            }
        };

        let new_engine = match Engine::new(cfg, embedder) {
            Ok(e) => e,
            Err(e) => {
                return serde_json::to_string(&serde_json::json!({
                    "error": format!("failed to initialize engine: {}", e),
                }))
                .unwrap();
            }
        };

        let snap = new_engine.snapshot();
        info!(
            diagnostics = snap.diagnostics.len(),
            frontier = snap.frontier().len(),
            "forge-mcp corpus loaded via init tool"
        );
        *state = EngineState::Ready(new_engine);
        serde_json::to_string_pretty(&serde_json::json!({
            "status": "loaded",
            "config": config_path.display().to_string(),
        }))
        .unwrap()
    }
}

#[tool_handler]
impl rmcp::ServerHandler for ForgeServer {
    fn get_info(&self) -> rmcp::model::ServerInfo {
        #[allow(deprecated)]
        let capabilities = rmcp::model::ServerCapabilities::builder()
            .enable_tools()
            .enable_logging()
            .build();
        rmcp::model::ServerInfo::new(capabilities)
    }
}

#[derive(Parser)]
#[command(name = "forge-mcp", version, about = "Forge MCP server over stdio")]
struct Cli {
    /// Path to forge.toml (same as --config; kept for backward compatibility)
    #[arg(value_name = "CONFIG", conflicts_with = "config")]
    positional_config: Option<PathBuf>,

    /// Path to forge.toml
    #[arg(long, value_name = "PATH")]
    config: Option<PathBuf>,

    #[command(subcommand)]
    command: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Scaffold a new forge corpus (forge.toml + decisions/ + forces/)
    Init {
        /// Target directory (default: current directory)
        dir: Option<PathBuf>,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    if let Some(Cmd::Init { dir }) = cli.command {
        let target = match dir {
            Some(d) => d,
            None => std::env::current_dir()?,
        };
        let config_path = scaffold::init(&target).map_err(|e| anyhow::anyhow!(e))?;
        println!("Scaffolded forge corpus: {}", config_path.display());

        let cfg = Config::load(&config_path).with_context(|| {
            format!("failed to load scaffolded config at {}", config_path.display())
        })?;
        println!(
            "Fetching embedding model '{}' (skipped if already cached)...",
            cfg.embedding.model
        );
        match forge_core::embed::prefetch_model(&cfg, true) {
            Ok(()) => println!("Embedding model ready."),
            Err(e) => eprintln!(
                "warning: model prefetch failed: {e}\n\
                 The model will be downloaded when the server first starts."
            ),
        }
        return Ok(());
    }

    // conflicts_with guarantees at most one of these is Some
    let explicit = cli.config.or(cli.positional_config);
    let env_value = std::env::var_os("FORGE_CONFIG").map(PathBuf::from);
    let cwd = std::env::current_dir()?;

    let (server, pending) = match discover::resolve_config(explicit, env_value, &cwd) {
        Some(config_path) => {
            let cfg = Config::load(&config_path)
                .with_context(|| format!("failed to load config at {}", config_path.display()))?;
            init_subscriber(&cfg.log.level, &cfg.log.format, cfg.log.file.as_ref());
            info!("forge-mcp starting; corpus will load in the background");
            let state = Arc::new(Mutex::new(EngineState::Loading));
            (
                ForgeServer::new(state.clone(), cwd),
                Some((cfg, state)),
            )
        }
        None => {
            init_subscriber("info", "compact", None);
            info!("forge-mcp starting in empty mode (no forge.toml found)");
            (
                ForgeServer::new(Arc::new(Mutex::new(EngineState::Empty)), cwd),
                None,
            )
        }
    };

    let (stdin, stdout) = rmcp::transport::io::stdio();
    let running = rmcp::service::serve_server(server, (stdin, stdout)).await?;

    if let Some((cfg, state)) = pending {
        let peer = running.peer().clone();
        tokio::spawn(async move {
            if let Some(ms) = std::env::var("FORGE_TEST_LOAD_DELAY_MS")
                .ok()
                .and_then(|v| v.parse::<u64>().ok())
            {
                tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
            }

            let result = tokio::task::spawn_blocking(move || {
                let embedder = default_embedder(&cfg).map_err(|e| e.0)?;
                Engine::new(cfg, embedder)
            })
            .await
            .unwrap_or_else(|e| Err(format!("engine loader panicked: {e}")));

            let mut st = state.lock().await;
            match result {
                Ok(engine) => {
                    let snap = engine.snapshot();
                    info!(
                        diagnostics = snap.diagnostics.len(),
                        frontier = snap.frontier().len(),
                        "forge-mcp corpus ready"
                    );
                    *st = EngineState::Ready(engine);

                    #[allow(deprecated)]
                    let _ = peer
                        .notify_logging_message(
                            rmcp::model::LoggingMessageNotificationParam::new(
                                rmcp::model::LoggingLevel::Info,
                                serde_json::json!({"event": "corpus_ready"}),
                            )
                            .with_logger("forge"),
                        )
                        .await;
                }
                Err(e) => {
                    tracing::error!(error = %e, "forge-mcp corpus failed to load");
                    *st = EngineState::Failed(e.clone());

                    #[allow(deprecated)]
                    let _ = peer
                        .notify_logging_message(
                            rmcp::model::LoggingMessageNotificationParam::new(
                                rmcp::model::LoggingLevel::Error,
                                serde_json::json!({"event": "corpus_load_failed", "error": e}),
                            )
                            .with_logger("forge"),
                        )
                        .await;
                }
            }
        });
    }

    running.waiting().await?;
    Ok(())
}
