use forge_core::config::Config;
use forge_core::embed::default_embedder;
use forge_core::guardian::Engine;
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
