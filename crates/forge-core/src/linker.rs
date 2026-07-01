use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use crate::record::{Decision, Force, Parsed};

#[derive(Debug, Clone, PartialEq)]
pub enum RefField {
    Cites,
    Supersedes,
    Relates,
    DependsOn,
    SupersededBy,
}

#[derive(Debug, Clone)]
pub enum Diagnostic {
    ParseError {
        path: PathBuf,
        message: String,
    },
    MissingRoot {
        path: PathBuf,
    },
    IdCollision {
        id: String,
        paths: Vec<PathBuf>,
    },
    DanglingRef {
        from: String,
        field: RefField,
        to: String,
    },
    DependsOnCycle {
        members: Vec<String>,
    },
}

pub struct Linked {
    pub decisions: Vec<Decision>,
    pub forces: Vec<Force>,
    pub diagnostics: Vec<Diagnostic>,
}

pub fn link(parsed: Vec<Parsed>) -> Linked {
    let mut diagnostics = Vec::new();

    let mut decisions: Vec<Decision> = Vec::new();
    let mut forces: Vec<Force> = Vec::new();

    for p in parsed {
        match p {
            Parsed::Decision(d) => decisions.push(d),
            Parsed::Force(f) => forces.push(f),
            Parsed::Error(e) => {
                diagnostics.push(Diagnostic::ParseError {
                    path: e.path,
                    message: e.message,
                });
            }
        }
    }

    decisions.sort_by(|a, b| a.path.cmp(&b.path));
    forces.sort_by(|a, b| a.path.cmp(&b.path));

    let mut id_map: HashMap<String, PathBuf> = HashMap::new();
    let mut collision_groups: HashMap<String, Vec<PathBuf>> = HashMap::new();

    let mut decisions_kept = Vec::new();
    let mut forces_kept = Vec::new();

    for d in decisions {
        if let Some(existing_path) = id_map.get(&d.id) {
            collision_groups
                .entry(d.id.clone())
                .or_insert_with(|| vec![existing_path.clone()])
                .push(d.path.clone());
        } else {
            id_map.insert(d.id.clone(), d.path.clone());
            decisions_kept.push(d);
        }
    }

    for f in forces {
        if let Some(existing_path) = id_map.get(&f.id) {
            collision_groups
                .entry(f.id.clone())
                .or_insert_with(|| vec![existing_path.clone()])
                .push(f.path.clone());
        } else {
            id_map.insert(f.id.clone(), f.path.clone());
            forces_kept.push(f);
        }
    }

    for (id, paths) in collision_groups {
        diagnostics.push(Diagnostic::IdCollision { id, paths });
    }

    for d in &decisions_kept {
        for cite in &d.cites {
            if !id_map.contains_key(cite) {
                diagnostics.push(Diagnostic::DanglingRef {
                    from: d.id.clone(),
                    field: RefField::Cites,
                    to: cite.clone(),
                });
            }
        }
        for s in &d.supersedes {
            if !id_map.contains_key(s) {
                diagnostics.push(Diagnostic::DanglingRef {
                    from: d.id.clone(),
                    field: RefField::Supersedes,
                    to: s.clone(),
                });
            }
        }
        for r in &d.relates {
            if !id_map.contains_key(r) {
                diagnostics.push(Diagnostic::DanglingRef {
                    from: d.id.clone(),
                    field: RefField::Relates,
                    to: r.clone(),
                });
            }
        }
    }

    for f in &forces_kept {
        for dep in &f.depends_on {
            if !id_map.contains_key(dep) {
                diagnostics.push(Diagnostic::DanglingRef {
                    from: f.id.clone(),
                    field: RefField::DependsOn,
                    to: dep.clone(),
                });
            }
        }
        if let Some(ref sb) = f.superseded_by {
            if !id_map.contains_key(sb) {
                diagnostics.push(Diagnostic::DanglingRef {
                    from: f.id.clone(),
                    field: RefField::SupersededBy,
                    to: sb.clone(),
                });
            }
        }
    }

    detect_cycles(&forces_kept, &mut diagnostics);

    Linked {
        decisions: decisions_kept,
        forces: forces_kept,
        diagnostics,
    }
}

fn detect_cycles(forces: &[Force], diagnostics: &mut Vec<Diagnostic>) {
    let force_ids: HashSet<String> = forces.iter().map(|f| f.id.clone()).collect();
    let mut adj: HashMap<String, Vec<String>> = HashMap::new();
    for f in forces {
        let deps: Vec<String> = f
            .depends_on
            .iter()
            .filter(|d| force_ids.contains(*d))
            .cloned()
            .collect();
        adj.insert(f.id.clone(), deps);
    }

    let mut color: HashMap<String, u8> = HashMap::new();
    let mut cycles: HashSet<Vec<String>> = HashSet::new();

    for id in &force_ids {
        if !color.contains_key(id) {
            let mut path: Vec<String> = Vec::new();
            dfs_cycle(id, &adj, &mut color, &mut path, &mut cycles);
        }
    }

    let mut cycles_sorted: Vec<Vec<String>> = cycles.into_iter().collect();
    cycles_sorted.sort();
    for cycle in cycles_sorted {
        diagnostics.push(Diagnostic::DependsOnCycle { members: cycle });
    }
}

fn dfs_cycle(
    node: &str,
    adj: &HashMap<String, Vec<String>>,
    color: &mut HashMap<String, u8>,
    path: &mut Vec<String>,
    cycles: &mut HashSet<Vec<String>>,
) {
    color.insert(node.to_string(), 1);
    path.push(node.to_string());

    if let Some(neighbors) = adj.get(node) {
        for neighbor in neighbors {
            let c = color.get(neighbor.as_str()).copied().unwrap_or(0);
            if c == 1 {
                if let Some(pos) = path.iter().position(|x| x == neighbor) {
                    let mut cycle: Vec<String> = path[pos..].to_vec();
                    cycle.sort();
                    cycles.insert(cycle);
                }
            } else if c == 0 {
                dfs_cycle(neighbor, adj, color, path, cycles);
            }
        }
    }

    path.pop();
    color.insert(node.to_string(), 2);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(rel: &str) -> std::path::PathBuf {
        std::path::PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/corpus"
        ))
        .join(rel)
    }

    fn parse_corpus_files() -> Vec<Parsed> {
        let dir = fixture("");
        let cfg = crate::config::Config::load(&dir.join("forge.toml")).unwrap();
        let (paths, _) = crate::discovery::discover(&cfg.roots);
        paths
            .iter()
            .map(|p| {
                let text = std::fs::read_to_string(p).unwrap();
                crate::record::parse(p, &text)
            })
            .collect()
    }

    #[test]
    fn detects_id_collision() {
        let parsed = parse_corpus_files();
        let linked = link(parsed);
        let collisions: Vec<_> = linked
            .diagnostics
            .iter()
            .filter(|d| matches!(d, Diagnostic::IdCollision { .. }))
            .collect();
        assert_eq!(collisions.len(), 1);
        match &collisions[0] {
            Diagnostic::IdCollision { id, paths } => {
                assert_eq!(id, "f-duplicate");
                assert_eq!(paths.len(), 2);
            }
            _ => panic!("expected IdCollision"),
        }
    }

    #[test]
    fn detects_dangling_refs_per_field() {
        let parsed = parse_corpus_files();
        let linked = link(parsed);
        let dangling: Vec<_> = linked
            .diagnostics
            .iter()
            .filter(|d| matches!(d, Diagnostic::DanglingRef { .. }))
            .collect();
        assert_eq!(dangling.len(), 1);
        match &dangling[0] {
            Diagnostic::DanglingRef { from, field, to } => {
                assert_eq!(from, "d-dangling");
                assert_eq!(to, "f-missing");
                assert!(matches!(field, RefField::Cites));
            }
            _ => panic!("expected DanglingRef"),
        }
    }

    #[test]
    fn detects_depends_on_cycle() {
        let parsed = parse_corpus_files();
        let linked = link(parsed);
        let cycles: Vec<_> = linked
            .diagnostics
            .iter()
            .filter(|d| matches!(d, Diagnostic::DependsOnCycle { .. }))
            .collect();
        assert_eq!(cycles.len(), 1);
        match &cycles[0] {
            Diagnostic::DependsOnCycle { members } => {
                assert!(members.contains(&"f-cycle-a".to_string()));
                assert!(members.contains(&"f-cycle-b".to_string()));
            }
            _ => panic!("expected DependsOnCycle"),
        }
    }

    #[test]
    fn anchors_never_produce_dangling() {
        let raw = "---\nid: d-anchor-test\ntype: decision\ntitle: Test\nstatus: accepted\ndate: 2026-01-01\nanchors:\n  - {file: nowhere.rs, symbol: Nope}\n---\nBody\n";
        let linked = link(vec![crate::record::parse(
            std::path::Path::new("test.md"),
            raw,
        )]);
        assert!(linked.diagnostics.is_empty());
    }

    #[test]
    fn collision_keeps_first_by_path_order_and_reports() {
        let f1 = "---\nid: f-collision\ntype: force\ntitle: First\nstatus_log:\n  - { status: holds, since: 2026-01-01 }\n---\nFirst\n";
        let f2 = "---\nid: f-collision\ntype: force\ntitle: Second\nstatus_log:\n  - { status: holds, since: 2026-01-01 }\n---\nSecond\n";
        let parsed = vec![
            crate::record::parse(std::path::Path::new("a.md"), f1),
            crate::record::parse(std::path::Path::new("b.md"), f2),
        ];
        let linked = link(parsed);
        assert_eq!(linked.forces.len(), 1);
        assert_eq!(linked.forces[0].title, "First");
    }
}
