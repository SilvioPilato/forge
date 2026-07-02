mod fixtures;
use forge_core::config::Config;
use forge_core::embed::fake::BucketEmbedder;
use forge_core::guardian::{Engine, ForceInput, ProposeInput};
use std::path::PathBuf;

fn temp_corpus() -> (PathBuf, PathBuf) {
    let src = fixtures::corpus_dir();
    let dst = std::env::temp_dir().join("forge-phase3-checkpoint");
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

fn hash_files(dir: &PathBuf) -> std::collections::HashMap<String, String> {
    use sha2::{Digest, Sha256};
    let mut hashes = std::collections::HashMap::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.is_file() && path.extension().is_some_and(|e| e == "md") {
            let content = std::fs::read(&path).unwrap();
            let mut h = Sha256::new();
            h.update(&content);
            hashes.insert(
                path.file_name().unwrap().to_string_lossy().to_string(),
                format!("{:x}", h.finalize()),
            );
        }
    }
    hashes
}

#[test]
fn phase3_checkpoint_scripted_e2e() {
    let (_dir, config_path) = temp_corpus();
    let cfg = Config::load(&config_path).unwrap();

    let pre_hashes_decisions = hash_files(&_dir.join("decisions"));
    let pre_hashes_forces = hash_files(&_dir.join("forces"));

    let embedder = Box::new(BucketEmbedder::default());
    let mut engine = Engine::new(cfg, embedder).unwrap();

    let p1 = engine
        .propose_decision(ProposeInput {
            title: "E2E test decision one".into(),
            body: "First checkpoint decision.".into(),
            forces: vec![
                ForceInput::New {
                    title: "A fresh checkpoint force".into(),
                    body: "...".into(),
                    force_new: false,
                },
                ForceInput::Existing {
                    id: "f-new-way".into(),
                },
            ],
            supersedes: vec![],
            relates: vec![],
            tags: vec![],
        })
        .unwrap();
    assert!(p1.problems.is_empty());
    let r1 = engine.commit(p1).unwrap();
    assert!(!r1.created_force_ids.is_empty());
    assert_eq!(r1.reused.len(), 0);

    let p2 = engine
        .propose_decision(ProposeInput {
            title: "E2E test decision two".into(),
            body: "Second checkpoint decision.".into(),
            forces: vec![ForceInput::New {
                title: "A fresh checkpoint force".into(),
                body: "...".into(),
                force_new: false,
            }],
            supersedes: vec![],
            relates: vec![],
            tags: vec![],
        })
        .unwrap();
    let r2 = engine.commit(p2).unwrap();
    assert!(
        !r2.reused.is_empty(),
        "second commit should reuse near-duplicate force"
    );

    let new_force_id = &r1.created_force_ids[0];
    let receipt = engine.set_status(new_force_id, "changed").unwrap();
    assert!(receipt.newly_stale.iter().any(|id| id == &r1.decision_id));
    assert!(receipt.newly_stale.iter().any(|id| id == &r2.decision_id));

    let snap = engine.snapshot();
    let report = snap.stale_report();
    assert!(report.iter().any(|e| e.decision_id == r1.decision_id));
    assert!(report.iter().any(|e| e.decision_id == r2.decision_id));

    let post_hashes_decisions = hash_files(&_dir.join("decisions"));
    let post_hashes_forces = hash_files(&_dir.join("forces"));

    for (name, hash) in &pre_hashes_decisions {
        assert_eq!(
            post_hashes_decisions.get(name),
            Some(hash),
            "decision file {} was modified",
            name
        );
    }
    for (name, hash) in &pre_hashes_forces {
        assert_eq!(
            post_hashes_forces.get(name),
            Some(hash),
            "force file {} was modified",
            name
        );
    }
}
