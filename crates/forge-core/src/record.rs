use std::path::{Path, PathBuf};

use chrono::NaiveDate;
use serde::Deserialize;

#[derive(Debug, Clone, PartialEq)]
pub enum Parsed {
    Decision(Decision),
    Force(Force),
    Error(ParseError),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Decision {
    pub id: String,
    pub title: String,
    pub status: DecisionStatus,
    pub date: String,
    pub cites: Vec<String>,
    pub supersedes: Vec<String>,
    pub relates: Vec<String>,
    pub anchors: Vec<serde_yaml::Value>,
    pub tags: Vec<String>,
    pub body: String,
    pub path: PathBuf,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Force {
    pub id: String,
    pub title: String,
    pub depends_on: Vec<String>,
    pub status_log: Vec<StatusEntry>,
    pub superseded_by: Option<String>,
    pub tags: Vec<String>,
    pub body: String,
    pub path: PathBuf,
}

impl Force {
    pub fn current_status(&self) -> ForceStatus {
        self.status_log.last().unwrap().status
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ForceStatus {
    Holds,
    Changed,
    Retired,
}

#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DecisionStatus {
    Proposed,
    Accepted,
    Rejected,
    Deprecated,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StatusEntry {
    pub status: ForceStatus,
    pub since: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ParseError {
    pub path: PathBuf,
    pub message: String,
}

#[derive(Deserialize)]
struct RawStatusEntry {
    status: ForceStatus,
    since: String,
}

#[derive(Deserialize)]
struct RawRecord {
    id: String,
    #[serde(rename = "type")]
    rec_type: String,
    title: String,
    #[serde(default)]
    status: Option<DecisionStatus>,
    #[serde(default)]
    date: Option<String>,
    #[serde(default)]
    cites: Vec<String>,
    #[serde(default)]
    supersedes: Vec<String>,
    #[serde(default)]
    relates: Vec<String>,
    #[serde(default)]
    anchors: Vec<serde_yaml::Value>,
    #[serde(rename = "dependsOn", default)]
    depends_on: Vec<String>,
    #[serde(default)]
    status_log: Vec<RawStatusEntry>,
    #[serde(rename = "supersededBy")]
    superseded_by: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
}

fn err(path: PathBuf, message: String) -> Parsed {
    Parsed::Error(ParseError { path, message })
}

fn extract_frontmatter(text: &str) -> Option<(&str, &str)> {
    let text = text
        .strip_prefix("---\n")
        .or_else(|| text.strip_prefix("---\r\n"))?;
    let (close_pos, delim_len) = text
        .find("\r\n---")
        .map(|p| (p, 5))
        .or_else(|| text.find("\n---").map(|p| (p, 4)))?;
    let yaml = &text[..close_pos];
    let body_start = close_pos + delim_len;
    let body = if body_start < text.len() {
        &text[body_start..]
    } else {
        ""
    };
    let body = body
        .strip_prefix('\n')
        .or_else(|| body.strip_prefix("\r\n"))
        .unwrap_or(body);
    Some((yaml, body))
}

pub fn parse(path: &Path, text: &str) -> Parsed {
    let path_buf = path.to_path_buf();

    let (yaml_str, body) = match extract_frontmatter(text) {
        Some(pair) => pair,
        None => {
            return err(path_buf, "missing frontmatter delimiter".to_string());
        }
    };

    let raw: RawRecord = match serde_yaml::from_str(yaml_str) {
        Ok(r) => r,
        Err(e) => return err(path_buf, e.to_string()),
    };

    if raw.id.is_empty() {
        return err(path_buf, "id must not be empty".to_string());
    }

    match raw.rec_type.as_str() {
        "decision" => parse_decision(path_buf, raw, body.to_string()),
        "force" => parse_force(path_buf, raw, body.to_string()),
        other => err(path_buf, format!("unknown record type: {other}")),
    }
}

fn parse_decision(path: PathBuf, raw: RawRecord, body: String) -> Parsed {
    let status = match raw.status {
        Some(s) => s,
        None => return err(path, "decision requires a status field".to_string()),
    };
    let date = match raw.date {
        Some(d) => d,
        None => return err(path, "decision requires a date field".to_string()),
    };

    if NaiveDate::parse_from_str(&date, "%Y-%m-%d").is_err() {
        return err(path, format!("invalid date format: {date}"));
    }

    Parsed::Decision(Decision {
        id: raw.id,
        title: raw.title,
        status,
        date,
        cites: raw.cites,
        supersedes: raw.supersedes,
        relates: raw.relates,
        anchors: raw.anchors,
        tags: raw.tags,
        body,
        path,
    })
}

fn parse_force(path: PathBuf, raw: RawRecord, body: String) -> Parsed {
    if raw.status_log.is_empty() {
        return err(path, "status_log must not be empty".to_string());
    }

    for entry in &raw.status_log {
        if NaiveDate::parse_from_str(&entry.since, "%Y-%m-%d").is_err() {
            return err(path, format!("invalid date in status_log: {}", entry.since));
        }
    }

    for i in 1..raw.status_log.len() {
        let prev = &raw.status_log[i - 1];
        let curr = &raw.status_log[i];

        let prev_date = NaiveDate::parse_from_str(&prev.since, "%Y-%m-%d").unwrap();
        let curr_date = NaiveDate::parse_from_str(&curr.since, "%Y-%m-%d").unwrap();

        if curr_date < prev_date {
            return err(
                path,
                format!(
                    "status_log entries must be in date order: {} is before {}",
                    curr.since, prev.since
                ),
            );
        }
    }

    for i in 1..raw.status_log.len() {
        let prev = raw.status_log[i - 1].status;
        let curr = raw.status_log[i].status;

        if prev == curr {
            return err(
                path,
                format!("consecutive statuses must not be identical: {prev:?} -> {curr:?}"),
            );
        }

        let legal = matches!(
            (prev, curr),
            (ForceStatus::Holds, ForceStatus::Changed)
                | (ForceStatus::Holds, ForceStatus::Retired)
                | (ForceStatus::Changed, ForceStatus::Retired)
        );

        if !legal {
            return err(
                path,
                format!("illegal status transition: {prev:?} -> {curr:?}"),
            );
        }
    }

    let status_log: Vec<StatusEntry> = raw
        .status_log
        .into_iter()
        .map(|e| StatusEntry {
            status: e.status,
            since: e.since,
        })
        .collect();

    Parsed::Force(Force {
        id: raw.id,
        title: raw.title,
        depends_on: raw.depends_on,
        status_log,
        superseded_by: raw.superseded_by,
        tags: raw.tags,
        body,
        path,
    })
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

    #[test]
    fn parses_a_decision() {
        let text = std::fs::read_to_string(fixture("decisions/d-use-rust.md")).unwrap();
        match parse(Path::new("decisions/d-use-rust.md"), &text) {
            Parsed::Decision(d) => {
                assert_eq!(d.id, "d-use-rust");
                assert_eq!(d.status, DecisionStatus::Accepted);
                assert_eq!(d.cites, vec!["f-rust-stable", "f-local-first"]);
                assert_eq!(d.body.trim(), "Single binary, in-process embedding.");
            }
            other => panic!("expected decision, got {other:#?}"),
        }
    }

    #[test]
    fn parses_a_force_with_status_log() {
        let text = std::fs::read_to_string(fixture("forces/f-onnx-portable.md")).unwrap();
        match parse(Path::new("forces/f-onnx-portable.md"), &text) {
            Parsed::Force(f) => {
                assert_eq!(f.id, "f-onnx-portable");
                assert_eq!(f.status_log.len(), 2);
                assert_eq!(f.current_status(), ForceStatus::Changed);
                assert_eq!(f.status_log[1].since, "2026-06-01");
            }
            other => panic!("expected force, got {other:#?}"),
        }
    }

    #[test]
    fn malformed_yaml_is_a_parse_error_not_a_panic() {
        let text = std::fs::read_to_string(fixture("forces/malformed.md")).unwrap();
        match parse(Path::new("forces/malformed.md"), &text) {
            Parsed::Error(e) => {
                assert!(!e.message.is_empty());
            }
            other => panic!("expected parse error, got {other:#?}"),
        }
    }

    #[test]
    fn empty_status_log_is_a_parse_error() {
        let raw = "---\nid: f-test\ntype: force\ntitle: Test\nstatus_log: []\n---\nBody\n";
        match parse(Path::new("test.md"), raw) {
            Parsed::Error(e) => assert!(
                e.message.contains("status_log") || e.message.contains("empty"),
                "expected status_log error, got: {}",
                e.message
            ),
            other => panic!("expected error, got {other:#?}"),
        }
    }

    #[test]
    fn illegal_status_regression_is_a_parse_error() {
        let raw = "---\nid: f-test\ntype: force\ntitle: Test\nstatus_log:\n  - { status: retired, since: 2026-01-01 }\n  - { status: holds, since: 2026-02-01 }\n---\nBody\n";
        match parse(Path::new("test.md"), raw) {
            Parsed::Error(e) => assert!(
                e.message.contains("transition")
                    || e.message.contains("illegal")
                    || e.message.contains("regression")
                    || e.message.contains("resurrection"),
                "expected transition error, got: {}",
                e.message
            ),
            other => panic!("expected error, got {other:#?}"),
        }
    }

    #[test]
    fn unknown_type_is_a_parse_error() {
        let raw = "---\nid: f-test\ntype: banana\ntitle: Test\n---\nBody\n";
        match parse(Path::new("test.md"), raw) {
            Parsed::Error(e) => assert!(
                e.message.contains("type") || e.message.contains("unknown"),
                "expected type error, got: {}",
                e.message
            ),
            _ => panic!("expected error"),
        }
    }

    #[test]
    fn anchors_are_carried_opaquely() {
        let raw = "---\nid: d-test\ntype: decision\ntitle: Test\nstatus: accepted\ndate: 2026-01-01\nanchors:\n  - {file: src/x.rs, symbol: Foo}\n---\nBody\n";
        match parse(Path::new("test.md"), raw) {
            Parsed::Decision(d) => assert_eq!(d.anchors.len(), 1),
            other => panic!("expected decision, got {other:#?}"),
        }
    }

    #[test]
    fn status_log_entries_not_in_date_order_is_an_error() {
        let raw = "---\nid: f-test\ntype: force\ntitle: Test\nstatus_log:\n  - { status: holds, since: 2026-02-01 }\n  - { status: holds, since: 2026-01-01 }\n---\nBody\n";
        match parse(Path::new("test.md"), raw) {
            Parsed::Error(e) => assert!(
                e.message.contains("date") || e.message.contains("order"),
                "expected date order error, got: {}",
                e.message
            ),
            other => panic!("expected error, got {other:#?}"),
        }
    }

    #[test]
    fn identical_consecutive_statuses_is_rejected() {
        let raw = "---\nid: f-test\ntype: force\ntitle: Test\nstatus_log:\n  - { status: holds, since: 2026-01-01 }\n  - { status: holds, since: 2026-02-01 }\n---\nBody\n";
        match parse(Path::new("test.md"), raw) {
            Parsed::Error(e) => assert!(
                e.message.contains("consecutive") || e.message.contains("duplicate"),
                "expected consecutive status error, got: {}",
                e.message
            ),
            other => panic!("expected error, got {other:#?}"),
        }
    }
}
