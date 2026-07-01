use crate::graph::Graph;
use crate::graph::Record;
use crate::record::{DecisionStatus, ForceStatus};
use std::collections::{HashMap, HashSet, VecDeque};

#[derive(Debug)]
pub struct FallenPremise {
    pub force_id: String,
    pub status: ForceStatus,
    pub distance: u32,
    pub since: String,
}

#[derive(Debug)]
pub enum PremiseVerdict {
    Fresh,
    Stale { fallen: Vec<FallenPremise> },
}

pub struct Verdicts {
    pub premise: HashMap<String, PremiseVerdict>,
    pub superseded: HashSet<String>,
    pub frontier: Vec<String>,
}

pub fn judge(graph: &Graph) -> Verdicts {
    let mut premise: HashMap<String, PremiseVerdict> = HashMap::new();
    for d in graph.decisions() {
        premise.insert(d.id.clone(), PremiseVerdict::Fresh);
    }

    let forces = graph.forces();

    for f in &forces {
        let status = f.current_status();
        if matches!(status, ForceStatus::Holds) {
            continue;
        }

        let since = f.status_log.last().unwrap().since.clone();
        let mut visited: HashSet<String> = HashSet::new();
        let mut queue: VecDeque<(String, u32)> = VecDeque::new();

        visited.insert(f.id.clone());
        queue.push_back((f.id.clone(), 0));

        while let Some((current_id, dist)) = queue.pop_front() {
            if let Some(Record::Decision(_)) = graph.get(&current_id) {
                let v = premise.get_mut(&current_id).unwrap();
                match v {
                    PremiseVerdict::Fresh => {
                        *v = PremiseVerdict::Stale {
                            fallen: vec![FallenPremise {
                                force_id: f.id.clone(),
                                status,
                                distance: dist,
                                since: since.clone(),
                            }],
                        };
                    }
                    PremiseVerdict::Stale { fallen } => {
                        fallen.push(FallenPremise {
                            force_id: f.id.clone(),
                            status,
                            distance: dist,
                            since: since.clone(),
                        });
                    }
                }
            }

            for dep_id in graph.depended_by(&current_id) {
                if !visited.contains(&dep_id) {
                    visited.insert(dep_id.clone());
                    queue.push_back((dep_id, dist + 1));
                }
            }

            for cite_id in graph.cited_by(&current_id) {
                if !visited.contains(&cite_id) {
                    visited.insert(cite_id.clone());
                    queue.push_back((cite_id, dist + 1));
                }
            }
        }
    }

    for v in premise.values_mut() {
        if let PremiseVerdict::Stale { fallen } = v {
            fallen.sort_by(|a, b| {
                fn severity(s: ForceStatus) -> u8 {
                    match s {
                        ForceStatus::Retired => 0,
                        ForceStatus::Changed => 1,
                        ForceStatus::Holds => 2,
                    }
                }
                severity(a.status)
                    .cmp(&severity(b.status))
                    .then(a.distance.cmp(&b.distance))
                    .then(a.force_id.cmp(&b.force_id))
            });
        }
    }

    let mut superseded: HashSet<String> = HashSet::new();
    for d in graph.decisions() {
        if matches!(d.status, DecisionStatus::Deprecated) {
            superseded.insert(d.id.clone());
        }
        for s in &d.supersedes {
            superseded.insert(s.clone());
        }
    }

    let mut frontier_ids: HashSet<String> = graph
        .decisions()
        .iter()
        .map(|d| d.id.clone())
        .filter(|id| !superseded.contains(id))
        .collect();
    let mut frontier: Vec<String> = frontier_ids.drain().collect();
    frontier.sort();

    Verdicts {
        premise,
        superseded,
        frontier,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::{parse, ForceStatus};

    fn build_verdicts_from_fixtures() -> (Graph, Verdicts) {
        let dir = std::path::PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/corpus"
        ));
        let cfg = crate::config::Config::load(&dir.join("forge.toml")).unwrap();
        let (paths, _) = crate::discovery::discover(&cfg.roots);
        let parsed: Vec<_> = paths
            .iter()
            .map(|p| {
                let text = std::fs::read_to_string(p).unwrap();
                parse(p, &text)
            })
            .collect();
        let linked = crate::linker::link(parsed);
        let graph = Graph::build(&linked);
        let verdicts = judge(&graph);
        (graph, verdicts)
    }

    #[test]
    fn direct_citation_of_fallen_force_is_stale_distance_1() {
        let (_graph, verdicts) = build_verdicts_from_fixtures();
        let v = verdicts.premise.get("d-embed-onnx").unwrap();
        match v {
            PremiseVerdict::Stale { fallen } => {
                assert_eq!(fallen.len(), 1);
                assert_eq!(fallen[0].force_id, "f-onnx-portable");
                assert_eq!(fallen[0].status, ForceStatus::Changed);
                assert_eq!(fallen[0].distance, 1);
            }
            _ => panic!("expected stale"),
        }
    }

    #[test]
    fn reticolo_propagates_transitively_with_distance() {
        let (_graph, verdicts) = build_verdicts_from_fixtures();
        let v = verdicts.premise.get("d-small-model").unwrap();
        match v {
            PremiseVerdict::Stale { fallen } => {
                assert_eq!(fallen.len(), 1);
                assert_eq!(fallen[0].force_id, "f-onnx-portable");
                assert_eq!(fallen[0].distance, 2);
            }
            _ => panic!("expected stale"),
        }
    }

    #[test]
    fn retired_propagates_like_changed() {
        let (_graph, verdicts) = build_verdicts_from_fixtures();
        let v = verdicts.premise.get("d-keep-legacy").unwrap();
        match v {
            PremiseVerdict::Stale { fallen } => {
                assert_eq!(fallen[0].force_id, "f-retired-old");
                assert_eq!(fallen[0].status, ForceStatus::Retired);
                assert_eq!(fallen[0].distance, 1);
            }
            _ => panic!("expected stale"),
        }
    }

    #[test]
    fn fresh_when_all_premises_hold() {
        let (_graph, verdicts) = build_verdicts_from_fixtures();
        assert!(matches!(
            verdicts.premise.get("d-use-rust").unwrap(),
            PremiseVerdict::Fresh
        ));
        assert!(matches!(
            verdicts.premise.get("d-new-storage").unwrap(),
            PremiseVerdict::Fresh
        ));
    }

    #[test]
    fn dangling_cite_does_not_make_stale() {
        let (_graph, verdicts) = build_verdicts_from_fixtures();
        assert!(matches!(
            verdicts.premise.get("d-dangling").unwrap(),
            PremiseVerdict::Fresh
        ));
    }

    #[test]
    fn superseded_flag_and_frontier() {
        let (_graph, verdicts) = build_verdicts_from_fixtures();
        assert!(verdicts.superseded.contains("d-old-storage"));
        assert!(verdicts.superseded.contains("d-deprecated"));
        assert!(!verdicts.superseded.contains("d-use-rust"));
        assert_eq!(verdicts.frontier.len(), 6);
        let expected_frontier: Vec<&str> = vec![
            "d-dangling",
            "d-embed-onnx",
            "d-keep-legacy",
            "d-new-storage",
            "d-small-model",
            "d-use-rust",
        ];
        assert_eq!(verdicts.frontier, expected_frontier);
    }

    #[test]
    fn cycle_with_fallen_member_terminates_and_propagates() {
        let parsed = vec![
            parse(
                std::path::Path::new("a.md"),
                "---\nid: f-a\ntype: force\ntitle: A\ndependsOn: [f-b]\nstatus_log:\n  - { status: holds, since: 2026-01-01 }\n---\nA\n",
            ),
            parse(
                std::path::Path::new("b.md"),
                "---\nid: f-b\ntype: force\ntitle: B\ndependsOn: [f-a]\nstatus_log:\n  - { status: holds, since: 2026-01-01 }\n  - { status: changed, since: 2026-06-01 }\n---\nB\n",
            ),
            parse(
                std::path::Path::new("d.md"),
                "---\nid: d-cycle-test\ntype: decision\ntitle: Test\nstatus: accepted\ndate: 2026-01-01\ncites: [f-a]\n---\nTest\n",
            ),
        ];
        let linked = crate::linker::link(parsed);
        let graph = Graph::build(&linked);
        let verdicts = judge(&graph);
        let v = verdicts.premise.get("d-cycle-test").unwrap();
        match v {
            PremiseVerdict::Stale { fallen } => {
                assert_eq!(fallen[0].force_id, "f-b");
                assert_eq!(fallen[0].distance, 2);
            }
            _ => panic!("expected stale"),
        }
    }

    #[test]
    fn multiple_fallen_premises_all_reported_at_min_distance() {
        let parsed = vec![
            parse(
                std::path::Path::new("fx.md"),
                "---\nid: f-x\ntype: force\ntitle: X\ndependsOn: [f-z]\nstatus_log:\n  - { status: holds, since: 2026-01-01 }\n---\nX\n",
            ),
            parse(
                std::path::Path::new("fy.md"),
                "---\nid: f-y\ntype: force\ntitle: Y\ndependsOn: [f-z]\nstatus_log:\n  - { status: holds, since: 2026-01-01 }\n---\nY\n",
            ),
            parse(
                std::path::Path::new("fz.md"),
                "---\nid: f-z\ntype: force\ntitle: Z\nstatus_log:\n  - { status: changed, since: 2026-06-01 }\n---\nZ\n",
            ),
            parse(
                std::path::Path::new("d.md"),
                "---\nid: d-multi\ntype: decision\ntitle: Test\nstatus: accepted\ndate: 2026-01-01\ncites: [f-x, f-y]\n---\nTest\n",
            ),
        ];
        let linked = crate::linker::link(parsed);
        let graph = Graph::build(&linked);
        let verdicts = judge(&graph);
        let v = verdicts.premise.get("d-multi").unwrap();
        match v {
            PremiseVerdict::Stale { fallen } => {
                assert_eq!(fallen.len(), 1);
                assert_eq!(fallen[0].force_id, "f-z");
            }
            _ => panic!("expected stale"),
        }
    }

    #[test]
    fn order_independent() {
        let dir = std::path::PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/corpus"
        ));
        let cfg = crate::config::Config::load(&dir.join("forge.toml")).unwrap();
        let (paths, _) = crate::discovery::discover(&cfg.roots);

        let parsed_normal: Vec<_> = paths
            .iter()
            .map(|p| {
                let text = std::fs::read_to_string(p).unwrap();
                parse(p, &text)
            })
            .collect();
        let mut parsed_rev = parsed_normal.clone();
        parsed_rev.reverse();

        let v1 = judge(&Graph::build(&crate::linker::link(parsed_normal)));
        let v2 = judge(&Graph::build(&crate::linker::link(parsed_rev)));

        assert_eq!(v1.frontier, v2.frontier);
        for (id, v) in &v1.premise {
            let v2_val = v2.premise.get(id).unwrap();
            assert_eq!(std::mem::discriminant(v), std::mem::discriminant(v2_val));
        }
    }
}
