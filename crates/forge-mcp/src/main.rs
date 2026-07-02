mod discover;
mod scaffold;
use forge_core::config::Config;
use forge_core::embed::default_embedder;
use forge_core::guardian::{Engine, ForceInput, ProposeInput};
use forge_core::recall::{search, Scope};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::{tool, tool_handler, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use tokio::sync::Mutex;

struct ForgeServer {
    engine: Mutex<Engine>,
}

impl ForgeServer {
    fn new(engine: Engine) -> Self {
        Self {
            engine: Mutex::new(engine),
        }
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
        let engine = self.engine.lock().await;
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
        let engine = self.engine.lock().await;
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
        let engine = self.engine.lock().await;
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
        let engine = self.engine.lock().await;
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
        let engine = self.engine.lock().await;
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
        description = "Commit a proposed decision to disk. Call only after the user has assented in conversation. Writes force and decision files, then synchronously rebuilds the index."
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
        let mut engine = self.engine.lock().await;
        match engine.commit(proposed) {
            Ok(receipt) => serde_json::to_string_pretty(&receipt).unwrap(),
            Err(e) => serde_json::to_string(&serde_json::json!({"error": e})).unwrap(),
        }
    }

    #[tool(
        description = "Set the status of a force or decision. Forces: changed or retired (forward-only). Decisions: deprecated. Returns which decisions became newly stale due to this change — this is the feedback to tell the user 'this wobbles N decisions.'"
    )]
    async fn set_status(&self, Parameters(params): Parameters<SetStatusParams>) -> String {
        let mut engine = self.engine.lock().await;
        match engine.set_status(&params.id, &params.status) {
            Ok(receipt) => serde_json::to_string_pretty(&receipt).unwrap(),
            Err(e) => serde_json::to_string(&serde_json::json!({"error": e})).unwrap(),
        }
    }
}

#[tool_handler]
impl rmcp::ServerHandler for ForgeServer {}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        anyhow::bail!("Usage: forge-mcp <path/to/forge.toml>");
    }

    let config_path = &args[1];
    let cfg = Config::load(std::path::Path::new(config_path))?;

    let embedder = match default_embedder(&cfg) {
        Ok(e) => e,
        Err(e) => {
            anyhow::bail!("Failed to create embedder: {}", e.0);
        }
    };

    let engine = Engine::new(cfg, embedder)
        .map_err(|e| anyhow::anyhow!("Failed to initialize engine: {}", e))?;

    let snap = engine.snapshot();
    eprintln!(
        "Loaded {} diagnostics, {} frontier decisions",
        snap.diagnostics.len(),
        snap.frontier().len()
    );

    let server = ForgeServer::new(engine);

    let (stdin, stdout) = rmcp::transport::io::stdio();
    rmcp::service::serve_server(server, (stdin, stdout))
        .await?
        .waiting()
        .await?;

    Ok(())
}
