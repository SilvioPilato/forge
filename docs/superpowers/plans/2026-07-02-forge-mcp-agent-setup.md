# forge-mcp Agent-Ready Setup Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `forge-mcp` zero-config under MCP clients: automatic `forge.toml` discovery (flag → env → cwd walk), an `init` scaffolder, and setup docs.

**Architecture:** Two new leaf modules in the `forge-mcp` binary crate — `discover` (pure config-resolution ladder, no global state, fully unit-testable) and `scaffold` (corpus init). `main.rs` gains a clap CLI that dispatches to them; the MCP serving path is unchanged after config resolution. Spec: `docs/superpowers/specs/2026-07-02-forge-mcp-agent-setup-design.md`.

**Tech Stack:** Rust, clap 4 (derive), tempfile (dev-only), existing rmcp/tokio stack untouched.

**Constraints verified against the codebase:**
- `forge_core::config::Config::load(&Path)` canonicalizes root dirs and fails if they don't exist — `init` MUST create `decisions/` and `forces/`, and a test must prove the scaffolded corpus loads.
- `crates/forge-mcp/tests/acceptance.rs:44-45` spawns the binary with a bare positional config path — that invocation form must keep working.
- Backward compat: `forge-mcp <path>` ≡ `forge-mcp --config <path>`; supplying both is a clap error (`conflicts_with`). A path literally named `init` is shadowed by the subcommand (accepted per spec).

---

### Task 1: Dependencies

**Files:**
- Modify: `crates/forge-mcp/Cargo.toml`

- [ ] **Step 1: Add clap and tempfile**

Append to the `[dependencies]` section and add a dev-dependencies section in `crates/forge-mcp/Cargo.toml`:

```toml
clap = { version = "4", features = ["derive"] }
```

```toml
[dev-dependencies]
tempfile = "3"
```

- [ ] **Step 2: Verify it builds**

Run: `cargo check -p forge-mcp`
Expected: clean check (warnings about unused deps are fine at this point; clap is not referenced yet).

- [ ] **Step 3: Commit**

```bash
git add crates/forge-mcp/Cargo.toml Cargo.lock
git commit -m "chore: add clap and tempfile to forge-mcp"
```

---

### Task 2: Config discovery ladder (`discover` module)

**Files:**
- Create: `crates/forge-mcp/src/discover.rs`
- Modify: `crates/forge-mcp/src/main.rs` (add `mod discover;` at top)

Design note: `resolve_config` takes the env value and cwd as *parameters* instead of reading globals — this keeps every rung of the ladder testable without mutating process state (env-var mutation in Rust tests is UB-adjacent and flaky under parallel test execution).

- [ ] **Step 1: Create the module with a stub and failing tests**

Create `crates/forge-mcp/src/discover.rs`:

```rust
use std::path::{Path, PathBuf};

/// Resolve the forge.toml path. Ladder, first hit wins:
/// 1. explicit (--config flag or positional arg)
/// 2. env (FORGE_CONFIG)
/// 3. walk up from cwd; a directory containing `.git` is the last one checked
pub fn resolve_config(
    explicit: Option<PathBuf>,
    env_value: Option<PathBuf>,
    cwd: &Path,
) -> Result<PathBuf, String> {
    let _ = (explicit, env_value, cwd);
    Err("unimplemented".to_string())
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
    fn walk_stops_at_git_boundary() {
        // forge.toml above the repo boundary must NOT be picked up.
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("forge.toml"), "").unwrap();
        let repo = tmp.path().join("repo");
        let nested = repo.join("src");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        let err = resolve_config(None, None, &nested).unwrap_err();
        assert!(err.contains("No forge.toml found"), "got: {err}");
    }

    #[test]
    fn miss_produces_actionable_error() {
        let tmp = tempfile::tempdir().unwrap();
        // .git marker keeps the walk from escaping the temp dir on a
        // developer machine that has a forge.toml somewhere above it.
        std::fs::create_dir_all(tmp.path().join(".git")).unwrap();
        let err = resolve_config(None, None, tmp.path()).unwrap_err();
        assert!(err.contains("forge-mcp init"), "got: {err}");
        assert!(err.contains("--config"), "got: {err}");
        assert!(err.contains("FORGE_CONFIG"), "got: {err}");
    }
}
```

Add `mod discover;` as the first line of `crates/forge-mcp/src/main.rs`.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p forge-mcp discover`
Expected: all 6 tests FAIL — the four success-path tests on `unwrap`, the two error-path tests on their message asserts (the stub's message is "unimplemented").

- [ ] **Step 3: Implement the ladder**

Replace the stub body of `resolve_config`:

```rust
pub fn resolve_config(
    explicit: Option<PathBuf>,
    env_value: Option<PathBuf>,
    cwd: &Path,
) -> Result<PathBuf, String> {
    if let Some(p) = explicit {
        return Ok(p);
    }
    if let Some(p) = env_value {
        return Ok(p);
    }
    let mut dir = Some(cwd);
    while let Some(d) = dir {
        let candidate = d.join("forge.toml");
        if candidate.is_file() {
            return Ok(candidate);
        }
        // `.git` may be a directory or a file (worktrees); either marks the
        // repo boundary. The boundary directory itself was just checked, so
        // stop here rather than escape into unrelated parents.
        if d.join(".git").exists() {
            break;
        }
        dir = d.parent();
    }
    Err(format!(
        "No forge.toml found (searched upward from {}).\n\
         Fix: pass --config <path>, set FORGE_CONFIG, or run `forge-mcp init` to scaffold a corpus.",
        cwd.display()
    ))
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p forge-mcp discover`
Expected: all 6 tests PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/forge-mcp/src/discover.rs crates/forge-mcp/src/main.rs
git commit -m "feat: forge.toml discovery ladder (explicit/env/cwd walk with .git boundary)"
```

---

### Task 3: Corpus scaffolder (`scaffold` module)

**Files:**
- Create: `crates/forge-mcp/src/scaffold.rs`
- Modify: `crates/forge-mcp/src/main.rs` (add `mod scaffold;`)

- [ ] **Step 1: Create the module with a stub and failing tests**

Create `crates/forge-mcp/src/scaffold.rs`:

```rust
use std::path::{Path, PathBuf};

const FORGE_TOML_TEMPLATE: &str = r#"roots = ["decisions", "forces"]

[dedup]
reuse = 0.90  # cosine threshold: silently reuse existing force
warn = 0.75   # cosine threshold: warn about near-duplicate

[embedding]
# "fake-bucket" is a deterministic test embedder. The real model below
# downloads ~120MB from Hugging Face on first run, then uses the local cache.
model = "intfloat/multilingual-e5-small"
"#;

/// Scaffold a new forge corpus in `dir`: forge.toml + decisions/ + forces/.
/// Creates `dir` (and intermediates) if missing. Refuses to overwrite an
/// existing forge.toml. Returns the path of the created forge.toml.
pub fn init(dir: &Path) -> Result<PathBuf, String> {
    let _ = dir;
    Err("unimplemented".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_scaffolds_config_and_dirs_that_load() {
        let tmp = tempfile::tempdir().unwrap();
        let config_path = init(tmp.path()).unwrap();
        assert_eq!(config_path, tmp.path().join("forge.toml"));
        assert!(tmp.path().join("decisions").is_dir());
        assert!(tmp.path().join("forces").is_dir());
        // Load-bearing: Config::load canonicalizes roots and fails if the
        // directories are missing, so this proves the scaffold is coherent.
        let cfg = forge_core::config::Config::load(&config_path).unwrap();
        assert_eq!(cfg.embedding.model, "intfloat/multilingual-e5-small");
        assert_eq!(cfg.roots.len(), 2);
        assert!((cfg.dedup.reuse - 0.90).abs() < 1e-4);
        assert!((cfg.dedup.warn - 0.75).abs() < 1e-4);
    }

    #[test]
    fn init_creates_missing_target_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("new").join("nested");
        let config_path = init(&target).unwrap();
        assert!(config_path.is_file());
        assert!(target.join("decisions").is_dir());
    }

    #[test]
    fn init_refuses_existing_config() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("forge.toml"), "# existing").unwrap();
        let err = init(tmp.path()).unwrap_err();
        assert!(err.contains("already exists"), "got: {err}");
        let content = std::fs::read_to_string(tmp.path().join("forge.toml")).unwrap();
        assert_eq!(content, "# existing");
    }
}
```

Add `mod scaffold;` below `mod discover;` in `crates/forge-mcp/src/main.rs`.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p forge-mcp scaffold`
Expected: all 3 tests FAIL (stub returns Err / error message mismatch).

- [ ] **Step 3: Implement init**

Replace the stub body of `init`:

```rust
pub fn init(dir: &Path) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir)
        .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let config_path = dir.join("forge.toml");
    if config_path.exists() {
        return Err(format!(
            "{} already exists; refusing to overwrite",
            config_path.display()
        ));
    }
    for sub in ["decisions", "forces"] {
        let d = dir.join(sub);
        std::fs::create_dir_all(&d)
            .map_err(|e| format!("cannot create {}: {e}", d.display()))?;
    }
    std::fs::write(&config_path, FORGE_TOML_TEMPLATE)
        .map_err(|e| format!("cannot write {}: {e}", config_path.display()))?;
    Ok(config_path)
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p forge-mcp scaffold`
Expected: all 3 tests PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/forge-mcp/src/scaffold.rs crates/forge-mcp/src/main.rs
git commit -m "feat: forge-mcp init scaffolds a loadable corpus"
```

---

### Task 4: CLI wiring in main.rs

**Files:**
- Modify: `crates/forge-mcp/src/main.rs` (imports at top, `main()` at `crates/forge-mcp/src/main.rs:305-341`)

The tool handlers and `ForgeServer` are untouched; only argument handling changes. Behavior is verified by the existing acceptance test (positional form) plus manual smoke checks, since `main()` itself has no unit-test seam — the logic worth unit-testing already lives in `discover`/`scaffold`.

- [ ] **Step 1: Add the clap CLI types and rewrite main()**

Add imports near the top of `crates/forge-mcp/src/main.rs`:

```rust
use clap::{Parser, Subcommand};
use std::path::PathBuf;
```

Add above `main()`:

```rust
#[derive(Parser)]
#[command(name = "forge-mcp", version, about = "Forge MCP server over stdio")]
struct Cli {
    /// Path to forge.toml (same as --config; kept for backward compatibility)
    #[arg(value_name = "CONFIG", conflicts_with = "config")]
    positional_config: Option<PathBuf>,

    /// Path to forge.toml
    #[arg(long, value_name = "PATH")]
    config: Option<PathBuf>,

    #[command(subcommand)]
    command: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Scaffold a new forge corpus (forge.toml + decisions/ + forces/)
    Init {
        /// Target directory (default: current directory)
        dir: Option<PathBuf>,
    },
}
```

Replace the body of `main()` (currently the manual `std::env::args()` block through `Config::load`) so it reads:

```rust
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    if let Some(Cmd::Init { dir }) = cli.command {
        let target = match dir {
            Some(d) => d,
            None => std::env::current_dir()?,
        };
        let config_path = scaffold::init(&target).map_err(|e| anyhow::anyhow!(e))?;
        println!("Scaffolded forge corpus: {}", config_path.display());
        return Ok(());
    }

    let explicit = cli.config.or(cli.positional_config);
    let env_value = std::env::var_os("FORGE_CONFIG").map(PathBuf::from);
    let cwd = std::env::current_dir()?;
    let config_path = discover::resolve_config(explicit, env_value, &cwd)
        .map_err(|e| anyhow::anyhow!(e))?;

    let cfg = Config::load(&config_path)?;
    // ... everything from `let embedder = ...` down stays exactly as-is
```

- [ ] **Step 2: Run the full test suite**

Run: `cargo test -p forge-mcp`
Expected: discover + scaffold unit tests PASS, acceptance test PASS (proves the positional form still works end to end).

- [ ] **Step 3: Smoke-test the CLI surface**

```bash
cargo run -p forge-mcp -- --help                 # shows init subcommand, --config, positional CONFIG
cargo run -p forge-mcp -- init "$TMPDIR/forge-smoke"   # prints "Scaffolded forge corpus: ..."
cargo run -p forge-mcp -- init "$TMPDIR/forge-smoke"   # errors: already exists; refusing to overwrite
cargo run -p forge-mcp -- a.toml --config b.toml # clap error: cannot be used with
```

Also confirm the cwd-default branch of `init` — run the built binary FROM a
scratch directory, never from the repo (a bare `cargo run -- init` at the repo
root would scaffold forge.toml into the repo itself):

```bash
cargo build -p forge-mcp
mkdir "$TMPDIR/forge-smoke-cwd" && cd "$TMPDIR/forge-smoke-cwd"
<repo>/target/debug/forge-mcp init            # scaffolds into the current directory
cd - # return to the repo
```

Expected: as annotated. (On Windows PowerShell use `$env:TEMP` instead of `$TMPDIR` and `target\debug\forge-mcp.exe`.)

- [ ] **Step 4: Commit**

```bash
git add crates/forge-mcp/src/main.rs
git commit -m "feat: clap CLI with config discovery and init subcommand"
```

---

### Task 5: README setup documentation

**Files:**
- Modify: `README.md` (replace the `### MCP Client Config` subsection, `README.md:46-57`)

- [ ] **Step 1: Replace the MCP Client Config subsection**

Replace lines 46–57 of `README.md` (the `### MCP Client Config` block) with:

````markdown
### Setup for agents

Install the server once, globally:

```bash
cargo install --path crates/forge-mcp
```

Bootstrap a corpus in any project:

```bash
forge-mcp init          # creates forge.toml, decisions/, forces/
```

**Claude Code** — one command, or a project-scoped `.mcp.json`:

```bash
claude mcp add forge -- forge-mcp
```

```json
{
  "mcpServers": {
    "forge": { "command": "forge-mcp" }
  }
}
```

**Other MCP clients** (Cursor, VS Code, Claude Desktop, …) use the same
`mcpServers` shape. If the client launches servers with an unpredictable
working directory, pin the config explicitly:

```json
{
  "mcpServers": {
    "forge": {
      "command": "forge-mcp",
      "env": { "FORGE_CONFIG": "C:/path/to/project/forge.toml" }
    }
  }
}
```

**Config resolution order:** `--config <path>` (or positional path) →
`FORGE_CONFIG` env var → walk up from the working directory until a
`forge.toml` is found, stopping at the repository boundary (`.git`).

> **Why not MCP roots?** The protocol's `roots` capability was deprecated by
> [SEP-2577](https://github.com/modelcontextprotocol/modelcontextprotocol/pull/2577)
> with no replacement; args, env vars, and cwd inference are the recommended
> configuration channels.
````

- [ ] **Step 2: Verify the README renders sanely**

Run: `git diff README.md`
Expected: only the client-config subsection replaced; Quick Start, forge.toml Reference, MCP Tools list intact.

- [ ] **Step 3: Commit**

```bash
git add README.md
git commit -m "docs: agent setup instructions for forge-mcp"
```

---

### Task 6: Final verification

- [ ] **Step 1: Full workspace test**

Run: `cargo test --workspace`
Expected: all tests PASS.

- [ ] **Step 2: Real install + end-to-end sanity (optional but recommended)**

```bash
cargo install --path crates/forge-mcp
cd <some scratch dir> && forge-mcp init
claude mcp add forge -- forge-mcp   # then verify the forge tools appear in a Claude Code session there
```

Expected: `forge-mcp` on PATH; a fresh corpus scaffolds; Claude Code lists the seven forge tools.
