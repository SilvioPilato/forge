use std::path::PathBuf;

use tracing::{debug, instrument};

use crate::linker::Diagnostic;

/// Walk roots, find candidate .md files by checking for frontmatter with `type:` field.
/// Returns (paths, diagnostics). Paths are sorted deterministically.
#[instrument]
pub fn discover(roots: &[PathBuf]) -> (Vec<PathBuf>, Vec<Diagnostic>) {
    let mut paths = Vec::new();
    let mut diagnostics = Vec::new();

    for root in roots {
        if !root.exists() {
            diagnostics.push(Diagnostic::MissingRoot { path: root.clone() });
            continue;
        }
        let before = paths.len();
        for entry in walkdir::WalkDir::new(root)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            if !entry.file_type().is_file() {
                continue;
            }
            let path = entry.path();
            if path.extension().is_none_or(|e| e != "md") {
                continue;
            }
            if let Ok(content) = std::fs::read_to_string(path) {
                if has_record_frontmatter(&content) {
                    paths.push(path.to_path_buf());
                }
            }
        }
        let count = paths.len() - before;
        debug!(root = %root.display(), count = count, "discovered records under root");
    }

    paths.sort();
    debug!(total = paths.len(), "discovery complete");
    (paths, diagnostics)
}

/// Quick check: does this text have frontmatter with a `type:` field that looks like a record?
/// Tolerate YAML errors — malformed.md should still reach the record layer.
fn has_record_frontmatter(text: &str) -> bool {
    let text_trimmed = text.trim_start();
    if !text_trimmed.starts_with("---") {
        return false;
    }
    let rest = &text_trimmed[3..];
    for line in rest.lines() {
        let trimmed = line.trim();
        if trimmed == "---" {
            return false;
        }
        if let Some(value) = trimmed.strip_prefix("type:") {
            let value = value.trim();
            return value == "decision" || value == "force";
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovers_exactly_the_corpus_files() {
        let dir = std::path::Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/corpus"
        ));
        let cfg = crate::config::Config::load(&dir.join("forge.toml")).unwrap();
        let (paths, _diags) = discover(&cfg.roots);
        assert_eq!(paths.len(), 19);
        for p in &paths {
            assert!(p.extension().is_some_and(|e| e == "md"));
        }
        let mut sorted = paths.clone();
        sorted.sort();
        assert_eq!(paths, sorted);
        assert!(paths
            .iter()
            .any(|p| p.file_name().unwrap() == "malformed.md"));
    }

    #[test]
    fn non_record_markdown_is_ignored() {
        use std::io::Write;
        let dir = std::env::temp_dir().join("forge-discovery-test-nonrecord");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut f = std::fs::File::create(dir.join("README.md")).unwrap();
        writeln!(f, "# README\n\nNo frontmatter here.").unwrap();
        let (paths, _diags) = discover(std::slice::from_ref(&dir));
        assert!(paths.is_empty(), "non-record MD files should be ignored");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_root_is_a_diagnostic_not_an_error() {
        let missing = std::path::PathBuf::from("/nonexistent/path/that/does/not/exist");
        let (paths, diags) = discover(&[missing]);
        assert!(paths.is_empty());
        assert!(!diags.is_empty());
    }
}
