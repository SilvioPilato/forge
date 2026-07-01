use std::env;
use std::process;

use forge_core::config::Config;
use forge_core::embed::NullEmbedder;
use forge_core::snapshot::Snapshot;
use serde::Serialize;

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
        eprintln!("Usage: forge-inspect <path/to/forge.toml> [--json]");
        process::exit(1);
    }

    let config_path = &args[1];
    let as_json = args.get(2).is_some_and(|a| a == "--json");

    let cfg = Config::load(std::path::Path::new(config_path)).unwrap_or_else(|e| {
        eprintln!("Failed to load config: {}", e);
        process::exit(1);
    });

    let embedder = NullEmbedder;
    let snap = Snapshot::build(&cfg, &embedder).unwrap_or_else(|e| {
        eprintln!("Failed to build snapshot: {}", e);
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
