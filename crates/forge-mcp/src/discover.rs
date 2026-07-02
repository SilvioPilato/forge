use std::path::{Path, PathBuf};

/// Resolve the forge.toml path. Ladder, first hit wins:
/// 1. explicit (--config flag or positional arg)
/// 2. env (FORGE_CONFIG)
/// 3. walk up from cwd; a directory containing `.git` is the last one checked
///
/// Returns `None` when no forge.toml exists within the `.git` boundary —
/// a valid state meaning "no corpus yet" (empty mode).
pub fn resolve_config(
    explicit: Option<PathBuf>,
    env_value: Option<PathBuf>,
    cwd: &Path,
) -> Option<PathBuf> {
    if let Some(p) = explicit {
        return Some(p);
    }
    if let Some(p) = env_value {
        return Some(p);
    }
    let mut dir = Some(cwd);
    while let Some(d) = dir {
        let candidate = d.join("forge.toml");
        if candidate.is_file() {
            return Some(candidate);
        }
        if d.join(".git").exists() {
            break;
        }
        dir = d.parent();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_path_wins_over_env_and_walk() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("forge.toml"), "").unwrap();
        let explicit = PathBuf::from("explicit/forge.toml");
        let env = Some(PathBuf::from("env/forge.toml"));
        let got = resolve_config(Some(explicit.clone()), env, tmp.path()).unwrap();
        assert_eq!(got, explicit);
    }

    #[test]
    fn env_wins_over_walk() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("forge.toml"), "").unwrap();
        let env = PathBuf::from("env/forge.toml");
        let got = resolve_config(None, Some(env.clone()), tmp.path()).unwrap();
        assert_eq!(got, env);
    }

    #[test]
    fn walk_finds_config_in_ancestor() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("forge.toml"), "").unwrap();
        let nested = tmp.path().join("a").join("b");
        std::fs::create_dir_all(&nested).unwrap();
        let got = resolve_config(None, None, &nested).unwrap();
        assert_eq!(got, tmp.path().join("forge.toml"));
    }

    #[test]
    fn config_in_repo_root_is_found_even_with_git_marker() {
        // .git and forge.toml in the same directory: forge.toml wins
        // (candidate is checked before the boundary cuts the walk).
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".git")).unwrap();
        std::fs::write(tmp.path().join("forge.toml"), "").unwrap();
        let nested = tmp.path().join("src");
        std::fs::create_dir_all(&nested).unwrap();
        let got = resolve_config(None, None, &nested).unwrap();
        assert_eq!(got, tmp.path().join("forge.toml"));
    }

    #[test]
    fn miss_returns_none() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".git")).unwrap();
        assert!(resolve_config(None, None, tmp.path()).is_none());
    }

    #[test]
    fn walk_stops_at_git_boundary() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("forge.toml"), "").unwrap();
        let repo = tmp.path().join("repo");
        let nested = repo.join("src");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        assert!(resolve_config(None, None, &nested).is_none());
    }
}
