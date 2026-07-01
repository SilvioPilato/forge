use crate::linker::Linked;
use crate::record::{Decision, Force};
use std::collections::HashMap;

pub enum Record {
    Decision(Decision),
    Force(Force),
}

pub struct Graph {
    records: Vec<Record>,
    by_id: HashMap<String, usize>,
    cited_by: HashMap<String, Vec<String>>,
    depended_by: HashMap<String, Vec<String>>,
    superseded_by: HashMap<String, Vec<String>>,
}

impl Graph {
    pub fn build(linked: &Linked) -> Graph {
        let mut records = Vec::new();
        let mut by_id = HashMap::new();
        let mut cited_by: HashMap<String, Vec<String>> = HashMap::new();
        let mut depended_by: HashMap<String, Vec<String>> = HashMap::new();
        let mut superseded_by: HashMap<String, Vec<String>> = HashMap::new();

        for d in &linked.decisions {
            let idx = records.len();
            by_id.insert(d.id.clone(), idx);
            for cite in &d.cites {
                cited_by.entry(cite.clone()).or_default().push(d.id.clone());
            }
            for sid in &d.supersedes {
                superseded_by
                    .entry(sid.clone())
                    .or_default()
                    .push(d.id.clone());
            }
            records.push(Record::Decision(d.clone()));
        }

        for f in &linked.forces {
            let idx = records.len();
            by_id.insert(f.id.clone(), idx);
            for dep in &f.depends_on {
                depended_by
                    .entry(dep.clone())
                    .or_default()
                    .push(f.id.clone());
            }
            records.push(Record::Force(f.clone()));
        }

        for v in cited_by.values_mut() {
            v.sort();
        }
        for v in depended_by.values_mut() {
            v.sort();
        }
        for v in superseded_by.values_mut() {
            v.sort();
        }

        Graph {
            records,
            by_id,
            cited_by,
            depended_by,
            superseded_by,
        }
    }

    pub fn get(&self, id: &str) -> Option<&Record> {
        self.by_id.get(id).map(|&i| &self.records[i])
    }

    pub fn cited_by(&self, force_id: &str) -> Vec<String> {
        self.cited_by.get(force_id).cloned().unwrap_or_default()
    }

    pub fn depended_by(&self, force_id: &str) -> Vec<String> {
        self.depended_by.get(force_id).cloned().unwrap_or_default()
    }

    pub fn superseded_by(&self, decision_id: &str) -> Vec<String> {
        self.superseded_by
            .get(decision_id)
            .cloned()
            .unwrap_or_default()
    }

    pub fn decisions(&self) -> Vec<&Decision> {
        self.records
            .iter()
            .filter_map(|r| match r {
                Record::Decision(d) => Some(d),
                _ => None,
            })
            .collect()
    }

    pub fn forces(&self) -> Vec<&Force> {
        self.records
            .iter()
            .filter_map(|r| match r {
                Record::Force(f) => Some(f),
                _ => None,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::linker::link;
    use crate::record::parse;
    use std::path::PathBuf;

    fn fixture(rel: &str) -> PathBuf {
        PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/corpus"
        ))
        .join(rel)
    }

    fn build_graph_from_fixtures() -> Graph {
        let dir = fixture("");
        let cfg = Config::load(&dir.join("forge.toml")).unwrap();
        let (paths, _) = crate::discovery::discover(&cfg.roots);
        let parsed: Vec<_> = paths
            .iter()
            .map(|p| {
                let text = std::fs::read_to_string(p).unwrap();
                parse(p, &text)
            })
            .collect();
        let linked = link(parsed);
        Graph::build(&linked)
    }

    #[test]
    fn reverse_cites_index() {
        let graph = build_graph_from_fixtures();
        let cited = graph.cited_by("f-retired-old");
        assert!(cited.contains(&"d-keep-legacy".to_string()));
        assert!(cited.contains(&"d-old-storage".to_string()));
        assert_eq!(cited.len(), 2);
    }

    #[test]
    fn depends_on_reverse_index() {
        let graph = build_graph_from_fixtures();
        let dep = graph.depended_by("f-onnx-portable");
        assert_eq!(dep, vec!["f-model-small"]);
    }

    #[test]
    fn supersession_index() {
        let graph = build_graph_from_fixtures();
        let sup = graph.superseded_by("d-old-storage");
        assert_eq!(sup, vec!["d-new-storage"]);
    }

    #[test]
    fn lookup_by_id() {
        let graph = build_graph_from_fixtures();
        assert!(graph.get("d-use-rust").is_some());
        assert!(graph.get("f-missing").is_none());
    }

    #[test]
    fn get_decision_returns_correct_type() {
        let graph = build_graph_from_fixtures();
        match graph.get("d-use-rust") {
            Some(Record::Decision(d)) => assert_eq!(d.title, "Build the engine in Rust"),
            _ => panic!("expected decision"),
        }
    }
}
