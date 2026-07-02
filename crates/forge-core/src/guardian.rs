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
    pub cfg: Config,
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

    pub fn commit(&mut self, proposed: Proposed) -> Result<Receipt, String> {
        let re_proposed = self
            .propose_decision(proposed.input.clone())
            .map_err(|e| format!("commit validation failed: {}", e.0))?;
        if !re_proposed.problems.is_empty() {
            return Err(format!("invalid: {}", re_proposed.problems.join("; ")));
        }

        let mut created_force_ids = Vec::new();
        let mut reused = Vec::new();
        let mut warnings = Vec::new();

        // reuse_map[i] = Some(existing_id) if the i-th new force was reused
        let mut reuse_map: Vec<Option<String>> = vec![None; re_proposed.new_forces.len()];
        let mut new_force_idx: usize = 0;

        for force_input in &proposed.input.forces {
            if let ForceInput::New {
                title,
                body: _,
                force_new,
            } = force_input
            {
                let near = crate::recall::near_matches(
                    &self.snapshot,
                    self.embedder.as_ref(),
                    title,
                    self.cfg.dedup.warn,
                )
                .unwrap_or_default();

                let living_matches: Vec<_> = near
                    .iter()
                    .filter(|h| h.status != "retired" && h.superseded_by.is_none())
                    .collect();

                let best_living = living_matches.first();

                if let Some(best) = best_living {
                    if best.score >= self.cfg.dedup.reuse && !*force_new {
                        reuse_map[new_force_idx] = Some(best.id.clone());
                        reused.push(ReusedEntry {
                            proposed_id: title.clone(),
                            existing_id: best.id.clone(),
                            score: best.score,
                        });
                        new_force_idx += 1;
                        continue;
                    } else if best.score >= self.cfg.dedup.warn {
                        warnings.push(format!(
                            "near-duplicate force '{}' (cosine {:.3} vs '{}')",
                            title, best.score, best.id
                        ));
                    }
                }

                for h in &near {
                    if h.status == "retired" || h.superseded_by.is_some() {
                        warnings.push(format!(
                            "near-match to retired/superseded force '{}' ({})",
                            h.id, h.status
                        ));
                    }
                }
                new_force_idx += 1;
            }
        }

        let write_decisions_dir = self.cfg.write_dir.join("decisions");
        let write_forces_dir = self.cfg.write_dir.join("forces");
        std::fs::create_dir_all(&write_decisions_dir)
            .map_err(|e| format!("create decisions dir: {}", e))?;
        std::fs::create_dir_all(&write_forces_dir)
            .map_err(|e| format!("create forces dir: {}", e))?;

        for (i, force) in re_proposed.new_forces.iter().enumerate() {
            if reuse_map[i].is_none() {
                let content = crate::record::serialize_force(force);
                let path = write_forces_dir.join(format!("{}.md", force.id));
                std::fs::write(&path, &content).map_err(|e| format!("write force: {}", e))?;
                created_force_ids.push(force.id.clone());
            }
        }

        let mut decision_cites: Vec<String> = Vec::new();
        let mut new_force_idx = 0;
        for force_input in &proposed.input.forces {
            match force_input {
                ForceInput::Existing { id } => {
                    decision_cites.push(id.clone());
                }
                ForceInput::New { .. } => {
                    if let Some(existing_id) = &reuse_map[new_force_idx] {
                        decision_cites.push(existing_id.clone());
                    } else {
                        decision_cites.push(re_proposed.new_forces[new_force_idx].id.clone());
                    }
                    new_force_idx += 1;
                }
            }
        }

        let mut decision = re_proposed.decision.clone();
        decision.cites = decision_cites;
        let content = crate::record::serialize_decision(&decision);
        let path = write_decisions_dir.join(format!("{}.md", decision.id));
        std::fs::write(&path, &content).map_err(|e| format!("write decision: {}", e))?;

        self.rebuild()
            .map_err(|e| format!("rebuild after commit: {}", e))?;

        Ok(Receipt {
            decision_id: decision.id,
            created_force_ids,
            reused,
            warnings,
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

    pub fn set_status(&mut self, id: &str, new_status: &str) -> Result<StatusReceipt, String> {
        let old_stale_ids: std::collections::HashSet<String> = self
            .snapshot
            .stale_report()
            .iter()
            .map(|e| e.decision_id.clone())
            .collect();

        let record = self
            .snapshot
            .graph
            .get(id)
            .ok_or_else(|| format!("unknown id: {}", id))?;

        match record {
            crate::graph::Record::Force(_f) => {
                let new_fstatus = match new_status {
                    "changed" => ForceStatus::Changed,
                    "retired" => ForceStatus::Retired,
                    "holds" => return Err(
                        "force status cannot be set to 'holds' (only forward transitions allowed)"
                            .to_string(),
                    ),
                    _ => return Err(format!("unknown force status: {}", new_status)),
                };

                let path = find_record_file(id, &self.cfg)
                    .ok_or_else(|| format!("file not found for: {}", id))?;
                let content =
                    std::fs::read_to_string(&path).map_err(|e| format!("read error: {}", e))?;
                let parsed = crate::record::parse(&path, &content);
                match parsed {
                    crate::record::Parsed::Force(mut force) => {
                        let current = force.current_status();
                        let legal = matches!(
                            (current, new_fstatus),
                            (ForceStatus::Holds, ForceStatus::Changed)
                                | (ForceStatus::Holds, ForceStatus::Retired)
                                | (ForceStatus::Changed, ForceStatus::Retired)
                        );
                        if !legal {
                            return Err(format!(
                                "illegal transition: {:?} -> {}",
                                current, new_status
                            ));
                        }

                        force.status_log.push(StatusEntry {
                            status: new_fstatus,
                            since: today_iso(),
                        });

                        let serialized = crate::record::serialize_force(&force);
                        std::fs::write(&path, &serialized)
                            .map_err(|e| format!("write error: {}", e))?;
                    }
                    _ => return Err(format!("record {} is not a force", id)),
                }
            }
            crate::graph::Record::Decision(_d) => match new_status {
                "deprecated" => {
                    let path = find_record_file(id, &self.cfg)
                        .ok_or_else(|| format!("file not found for: {}", id))?;
                    let content =
                        std::fs::read_to_string(&path).map_err(|e| format!("read error: {}", e))?;
                    let parsed = crate::record::parse(&path, &content);
                    match parsed {
                        crate::record::Parsed::Decision(mut decision) => {
                            if decision.status != DecisionStatus::Accepted {
                                return Err(format!(
                                    "only accepted decisions can be deprecated, current: {:?}",
                                    decision.status
                                ));
                            }
                            decision.status = DecisionStatus::Deprecated;
                            decision.date = today_iso();
                            let serialized = crate::record::serialize_decision(&decision);
                            std::fs::write(&path, &serialized)
                                .map_err(|e| format!("write error: {}", e))?;
                        }
                        _ => return Err(format!("record {} is not a decision", id)),
                    }
                }
                _ => {
                    return Err(format!(
                        "decision status can only be set to 'deprecated', got: {}",
                        new_status
                    ))
                }
            },
        }

        self.rebuild()
            .map_err(|e| format!("rebuild error: {}", e))?;

        let new_stale_ids: std::collections::HashSet<String> = self
            .snapshot
            .stale_report()
            .iter()
            .map(|e| e.decision_id.clone())
            .collect();
        let newly_stale: Vec<String> = new_stale_ids.difference(&old_stale_ids).cloned().collect();

        Ok(StatusReceipt {
            id: id.to_string(),
            new_status: new_status.to_string(),
            newly_stale,
        })
    }
}

fn find_record_file(id: &str, cfg: &Config) -> Option<std::path::PathBuf> {
    for root in &cfg.roots {
        let candidate = root.join(format!("{}.md", id));
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

#[derive(Debug, Clone)]
pub struct Proposed {
    pub decision: Decision,
    pub new_forces: Vec<Force>,
    pub problems: Vec<String>,
    pub near_matches: Vec<(String, Vec<Hit>)>,
    pub input: ProposeInput,
}

pub struct Receipt {
    pub decision_id: String,
    pub created_force_ids: Vec<String>,
    pub reused: Vec<ReusedEntry>,
    pub warnings: Vec<String>,
}

pub struct ReusedEntry {
    pub proposed_id: String,
    pub existing_id: String,
    pub score: f32,
}

pub struct StatusReceipt {
    pub id: String,
    pub new_status: String,
    pub newly_stale: Vec<String>,
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

    fn hash_all_files(dir: &PathBuf) -> std::collections::HashMap<String, String> {
        use sha2::{Digest, Sha256};
        use std::fs;
        let mut hashes = std::collections::HashMap::new();
        for entry in walkdir::WalkDir::new(dir) {
            let entry = entry.unwrap();
            if entry.file_type().is_file() {
                let content = fs::read(entry.path()).unwrap();
                let mut hasher = Sha256::new();
                hasher.update(&content);
                let hash = format!("{:x}", hasher.finalize());
                hashes.insert(entry.path().to_string_lossy().to_string(), hash);
            }
        }
        hashes
    }

    #[test]
    fn commit_writes_files_and_agent_reads_its_own_write() {
        let (_dir, config_path) = fixture_copy_to_temp();
        let cfg = Config::load(&config_path).unwrap();
        let embedder = Box::new(BucketEmbedder::default());
        let mut engine = Engine::new(cfg, embedder).unwrap();

        let p = engine
            .propose_decision(ProposeInput {
                title: "Commit write test".into(),
                body: "Testing commit writes.".into(),
                forces: vec![ForceInput::New {
                    title: "A test force".into(),
                    body: "...".into(),
                    force_new: false,
                }],
                supersedes: vec![],
                relates: vec![],
                tags: vec![],
            })
            .unwrap();

        let receipt = engine.commit(p).unwrap();
        assert!(!receipt.decision_id.is_empty());
        let decision_path = engine
            .cfg
            .write_dir
            .join("decisions")
            .join(format!("{}.md", receipt.decision_id));
        assert!(decision_path.exists());
        let snap = engine.snapshot();
        assert!(snap.graph.get(&receipt.decision_id).is_some());
    }

    #[test]
    fn dedup_high_band_reuses_existing_id() {
        let mut embedder = crate::embed::fake::PinnedEmbedder::new();
        let norm = (0.5_f32 * 0.5 + 0.5 * 0.5 + 0.5 * 0.5).sqrt();
        let v = vec![0.5 / norm, 0.5 / norm, 0.5 / norm];
        embedder.pin("A test force", v.clone());
        embedder.pin("A tast farce", vec![0.48 / norm, 0.52 / norm, 0.5 / norm]);

        let (_dir, config_path) = fixture_copy_to_temp();
        let cfg = Config::load(&config_path).unwrap();
        let mut engine = Engine::new(cfg, Box::new(embedder)).unwrap();

        let p1 = engine
            .propose_decision(ProposeInput {
                title: "First proposal".into(),
                body: "body".into(),
                forces: vec![ForceInput::New {
                    title: "A test force".into(),
                    body: "...".into(),
                    force_new: false,
                }],
                supersedes: vec![],
                relates: vec![],
                tags: vec![],
            })
            .unwrap();
        let r1 = engine.commit(p1).unwrap();
        assert!(!r1.decision_id.is_empty());

        let p2 = engine
            .propose_decision(ProposeInput {
                title: "Second proposal".into(),
                body: "body".into(),
                forces: vec![ForceInput::New {
                    title: "A tast farce".into(),
                    body: "...".into(),
                    force_new: false,
                }],
                supersedes: vec![],
                relates: vec![],
                tags: vec![],
            })
            .unwrap();
        let r2 = engine.commit(p2).unwrap();
        assert!(!r2.reused.is_empty());
    }

    #[test]
    fn commit_is_append_only() {
        let (_dir, config_path) = fixture_copy_to_temp();
        let cfg = Config::load(&config_path).unwrap();
        let embedder = Box::new(BucketEmbedder::default());
        let mut engine = Engine::new(cfg, embedder).unwrap();

        let pre_hashes = hash_all_files(&_dir);

        let p = engine
            .propose_decision(ProposeInput {
                title: "Append only test".into(),
                body: "body".into(),
                forces: vec![ForceInput::New {
                    title: "New force for append test".into(),
                    body: "...".into(),
                    force_new: false,
                }],
                supersedes: vec![],
                relates: vec![],
                tags: vec![],
            })
            .unwrap();
        engine.commit(p).unwrap();

        let post_hashes = hash_all_files(&_dir);
        for (path, pre_hash) in &pre_hashes {
            assert_eq!(
                post_hashes.get(path),
                Some(pre_hash),
                "file {} was modified!",
                path
            );
        }
    }

    #[test]
    fn force_set_status_appends_to_log() {
        let (_dir, config_path) = fixture_copy_to_temp();
        let cfg = Config::load(&config_path).unwrap();
        let embedder = Box::new(BucketEmbedder::default());
        let mut engine = Engine::new(cfg, embedder).unwrap();

        let receipt = engine.set_status("f-rust-stable", "changed").unwrap();
        assert_eq!(receipt.id, "f-rust-stable");
        assert_eq!(receipt.new_status, "changed");
        let snap = engine.snapshot();
        if let Some(crate::graph::Record::Force(f)) = snap.graph.get("f-rust-stable") {
            assert_eq!(f.current_status(), crate::record::ForceStatus::Changed);
        } else {
            panic!("f-rust-stable not found");
        }
    }

    #[test]
    fn illegal_transitions_rejected() {
        let (_dir, config_path) = fixture_copy_to_temp();
        let cfg = Config::load(&config_path).unwrap();
        let embedder = Box::new(BucketEmbedder::default());
        let mut engine = Engine::new(cfg, embedder).unwrap();

        assert!(engine.set_status("f-retired-old", "holds").is_err());
        assert!(engine.set_status("f-onnx-portable", "retired").is_ok());
        assert!(engine.set_status("f-onnx-portable", "holds").is_err());
        assert!(engine.set_status("f-nonexistent", "changed").is_err());
    }

    #[test]
    fn set_status_returns_propagation_impact() {
        let (_dir, config_path) = fixture_copy_to_temp();
        let cfg = Config::load(&config_path).unwrap();
        let embedder = Box::new(BucketEmbedder::default());
        let mut engine = Engine::new(cfg, embedder).unwrap();

        let receipt = engine.set_status("f-rust-stable", "changed").unwrap();
        assert!(receipt.newly_stale.iter().any(|id| id == "d-use-rust"));
    }
}
