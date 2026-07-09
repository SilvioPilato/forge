use std::collections::HashMap;

use tracing::{info, instrument};

use crate::config::Config;
use crate::discovery;
use crate::embed::cache::VectorCache;
use crate::embed::{Embedder, Vector};
use crate::graph::Graph;
use crate::judge::{judge, Verdicts};
use crate::linker::{link, Diagnostic};
use crate::record::parse;

pub struct Snapshot {
    pub graph: Graph,
    pub verdicts: Verdicts,
    pub diagnostics: Vec<Diagnostic>,
    pub vectors: HashMap<String, Vector>,
    pub model_id: String,
    pub built_at: chrono::DateTime<chrono::Utc>,
}

pub struct StaleEntry {
    pub decision_id: String,
    pub decision_title: String,
    pub fallen_force_id: String,
    pub fallen_status: String,
    pub distance: u32,
    pub since: String,
}

pub struct WhyEntry {
    pub force_id: String,
    pub force_title: String,
    pub current_status: String,
    pub path: String,
}

impl Snapshot {
    #[instrument(skip(cfg, embedder), fields(roots = ?cfg.roots))]
    pub fn build(cfg: &Config, embedder: &dyn Embedder) -> Result<Snapshot, BuildError> {
        let (paths, mut diagnostics) = discovery::discover(&cfg.roots);
        let parsed: Vec<_> = paths
            .iter()
            .map(|p| {
                let text = std::fs::read_to_string(p).unwrap_or_default();
                parse(p, &text)
            })
            .collect();
        let linked = link(parsed);
        let graph = Graph::build(&linked);
        let verdicts = judge(&graph);
        diagnostics.extend(linked.diagnostics);

        let mut texts: Vec<String> = Vec::new();
        let mut text_ids: Vec<String> = Vec::new();
        for d in &linked.decisions {
            texts.push(embed_text(&d.title, &d.body, &d.tags));
            text_ids.push(d.id.clone());
        }
        for f in &linked.forces {
            texts.push(embed_text(&f.title, &f.body, &f.tags));
            text_ids.push(f.id.clone());
        }

        let mut cache = VectorCache::new(cfg.cache_dir.clone(), embedder.model_id());

        let mut vectors = HashMap::new();
        let mut missed: Vec<(String, String)> = Vec::new();

        for (id, text) in text_ids.iter().zip(texts.iter()) {
            let hash = cache.content_hash(text);
            if let Some(vec) = cache.get(&hash) {
                vectors.insert(id.clone(), vec.clone());
            } else {
                missed.push((id.clone(), text.clone()));
            }
        }

        if !missed.is_empty() {
            let missed_texts: Vec<String> = missed.iter().map(|(_, t)| t.clone()).collect();
            let fresh = embedder
                .embed_passages(&missed_texts)
                .map_err(|e| BuildError(format!("embedding failed: {}", e.0)))?;
            for ((id, text), vec) in missed.into_iter().zip(fresh.into_iter()) {
                let hash = cache.content_hash(&text);
                cache.put(&hash, &vec);
                vectors.insert(id, vec);
            }
        }

        let _ = cache.save();

        info!(
            decisions = graph.decisions().len(),
            forces = graph.forces().len(),
            diagnostics = diagnostics.len(),
            frontier = verdicts.frontier.len(),
            "snapshot built"
        );

        Ok(Snapshot {
            graph,
            verdicts,
            diagnostics,
            vectors,
            model_id: embedder.model_id().to_string(),
            built_at: chrono::Utc::now(),
        })
    }

    pub fn stale_report(&self) -> Vec<StaleEntry> {
        let mut entries = Vec::new();
        for id in &self.verdicts.frontier {
            if let Some(crate::judge::PremiseVerdict::Stale { fallen }) =
                self.verdicts.premise.get(id)
            {
                for fp in fallen {
                    let title = self.get_title(id);
                    entries.push(StaleEntry {
                        decision_id: id.clone(),
                        decision_title: title,
                        fallen_force_id: fp.force_id.clone(),
                        fallen_status: format!("{:?}", fp.status).to_lowercase(),
                        distance: fp.distance,
                        since: fp.since.clone(),
                    });
                }
            }
        }
        entries.sort_by(|a, b| {
            let severity_a = if a.fallen_status == "retired" { 0 } else { 1 };
            let severity_b = if b.fallen_status == "retired" { 0 } else { 1 };
            severity_a
                .cmp(&severity_b)
                .then(a.distance.cmp(&b.distance))
                .then(b.since.cmp(&a.since))
                .then(a.decision_id.cmp(&b.decision_id))
        });
        entries
    }

    pub fn why(&self, id: &str) -> Option<Vec<WhyEntry>> {
        let verdict = self.verdicts.premise.get(id)?;
        match verdict {
            crate::judge::PremiseVerdict::Fresh => Some(vec![]),
            crate::judge::PremiseVerdict::Stale { fallen } => {
                let mut entries = Vec::new();
                for fp in fallen {
                    if let Some(crate::graph::Record::Decision(d)) = self.graph.get(id) {
                        for cite in &d.cites {
                            if let Some(crate::graph::Record::Force(f)) = self.graph.get(cite) {
                                entries.push(WhyEntry {
                                    force_id: f.id.clone(),
                                    force_title: f.title.clone(),
                                    current_status: format!("{:?}", f.current_status())
                                        .to_lowercase(),
                                    path: "direct".to_string(),
                                });
                                if f.depends_on.iter().any(|dep| dep == &fp.force_id) {
                                    if let Some(crate::graph::Record::Force(ff)) =
                                        self.graph.get(&fp.force_id)
                                    {
                                        entries.push(WhyEntry {
                                            force_id: ff.id.clone(),
                                            force_title: ff.title.clone(),
                                            current_status: format!("{:?}", ff.current_status())
                                                .to_lowercase(),
                                            path: format!("via {}", f.id),
                                        });
                                    }
                                }
                            }
                        }
                    }
                }
                Some(entries)
            }
        }
    }

    pub fn frontier(&self) -> &[String] {
        &self.verdicts.frontier
    }

    fn get_title(&self, id: &str) -> String {
        self.graph
            .get(id)
            .map(|r| match r {
                crate::graph::Record::Decision(d) => d.title.clone(),
                crate::graph::Record::Force(f) => f.title.clone(),
            })
            .unwrap_or_default()
    }
}

/// The text embedded for a record's similarity vector. Both decisions and
/// forces embed `title + body` so short or generic titles stay searchable, and
/// tags are appended so tag terms match a query too (issue #8).
fn embed_text(title: &str, body: &str, tags: &[String]) -> String {
    let mut text = format!("{}\n\n{}", title, body);
    if !tags.is_empty() {
        text.push_str("\n\nTags: ");
        text.push_str(&tags.join(", "));
    }
    text
}

#[derive(Debug)]
pub struct BuildError(pub String);

impl std::fmt::Display for BuildError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}
impl std::error::Error for BuildError {}

#[cfg(test)]
mod tests {
    use crate::config::Config;
    use crate::embed::NullEmbedder;

    use super::*;

    fn load_corpus_snapshot() -> Snapshot {
        let dir = std::path::PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/corpus"
        ));
        let cfg = Config::load(&dir.join("forge.toml")).unwrap();
        let embedder = NullEmbedder;
        Snapshot::build(&cfg, &embedder).unwrap()
    }

    #[test]
    fn builds_snapshot_from_fixture_corpus() {
        let snap = load_corpus_snapshot();
        assert_eq!(snap.diagnostics.len(), 4);
        assert_eq!(snap.frontier().len(), 6);
        let report = snap.stale_report();
        let ids: Vec<&str> = report.iter().map(|e| e.decision_id.as_str()).collect();
        assert_eq!(ids, vec!["d-keep-legacy", "d-embed-onnx", "d-small-model"]);
    }

    #[test]
    fn stale_report_orders_by_status_then_distance_then_recency() {
        let snap = load_corpus_snapshot();
        let report = snap.stale_report();
        assert_eq!(report[0].decision_id, "d-keep-legacy");
        assert_eq!(report[1].decision_id, "d-embed-onnx");
        assert_eq!(report[2].decision_id, "d-small-model");
    }

    #[test]
    fn why_walks_cites_then_depends_on() {
        let snap = load_corpus_snapshot();
        let chain = snap.why("d-small-model").unwrap();
        assert!(!chain.is_empty());
        let ids: Vec<&str> = chain.iter().map(|e| e.force_id.as_str()).collect();
        assert!(ids.contains(&"f-model-small"));
        assert!(ids.contains(&"f-onnx-portable"));
    }

    #[test]
    fn partial_graph_still_answers() {
        let snap = load_corpus_snapshot();
        assert!(snap.graph.get("d-use-rust").is_some());
        assert!(snap.graph.get("d-embed-onnx").is_some());
        let decisions = snap.graph.decisions();
        assert_eq!(decisions.len(), 8);
    }

    // Issue #8: a force's body and tags must contribute to its search vector,
    // not just its title. Two forces share a generic title; the distinctive
    // terms live only in one's body and tags, so a query for those terms must
    // surface that force ahead of the other.
    #[test]
    fn force_body_and_tags_are_searchable() {
        use crate::embed::fake::BucketEmbedder;
        use crate::recall::{search, Scope};

        let dir = std::env::temp_dir().join("forge-snapshot-force-index-test");
        let _ = std::fs::remove_dir_all(&dir);
        let forces = dir.join("forces");
        std::fs::create_dir_all(&forces).unwrap();
        std::fs::write(
            dir.join("forge.toml"),
            "roots = [\"forces\"]\n[embedding]\nmodel = \"fake-bucket\"\n",
        )
        .unwrap();
        std::fs::write(
            forces.join("f-alpha.md"),
            "---\nid: f-alpha\ntype: force\ntitle: Generic capability\nstatus_log:\n  - { status: holds, since: 2026-01-01 }\ntags: [distincttagword]\n---\nAlpha body mentions distinctbodyword prominently.\n",
        )
        .unwrap();
        std::fs::write(
            forces.join("f-beta.md"),
            "---\nid: f-beta\ntype: force\ntitle: Generic capability\nstatus_log:\n  - { status: holds, since: 2026-01-01 }\n---\nBeta body is entirely unrelated.\n",
        )
        .unwrap();

        let cfg = Config::load(&dir.join("forge.toml")).unwrap();
        let embedder = BucketEmbedder::default();
        let snap = Snapshot::build(&cfg, &embedder).unwrap();

        let body_hits = search(&snap, &embedder, "distinctbodyword", Scope::Force, 5).unwrap();
        assert_eq!(
            body_hits.first().map(|h| h.id.as_str()),
            Some("f-alpha"),
            "body term should surface f-alpha: {body_hits:?}"
        );

        let tag_hits = search(&snap, &embedder, "distincttagword", Scope::Force, 5).unwrap();
        assert_eq!(
            tag_hits.first().map(|h| h.id.as_str()),
            Some("f-alpha"),
            "tag term should surface f-alpha: {tag_hits:?}"
        );
    }
}
