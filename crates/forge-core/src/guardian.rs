use std::sync::Arc;

use crate::config::Config;
use crate::embed::{EmbedError, Embedder};
use crate::recall::Hit;
use crate::record::{Decision, DecisionStatus, Force, ForceStatus, StatusEntry};
use crate::snapshot::Snapshot;

pub fn today_iso() -> String {
    chrono::Utc::now().format("%Y-%m-%d").to_string()
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ProposeInput {
    pub title: String,
    pub body: String,
    pub forces: Vec<ForceInput>,
    pub supersedes: Vec<String>,
    pub relates: Vec<String>,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum ForceInput {
    New {
        title: String,
        body: String,
        force_new: bool,
    },
    Existing {
        id: String,
    },
}

pub struct Engine {
    cfg: Config,
    embedder: Box<dyn Embedder>,
    snapshot: Arc<Snapshot>,
    used_ids: std::collections::HashSet<String>,
}

impl Engine {
    pub fn new(cfg: Config, embedder: Box<dyn Embedder>) -> Result<Engine, String> {
        let snap = Snapshot::build(&cfg, embedder.as_ref())
            .map_err(|e| format!("build error: {}", e.0))?;
        let used_ids: std::collections::HashSet<String> = snap
            .graph
            .decisions()
            .iter()
            .map(|d| d.id.clone())
            .chain(snap.graph.forces().iter().map(|f| f.id.clone()))
            .collect();
        Ok(Engine {
            cfg,
            embedder,
            snapshot: Arc::new(snap),
            used_ids,
        })
    }

    pub fn snapshot(&self) -> Arc<Snapshot> {
        self.snapshot.clone()
    }

    pub fn rebuild(&mut self) -> Result<(), String> {
        let snap = Snapshot::build(&self.cfg, self.embedder.as_ref())
            .map_err(|e| format!("build error: {}", e.0))?;
        self.used_ids = snap
            .graph
            .decisions()
            .iter()
            .map(|d| d.id.clone())
            .chain(snap.graph.forces().iter().map(|f| f.id.clone()))
            .collect();
        self.snapshot = Arc::new(snap);
        Ok(())
    }

    pub fn propose_decision(&self, input: ProposeInput) -> Result<Proposed, EmbedError> {
        let mut problems = Vec::new();
        let mut new_forces = Vec::new();
        let mut near_matches_vec: Vec<(String, Vec<Hit>)> = Vec::new();
        let mut all_cites: Vec<String> = Vec::new();

        for force_input in &input.forces {
            match force_input {
                ForceInput::Existing { id } => {
                    if self.snapshot.graph.get(id).is_none() {
                        problems.push(format!("unknown force id {}", id));
                    } else {
                        all_cites.push(id.clone());
                    }
                }
                ForceInput::New {
                    title,
                    body,
                    force_new: _force_new,
                } => {
                    let force_id = self.generate_id("f", title);
                    let near = crate::recall::near_matches(
                        &self.snapshot,
                        self.embedder.as_ref(),
                        title,
                        self.cfg.dedup.warn,
                    )
                    .unwrap_or_default();
                    near_matches_vec.push((force_id.clone(), near));

                    let new_force = Force {
                        id: force_id.clone(),
                        title: title.clone(),
                        depends_on: vec![],
                        status_log: vec![StatusEntry {
                            status: ForceStatus::Holds,
                            since: today_iso(),
                        }],
                        superseded_by: None,
                        tags: vec![],
                        body: body.clone(),
                        path: std::path::PathBuf::new(),
                    };
                    new_forces.push(new_force);
                    all_cites.push(force_id);
                }
            }
        }

        let decision_id = self.generate_id("d", &input.title);
        let decision = Decision {
            id: decision_id,
            title: input.title.clone(),
            status: DecisionStatus::Accepted,
            date: today_iso(),
            cites: all_cites,
            supersedes: input.supersedes.clone(),
            relates: input.relates.clone(),
            anchors: vec![],
            tags: input.tags.clone(),
            body: input.body.clone(),
            path: std::path::PathBuf::new(),
        };

        Ok(Proposed {
            decision,
            new_forces,
            problems,
            near_matches: near_matches_vec,
            input,
        })
    }

    pub fn generate_id(&self, prefix: &str, title: &str) -> String {
        let slug = title
            .to_lowercase()
            .chars()
            .map(|c| if c.is_alphanumeric() { c } else { '-' })
            .collect::<String>()
            .split('-')
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("-");
        let slug = &slug[..slug.len().min(48)];
        let base = format!("{}-{}", prefix, slug.trim_matches('-'));

        let mut id = base.clone();
        let mut counter = 2;
        while self.used_ids.contains(&id) {
            id = format!("{}-{}", base, counter);
            counter += 1;
        }
        id
    }
}

#[derive(Debug, Clone)]
pub struct Proposed {
    pub decision: Decision,
    pub new_forces: Vec<Force>,
    pub problems: Vec<String>,
    pub near_matches: Vec<(String, Vec<Hit>)>,
    pub input: ProposeInput,
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    use crate::config::Config;
    use crate::embed::fake::BucketEmbedder;

    use super::*;

    static TEST_COUNTER: AtomicU32 = AtomicU32::new(0);

    fn fixture_copy_to_temp() -> (PathBuf, PathBuf) {
        let n = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
        let src = PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/corpus"
        ));
        let dst = std::env::temp_dir().join(format!("forge-guardian-test-{}", n));
        let _ = std::fs::remove_dir_all(&dst);
        copy_dir(&src, &dst).unwrap();
        (dst.clone(), dst.join("forge.toml"))
    }

    fn copy_dir(src: &PathBuf, dst: &PathBuf) -> std::io::Result<()> {
        std::fs::create_dir_all(dst)?;
        for entry in std::fs::read_dir(src)? {
            let entry = entry?;
            let ty = entry.file_type()?;
            let src_path = entry.path();
            let dst_path = dst.join(entry.file_name());
            if ty.is_dir() {
                copy_dir(&src_path, &dst_path)?;
            } else {
                std::fs::copy(&src_path, &dst_path)?;
            }
        }
        Ok(())
    }

    #[test]
    fn propose_composes_records_without_touching_disk() {
        let (_dir, config_path) = fixture_copy_to_temp();
        let cfg = Config::load(&config_path).unwrap();
        let embedder = Box::new(BucketEmbedder::default());
        let engine = Engine::new(cfg, embedder).unwrap();

        let p = engine
            .propose_decision(ProposeInput {
                title: "Adopt sqlite for the cache".into(),
                body: "Sqlite is a proven embedded database.".into(),
                forces: vec![
                    ForceInput::New {
                        title: "SQLite is zero-config".into(),
                        body: "No server setup needed.".into(),
                        force_new: false,
                    },
                    ForceInput::Existing {
                        id: "f-rust-stable".into(),
                    },
                ],
                supersedes: vec![],
                relates: vec![],
                tags: vec![],
            })
            .unwrap();

        assert!(p.decision.id.starts_with("d-adopt-sqlite-for-the-cache"));
        assert_eq!(p.decision.status, crate::record::DecisionStatus::Accepted);
        assert_eq!(p.decision.date, today_iso());
        assert_eq!(p.new_forces.len(), 1);
        assert!(p.new_forces[0].id.starts_with("f-sqlite-is-zero-config"));
        assert!(p.decision.cites.contains(&"f-rust-stable".to_string()));
    }

    #[test]
    fn propose_reports_validation_problems_as_data() {
        let (_dir, config_path) = fixture_copy_to_temp();
        let cfg = Config::load(&config_path).unwrap();
        let embedder = Box::new(BucketEmbedder::default());
        let engine = Engine::new(cfg, embedder).unwrap();

        let p = engine
            .propose_decision(ProposeInput {
                title: "Test".into(),
                body: "body".into(),
                forces: vec![ForceInput::Existing {
                    id: "f-nope".into(),
                }],
                supersedes: vec![],
                relates: vec![],
                tags: vec![],
            })
            .unwrap();

        assert!(!p.problems.is_empty());
        assert!(p.problems.iter().any(|msg| msg.contains("f-nope")));
    }

    #[test]
    fn propose_attaches_near_matches_per_new_force() {
        let mut embedder = crate::embed::fake::PinnedEmbedder::new();
        embedder.pin("SQLite is zero-config", vec![0.577, 0.577, 0.577]);
        embedder.pin(
            "Rust is a stable, mature systems language",
            vec![0.55, 0.55, 0.55],
        );

        let (_dir, config_path) = fixture_copy_to_temp();
        let cfg = Config::load(&config_path).unwrap();
        let engine = Engine::new(cfg, Box::new(embedder)).unwrap();

        let p = engine
            .propose_decision(ProposeInput {
                title: "Test".into(),
                body: "body".into(),
                forces: vec![ForceInput::New {
                    title: "SQLite is zero-config".into(),
                    body: "...".into(),
                    force_new: false,
                }],
                supersedes: vec![],
                relates: vec![],
                tags: vec![],
            })
            .unwrap();

        assert!(!p.near_matches.is_empty());
    }

    #[test]
    fn id_generation_slugifies_and_disambiguates() {
        let (_dir, config_path) = fixture_copy_to_temp();
        let cfg = Config::load(&config_path).unwrap();
        let embedder = Box::new(BucketEmbedder::default());
        let engine = Engine::new(cfg, embedder).unwrap();

        let id = engine.generate_id("d", "Hello, World! 2026");
        assert!(id.starts_with("d-hello-world-2026"));
        assert!(!id.contains(' '));
        assert!(!id.contains(','));

        let long_title = "a".repeat(60);
        let id = engine.generate_id("d", &long_title);
        assert!(id.len() <= 50);

        let id2 = engine.generate_id("d", "Use, Rust!");
        assert!(id2.starts_with("d-use-rust"));
        assert_ne!(id, id2);
        assert!(id2.ends_with("-2"));
    }

    #[test]
    fn propose_is_repeatable() {
        let (_dir, config_path) = fixture_copy_to_temp();
        let cfg = Config::load(&config_path).unwrap();
        let embedder = Box::new(BucketEmbedder::default());
        let engine = Engine::new(cfg, embedder).unwrap();

        let input = ProposeInput {
            title: "Test repeatable propose".into(),
            body: "body".into(),
            forces: vec![ForceInput::New {
                title: "A new force".into(),
                body: "...".into(),
                force_new: false,
            }],
            supersedes: vec![],
            relates: vec![],
            tags: vec![],
        };

        let p1 = engine.propose_decision(input.clone()).unwrap();
        let p2 = engine.propose_decision(input).unwrap();

        assert_eq!(p1.decision.id, p2.decision.id);
        assert_eq!(p1.decision.title, p2.decision.title);
    }
}
