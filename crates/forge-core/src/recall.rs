use crate::embed::{cosine, EmbedError, Embedder};
use crate::graph::Record;
use crate::record::ForceStatus;
use crate::snapshot::Snapshot;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone)]
pub enum Scope {
    Force,
    Decision,
    Both,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hit {
    pub id: String,
    pub title: String,
    pub score: f32,
    pub kind: String,
    pub status: String,
    pub superseded_by: Option<String>,
}

pub fn search(
    snap: &Snapshot,
    embedder: &dyn Embedder,
    query: &str,
    scope: Scope,
    limit: usize,
) -> Result<Vec<Hit>, EmbedError> {
    let query_vec = embedder.embed_query(query)?;
    let mut hits = Vec::new();

    let frontier_set: std::collections::HashSet<&str> =
        snap.frontier().iter().map(|s| s.as_str()).collect();

    for (id, vec) in &snap.vectors {
        if vec.is_empty() {
            continue;
        }
        let record = match snap.graph.get(id) {
            Some(r) => r,
            None => continue,
        };

        match record {
            Record::Decision(d) => {
                if !matches!(scope, Scope::Force) && frontier_set.contains(id.as_str()) {
                    let score = cosine(&query_vec, vec);
                    hits.push(Hit {
                        id: d.id.clone(),
                        title: d.title.clone(),
                        score,
                        kind: "decision".to_string(),
                        status: format!("{:?}", d.status).to_lowercase(),
                        superseded_by: None,
                    });
                }
            }
            Record::Force(f) => {
                if !matches!(scope, Scope::Decision)
                    && f.current_status() != ForceStatus::Retired
                    && f.superseded_by.is_none()
                {
                    let score = cosine(&query_vec, vec);
                    hits.push(Hit {
                        id: f.id.clone(),
                        title: f.title.clone(),
                        score,
                        kind: "force".to_string(),
                        status: format!("{:?}", f.current_status()).to_lowercase(),
                        superseded_by: f.superseded_by.clone(),
                    });
                }
            }
        }
    }

    hits.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.id.cmp(&b.id))
    });
    if limit > 0 && hits.len() > limit {
        hits.truncate(limit);
    }
    Ok(hits)
}

pub fn near_matches(
    snap: &Snapshot,
    embedder: &dyn Embedder,
    force_title: &str,
    warn: f32,
) -> Result<Vec<Hit>, EmbedError> {
    let query_vec = embedder.embed_query(force_title)?;
    let mut hits = Vec::new();

    for f in snap.graph.forces() {
        if let Some(vec) = snap.vectors.get(&f.id) {
            if vec.is_empty() {
                continue;
            }
            let score = cosine(&query_vec, vec);
            if score >= warn {
                hits.push(Hit {
                    id: f.id.clone(),
                    title: f.title.clone(),
                    score,
                    kind: "force".to_string(),
                    status: format!("{:?}", f.current_status()).to_lowercase(),
                    superseded_by: f.superseded_by.clone(),
                });
            }
        }
    }

    hits.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.id.cmp(&b.id))
    });
    Ok(hits)
}
