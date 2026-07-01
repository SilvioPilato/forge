mod fixtures;

use forge_core::embed::NullEmbedder;
use forge_core::snapshot::Snapshot;

#[test]
fn phase1_checkpoint_matches_oracle() {
    let cfg = fixtures::corpus_config();
    let snap = Snapshot::build(&cfg, &NullEmbedder).unwrap();

    assert_eq!(snap.diagnostics.len(), 4, "expected exactly 4 diagnostics");

    use forge_core::linker::Diagnostic;
    let has_parse_error = snap.diagnostics.iter().any(|d| {
        matches!(d, Diagnostic::ParseError { path, .. } if path.file_name().unwrap() == "malformed.md")
    });
    assert!(has_parse_error, "missing parse error for malformed.md");

    let has_collision = snap.diagnostics.iter().any(|d| {
        matches!(d, Diagnostic::IdCollision { id, paths } if id == "f-duplicate" && paths.len() == 2)
    });
    assert!(has_collision, "missing id collision for f-duplicate");

    let has_dangling = snap.diagnostics.iter().any(|d| {
        matches!(d, Diagnostic::DanglingRef { from, field: _, to } if from == "d-dangling" && to == "f-missing")
    });
    assert!(has_dangling, "missing dangling ref d-dangling -> f-missing");

    let has_cycle = snap.diagnostics.iter().any(|d| {
        matches!(d, Diagnostic::DependsOnCycle { members } if members.contains(&"f-cycle-a".to_string()) && members.contains(&"f-cycle-b".to_string()))
    });
    assert!(has_cycle, "missing depends-on cycle");

    use forge_core::judge::PremiseVerdict;

    let v_embed = snap.verdicts.premise.get("d-embed-onnx").unwrap();
    match v_embed {
        PremiseVerdict::Stale { fallen } => {
            assert_eq!(fallen.len(), 1);
            assert_eq!(fallen[0].force_id, "f-onnx-portable");
            assert_eq!(fallen[0].distance, 1);
        }
        _ => panic!("d-embed-onnx should be stale"),
    }

    let v_small = snap.verdicts.premise.get("d-small-model").unwrap();
    match v_small {
        PremiseVerdict::Stale { fallen } => {
            assert_eq!(fallen.len(), 1);
            assert_eq!(fallen[0].force_id, "f-onnx-portable");
            assert_eq!(fallen[0].distance, 2);
        }
        _ => panic!("d-small-model should be stale"),
    }

    let v_legacy = snap.verdicts.premise.get("d-keep-legacy").unwrap();
    match v_legacy {
        PremiseVerdict::Stale { fallen } => {
            assert_eq!(fallen.len(), 1);
            assert_eq!(fallen[0].force_id, "f-retired-old");
            assert_eq!(fallen[0].distance, 1);
        }
        _ => panic!("d-keep-legacy should be stale"),
    }

    let v_old = snap.verdicts.premise.get("d-old-storage").unwrap();
    assert!(
        matches!(v_old, PremiseVerdict::Stale { .. }),
        "d-old-storage should be premise-stale"
    );
    assert!(snap.verdicts.superseded.contains("d-old-storage"));

    for fresh_id in &["d-use-rust", "d-new-storage", "d-dangling", "d-deprecated"] {
        let v = snap.verdicts.premise.get(*fresh_id).unwrap();
        assert!(
            matches!(v, PremiseVerdict::Fresh),
            "{} should be fresh",
            fresh_id
        );
    }

    assert!(snap.verdicts.superseded.contains("d-old-storage"));
    assert!(snap.verdicts.superseded.contains("d-deprecated"));
    assert!(!snap.verdicts.superseded.contains("d-use-rust"));

    assert_eq!(snap.frontier().len(), 6);
    let expected: Vec<&str> = vec![
        "d-dangling",
        "d-embed-onnx",
        "d-keep-legacy",
        "d-new-storage",
        "d-small-model",
        "d-use-rust",
    ];
    assert_eq!(snap.frontier(), expected.as_slice());

    let report = snap.stale_report();
    let stale_ids: Vec<&str> = report.iter().map(|e| e.decision_id.as_str()).collect();
    assert_eq!(
        stale_ids,
        vec!["d-keep-legacy", "d-embed-onnx", "d-small-model"]
    );

    let why_chain = snap.why("d-small-model").unwrap();
    let why_ids: Vec<&str> = why_chain.iter().map(|e| e.force_id.as_str()).collect();
    assert!(why_ids.contains(&"f-model-small"));
    assert!(why_ids.contains(&"f-onnx-portable"));
}
