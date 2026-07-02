use std::env;
use std::process;

use forge_core::config::Config;
use forge_core::logging::init_subscriber;
use forge_core::recall::{search, Scope};
use forge_core::snapshot::Snapshot;
use serde::Serialize;
use tracing::error;

#[derive(Serialize)]
struct InspectReport {
    diagnostics_count: usize,
    diagnostics: Vec<DiagnosticEntry>,
    stale_report: Vec<StaleReportEntry>,
    frontier: Vec<String>,
}

#[derive(Serialize)]
struct DiagnosticEntry {
    kind: String,
    detail: String,
}

#[derive(Serialize)]
struct StaleReportEntry {
    decision_id: String,
    decision_title: String,
    fallen_force_id: String,
    fallen_status: String,
    distance: u32,
    since: String,
}

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        init_subscriber("info", "pretty", None);
        error!("Usage: forge-inspect <path/to/forge.toml> [--json] [--search \"query\" [--scope force|decision|both]]");
        process::exit(1);
    }

    let config_path = &args[1];
    let as_json = args.iter().any(|a| a == "--json");

    let search_idx = args.iter().position(|a| a == "--search");
    let scope_idx = args.iter().position(|a| a == "--scope");

    let search_query = search_idx.and_then(|i| args.get(i + 1).cloned());
    let scope_str = scope_idx.and_then(|i| args.get(i + 1).cloned());

    let scope = match scope_str.as_deref() {
        Some("force") => Scope::Force,
        Some("decision") => Scope::Decision,
        Some("both") | None => Scope::Both,
        Some(other) => {
            error!(scope = %other, "Unknown scope. Use force, decision, or both.");
            process::exit(1);
        }
    };

    let cfg = Config::load(std::path::Path::new(config_path)).unwrap_or_else(|e| {
        error!("Failed to load config: {}", e);
        process::exit(1);
    });
    init_subscriber(&cfg.log.level, &cfg.log.format, cfg.log.file.as_ref());

    if let Some(query) = search_query {
        let embedder = forge_core::embed::default_embedder(&cfg).unwrap_or_else(|e| {
            error!("Failed to create embedder: {}", e);
            process::exit(1);
        });
        let snap = Snapshot::build(&cfg, embedder.as_ref()).unwrap_or_else(|e| {
            error!("Failed to build snapshot: {}", e);
            process::exit(1);
        });
        let hits = search(&snap, embedder.as_ref(), &query, scope, 20).unwrap_or_else(|e| {
            error!("Search failed: {}", e);
            process::exit(1);
        });
        if as_json {
            println!("{}", serde_json::to_string_pretty(&hits).unwrap());
        } else if hits.is_empty() {
            println!("No results.");
        } else {
            println!("{:<20} {:<50} {:<8} {:<10}", "ID", "TITLE", "SCORE", "KIND");
            for h in &hits {
                println!(
                    "{:<20} {:<50} {:<8.4} {:<10}",
                    h.id, h.title, h.score, h.kind
                );
            }
        }
        return;
    }

    let embedder = forge_core::embed::NullEmbedder;
    let snap = Snapshot::build(&cfg, &embedder).unwrap_or_else(|e| {
        error!("Failed to build snapshot: {}", e);
        process::exit(1);
    });

    if as_json {
        let report = InspectReport {
            diagnostics_count: snap.diagnostics.len(),
            diagnostics: snap
                .diagnostics
                .iter()
                .map(|d| DiagnosticEntry {
                    kind: format!("{:?}", d),
                    detail: format!("{:?}", d),
                })
                .collect(),
            stale_report: snap
                .stale_report()
                .iter()
                .map(|e| StaleReportEntry {
                    decision_id: e.decision_id.clone(),
                    decision_title: e.decision_title.clone(),
                    fallen_force_id: e.fallen_force_id.clone(),
                    fallen_status: e.fallen_status.clone(),
                    distance: e.distance,
                    since: e.since.clone(),
                })
                .collect(),
            frontier: snap.frontier().to_vec(),
        };
        println!("{}", serde_json::to_string_pretty(&report).unwrap());
    } else {
        println!("Diagnostics ({}):", snap.diagnostics.len());
        for d in &snap.diagnostics {
            println!("  {:?}", d);
        }
        println!();
        println!("Stale report:");
        for e in snap.stale_report() {
            println!(
                "  {} | force: {} | status: {} | distance: {} | since: {}",
                e.decision_id, e.fallen_force_id, e.fallen_status, e.distance, e.since
            );
        }
        println!();
        println!("Frontier ({}):", snap.frontier().len());
        for id in snap.frontier() {
            println!("  {}", id);
        }
    }
}
