# forge-mcp Empty/No-Corpus Mode — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Make the forge MCP server always connect (even with no `forge.toml`), scaffolding a corpus on demand via a new `init` tool.

**Architecture:** `discover::resolve_config` returns `Option<PathBuf>` (`None` = no corpus, a valid state). `ForgeServer.engine` becomes `Mutex<Option<Engine>>`; the 7 existing tools return empty results + an actionable hint when `None`. A new 8th tool `init` does the `None → Some` transition via `scaffold::ensure_corpus` + `Engine::new`, hot-swapping under the write lock.

**Tech Stack:** Rust, rmcp (MCP SDK), tokio, clap, tracing. Tests: `cargo test -p forge-mcp` (unit) + `--test acceptance` (integration over stdio).

**Design doc:** `docs/plans/2026-07-02-forge-mcp-empty-mode-design.md`

**Worktree:** `C:/Users/Silvio/Dev/forge/.worktrees/empty-mode` on branch `feature/empty-mode`.

---

### Task 1: `resolve_config` returns `Option<PathBuf>`

Change "not found" from an error into a valid `None` state. Update tests. Adapt the `main.rs` call site with a temporary bail (removed in Task 3) so the build stays green.

**Files:**
- Modify: `crates/forge-mcp/src/discover.rs`
- Modify: `crates/forge-mcp/src/main.rs:350-355`

**Step 1: Update the failing tests first (TDD).**

In `crates/forge-mcp/src/discover.rs`, replace `miss_produces_actionable_error` and `walk_stops_at_git_boundary`:

```rust
    #[test]
    fn miss_returns_none() {
        // .git marker keeps the walk from escaping the temp dir on a
        // developer machine that has a forge.toml somewhere above it.
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".git")).unwrap();
        assert!(resolve_config(None, None, tmp.path()).is_none());
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
        assert!(resolve_config(None, None, &nested).is_none());
    }
```

The other four tests use `.unwrap()` on `resolve_config(...)`, which still works on `Option` (panics on `None`). Leave them.

**Step 2: Run tests to verify they fail.**

Run: `cargo test -p forge-mcp discover`
Expected: FAIL — `resolve_config` still returns `Result`, so `is_none()` on a `Result` is a type error (compile failure), and the renamed test doesn't compile.

**Step 3: Change the implementation.**

Replace the signature and body of `resolve_config` in `crates/forge-mcp/src/discover.rs`:

```rust
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
        // `.git` may be a directory or a file (worktrees); either marks the
        // repo boundary. The boundary directory itself was just checked, so
        // stop here rather than escape into unrelated parents.
        if d.join(".git").exists() {
            break;
        }
        dir = d.parent();
    }
    None
}
```

**Step 4: Adapt the `main.rs` call site (temporary bail, removed in Task 3).**

In `crates/forge-mcp/src/main.rs`, replace lines 350-355:

```rust
    // conflicts_with guarantees at most one of these is Some
    let explicit = cli.config.or(cli.positional_config);
    let env_value = std::env::var_os("FORGE_CONFIG").map(PathBuf::from);
    let cwd = std::env::current_dir()?;
    let config_path = discover::resolve_config(explicit, env_value, &cwd)
        .map_err(|e| anyhow::anyhow!(e))?;
```

with:

```rust
    // conflicts_with guarantees at most one of these is Some
    let explicit = cli.config.or(cli.positional_config);
    let env_value = std::env::var_os("FORGE_CONFIG").map(PathBuf::from);
    let cwd = std::env::current_dir()?;
    let config_path = match discover::resolve_config(explicit, env_value, &cwd) {
        Some(p) => p,
        None => anyhow::bail!(
            "No forge.toml found (searched upward from {}).\n\
             Fix: pass --config <path>, set FORGE_CONFIG, or run `forge-mcp init` to scaffold a corpus.",
            cwd.display()
        ),
    };
```

**Step 5: Run tests to verify they pass.**

Run: `cargo test -p forge-mcp discover`
Expected: PASS — all 6 discover tests green.
Run: `cargo test -p forge-mcp`
Expected: PASS — 9 unit tests green (behavior unchanged; the bail preserves current crash-on-missing).

**Step 6: Commit.**

```bash
git add crates/forge-mcp/src/discover.rs crates/forge-mcp/src/main.rs
git commit -m "refactor: resolve_config returns Option<PathBuf>"
```

---

### Task 2: `scaffold::ensure_corpus` helper

A file-level helper that returns an existing `forge.toml` untouched, or scaffolds a new corpus. This is the idempotent backbone the `init` tool will call. No Engine, no network.

**Files:**
- Modify: `crates/forge-mcp/src/scaffold.rs`

**Step 1: Write the failing tests.**

In `crates/forge-mcp/src/scaffold.rs`, add to the `mod tests` block:

```rust
    #[test]
    fn ensure_corpus_creates_if_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let config_path = ensure_corpus(tmp.path()).unwrap();
        assert_eq!(config_path, tmp.path().join("forge.toml"));
        assert!(tmp.path().join("decisions").is_dir());
        assert!(tmp.path().join("forces").is_dir());
        // Load-bearing: the scaffolded config must be loadable.
        let cfg = forge_core::config::Config::load(&config_path).unwrap();
        assert_eq!(cfg.roots.len(), 2);
    }

    #[test]
    fn ensure_corpus_returns_existing_without_clobbering() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("forge.toml"), "# existing").unwrap();
        let config_path = ensure_corpus(tmp.path()).unwrap();
        assert_eq!(config_path, tmp.path().join("forge.toml"));
        // Content untouched: not overwritten with the template.
        let content = std::fs::read_to_string(&config_path).unwrap();
        assert_eq!(content, "# existing");
        // Did not create corpus subdirs (those are only for a fresh scaffold).
        assert!(!tmp.path().join("decisions").exists());
        assert!(!tmp.path().join("forces").exists());
    }
```

**Step 2: Run tests to verify they fail.**

Run: `cargo test -p forge-mcp scaffold::tests::ensure_corpus`
Expected: FAIL — `ensure_corpus` is not defined (compile error).

**Step 3: Implement `ensure_corpus`.**

In `crates/forge-mcp/src/scaffold.rs`, add after the `init` function:

```rust
/// Ensure a corpus exists in `dir`. If `dir/forge.toml` is already present,
/// return its path untouched. Otherwise scaffold a new corpus there via
/// [`init`] and return the created path. Never overwrites an existing
/// forge.toml (delegates to `init`'s race-safe `create_new`).
pub fn ensure_corpus(dir: &Path) -> Result<PathBuf, String> {
    let config_path = dir.join("forge.toml");
    if config_path.is_file() {
        return Ok(config_path);
    }
    init(dir)
}
```

**Step 4: Run tests to verify they pass.**

Run: `cargo test -p forge-mcp scaffold`
Expected: PASS — all scaffold tests green (5 total).

**Step 5: Commit.**

```bash
git add crates/forge-mcp/src/scaffold.rs
git commit -m "feat: add scaffold::ensure_corpus helper"
```

---

### Task 3: Empty-mode startup + `None`-handling for the 7 existing tools

The big one. `ForgeServer.engine` becomes `Mutex<Option<Engine>>`. `main.rs` branches: `Some` → load normally; `None` → empty mode (no embedder, instant connect). The 7 tools match on `Option` and return empty results + a hint when `None`.

**Files:**
- Modify: `crates/forge-mcp/src/main.rs` (struct, constructors, helpers, `main()`, all 7 tool bodies)
- Modify: `crates/forge-mcp/tests/acceptance.rs` (add `new_empty`, `empty_corpus_cwd`, `acceptance_spec_11_empty_mode`)

**Step 1: Write the failing acceptance test.**

In `crates/forge-mcp/tests/acceptance.rs`, add a constructor and a helper after `MCPClient::new`:

```rust
    fn new_empty(cwd: &Path) -> Self {
        let bin = env!("CARGO_BIN_EXE_forge-mcp");
        let mut child = Command::new(bin)
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        MCPClient {
            child,
            stdin,
            reader: BufReader::new(stdout),
            next_id: 1,
        }
    }
```

Add a top-level helper (near `temp_corpus_for_mcp`):

```rust
fn empty_corpus_cwd() -> PathBuf {
    let dir = std::env::temp_dir().join("forge-mcp-empty-mode-test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // .git marker stops the upward walk so discover returns None deterministically.
    std::fs::create_dir_all(dir.join(".git")).unwrap();
    dir
}
```

Add the test at the end of the file:

```rust
#[test]
fn acceptance_spec_11_empty_mode() {
    let cwd = empty_corpus_cwd();
    let mut client = MCPClient::new_empty(&cwd);

    let init_resp = client.initialize();
    assert!(
        init_resp.get("result").is_some(),
        "initialize failed in empty mode: {}",
        init_resp
    );

    // A read tool returns empty results + an actionable hint.
    let search_resp = client.call_tool(
        "search",
        serde_json::json!({"query": "anything", "scope": "both", "limit": 10}),
    );
    let search_text = search_resp["result"]["content"][0]["text"]
        .as_str()
        .unwrap();
    let search_result: Value = serde_json::from_str(search_text).unwrap();
    assert!(
        search_result["hits"].as_array().unwrap().is_empty(),
        "empty corpus should return no hits"
    );
    assert!(
        search_result.get("hint").is_some(),
        "empty-mode search should include a hint"
    );

    // A write tool is refused with an error.
    let propose_resp = client.call_tool(
        "propose_decision",
        serde_json::json!({"title": "x", "forces": []}),
    );
    let propose_text = propose_resp["result"]["content"][0]["text"]
        .as_str()
        .unwrap();
    let propose_result: Value = serde_json::from_str(propose_text).unwrap();
    assert!(
        propose_result.get("error").is_some(),
        "propose in empty mode should be refused"
    );
}
```

**Step 2: Run the test to verify it fails.**

Run: `cargo test -p forge-mcp --test acceptance acceptance_spec_11_empty_mode`
Expected: FAIL — the server still bails on missing config (Task 1's stepping stone), so `initialize` never completes / the process exits; `init_resp.get("result")` is `None`.

**Step 3: Change `ForgeServer` to hold `Mutex<Option<Engine>>` + add constructors and hint helpers.**

In `crates/forge-mcp/src/main.rs`, replace the struct and its `impl` block (lines 18-28):

```rust
struct ForgeServer {
    engine: Mutex<Option<Engine>>,
    project_dir: PathBuf,
}

impl ForgeServer {
    fn empty(project_dir: PathBuf) -> Self {
        Self {
            engine: Mutex::new(None),
            project_dir,
        }
    }

    fn loaded(engine: Engine, project_dir: PathBuf) -> Self {
        Self {
            engine: Mutex::new(Some(engine)),
            project_dir,
        }
    }

    fn no_corpus_hits() -> String {
        serde_json::to_string(&serde_json::json!({
            "hits": [],
            "hint": "No forge.toml in this project. Call the `init` tool (after user assent) to scaffold a corpus.",
        }))
        .unwrap()
    }

    fn no_corpus_not_found() -> String {
        serde_json::to_string(&serde_json::json!({
            "error": "not found",
            "hint": "No forge.toml in this project. Call the `init` tool (after user assent) to scaffold a corpus.",
        }))
        .unwrap()
    }

    fn no_corpus_stale_report() -> String {
        serde_json::to_string(&serde_json::json!({
            "stale": [],
            "diagnostics_summary": {"count": 0, "kinds": []},
            "hint": "No forge.toml in this project. Call the `init` tool (after user assent) to scaffold a corpus.",
        }))
        .unwrap()
    }

    fn no_corpus_write_refused() -> String {
        serde_json::to_string(&serde_json::json!({
            "error": "no corpus; call `init` first (after user assent)",
        }))
        .unwrap()
    }
}
```

**Step 4: Update `main()` to branch on `Option` (removes Task 1's bail).**

In `crates/forge-mcp/src/main.rs`, replace the block from `let explicit = ...` through `let server = ForgeServer::new(engine);` (the whole config-load + engine-build + server construction) with:

```rust
    // conflicts_with guarantees at most one of these is Some
    let explicit = cli.config.or(cli.positional_config);
    let env_value = std::env::var_os("FORGE_CONFIG").map(PathBuf::from);
    let cwd = std::env::current_dir()?;

    let server = match discover::resolve_config(explicit, env_value, &cwd) {
        Some(config_path) => {
            let cfg = Config::load(&config_path)
                .with_context(|| format!("failed to load config at {}", config_path.display()))?;
            init_subscriber(&cfg.log.level, &cfg.log.format, cfg.log.file.as_ref());
            let embedder = match default_embedder(&cfg) {
                Ok(e) => e,
                Err(e) => {
                    anyhow::bail!("Failed to create embedder: {}", e.0);
                }
            };
            let engine = Engine::new(cfg, embedder)
                .map_err(|e| anyhow::anyhow!("Failed to initialize engine: {}", e))?;
            let snap = engine.snapshot();
            info!(
                diagnostics = snap.diagnostics.len(),
                frontier = snap.frontier().len(),
                "forge-mcp ready"
            );
            ForgeServer::loaded(engine, cwd)
        }
        None => {
            init_subscriber("info", "compact", None);
            info!("forge-mcp starting in empty mode (no forge.toml found)");
            ForgeServer::empty(cwd)
        }
    };
```

Leave the trailing `let (stdin, stdout) = rmcp::transport::io::stdio(); ...` block unchanged.

**Step 5: Update the 7 tool bodies to match on `Option`.**

For each **read** tool (`search`, `get`, `why`, `stale_report`), change the lock acquisition to short-circuit on `None`. Pattern for `search`:

```rust
    async fn search(&self, Parameters(params): Parameters<SearchParams>) -> String {
        let engine = self.engine.lock().await;
        let engine = match engine.as_ref() {
            Some(e) => e,
            None => return Self::no_corpus_hits(),
        };
        let snap = engine.snapshot();
        // ... rest of the existing search body, unchanged
```

For `get` and `why`: `None => return Self::no_corpus_not_found(),` then the rest unchanged.
For `stale_report`: `None => return Self::no_corpus_stale_report(),` then the rest unchanged.

For each **write** tool that needs `&mut Engine` (`commit`, `set_status`), use `as_mut`:

```rust
    async fn commit(&self, Parameters(params): Parameters<CommitParams>) -> String {
        let mut engine = self.engine.lock().await;
        let engine = match engine.as_mut() {
            Some(e) => e,
            None => return Self::no_corpus_write_refused(),
        };
        // ... rest of the existing commit body, unchanged
```

For `set_status`: same `as_mut` pattern with `None => return Self::no_corpus_write_refused(),`.

For `propose_decision` (write tool, but uses `&Engine` — it calls `engine.propose_decision(input)` with an immutable lock in the current code), use `as_ref`:

```rust
    async fn propose_decision(&self, Parameters(params): Parameters<ProposeParams>) -> String {
        let engine = self.engine.lock().await;
        let engine = match engine.as_ref() {
            Some(e) => e,
            None => return Self::no_corpus_write_refused(),
        };
        // ... rest of the existing propose_decision body, unchanged
```

> Note: `engine.as_ref()` / `engine.as_mut()` work because `tokio::sync::MutexGuard` derefs to the inner `Option<Engine>`. The rest of each body references `engine` (now the `&Engine` / `&mut Engine` from the match arm), which is a drop-in replacement — no other edits needed inside the bodies.

**Step 6: Run the acceptance test to verify it passes.**

Run: `cargo test -p forge-mcp --test acceptance acceptance_spec_11_empty_mode`
Expected: PASS — server starts in empty mode; search returns empty+hint; propose is refused.

**Step 7: Run the full suite to verify no regression.**

Run: `cargo test -p forge-mcp`
Expected: PASS — 9 unit tests + both acceptance tests (`acceptance_spec_10_e2e`, `acceptance_spec_11_empty_mode`) green.

**Step 8: Commit.**

```bash
git add crates/forge-mcp/src/main.rs crates/forge-mcp/tests/acceptance.rs
git commit -m "feat: forge-mcp starts in empty mode when no forge.toml found"
```

---

### Task 4: Add the `init` MCP tool (`None → Some` hot-reload)

The 8th tool. Scaffolds (or reuses) a corpus in `project_dir`, builds an `Engine`, swaps it into the `Mutex<Option<Engine>>` under the write lock. Network-free in the acceptance test via a pre-placed `fake-bucket` corpus (exercises `ensure_corpus`'s return-existing branch).

**Files:**
- Modify: `crates/forge-mcp/src/main.rs` (add `init` tool inside the `#[tool_router] impl ForgeServer` block)
- Modify: `crates/forge-mcp/tests/acceptance.rs` (add `acceptance_spec_12_init_loads_corpus`)

**Step 1: Write the failing acceptance test.**

In `crates/forge-mcp/tests/acceptance.rs`, add:

```rust
#[test]
fn acceptance_spec_12_init_loads_corpus() {
    let cwd = empty_corpus_cwd();
    // Pre-place a fake-bucket corpus so init's ensure_corpus takes the
    // return-existing branch and Engine::new stays network-free.
    let src = PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/corpus"
    ));
    copy_dir(&src, &cwd).unwrap();

    let mut client = MCPClient::new_empty(&cwd);
    client.initialize();

    // tools/list must include the init tool.
    let list_resp = client.send("tools/list", serde_json::json!({}));
    let tools = list_resp["result"]["tools"].as_array().unwrap();
    let names: Vec<&str> = tools
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert!(
        names.contains(&"init"),
        "tools/list must include init: {:?}",
        names
    );

    // init loads the pre-placed corpus.
    let init_resp = client.call_tool("init", serde_json::json!({}));
    let init_text = init_resp["result"]["content"][0]["text"]
        .as_str()
        .unwrap();
    let init_result: Value = serde_json::from_str(init_text).unwrap();
    assert_eq!(init_result["status"], "loaded");

    // post-init search returns hits WITHOUT the empty-mode hint.
    let search_resp = client.call_tool(
        "search",
        serde_json::json!({"query": "rust", "scope": "both", "limit": 5}),
    );
    let search_text = search_resp["result"]["content"][0]["text"]
        .as_str()
        .unwrap();
    let search_result: Value = serde_json::from_str(search_text).unwrap();
    assert!(
        search_result.get("hint").is_none(),
        "loaded corpus should not show the empty-mode hint"
    );
    assert!(
        !search_result["hits"].as_array().unwrap().is_empty(),
        "fixtures corpus should return hits after init"
    );

    // a second init is a no-op.
    let init2_resp = client.call_tool("init", serde_json::json!({}));
    let init2_text = init2_resp["result"]["content"][0]["text"]
        .as_str()
        .unwrap();
    let init2_result: Value = serde_json::from_str(init2_text).unwrap();
    assert_eq!(init2_result["status"], "already loaded");
}
```

**Step 2: Run the test to verify it fails.**

Run: `cargo test -p forge-mcp --test acceptance acceptance_spec_12_init_loads_corpus`
Expected: FAIL — `init` tool does not exist; `tools/list` lacks `init`, so the first assertion fails.

**Step 3: Implement the `init` tool.**

In `crates/forge-mcp/src/main.rs`, inside the `#[tool_router] impl ForgeServer { ... }` block (add it alongside the other tools, e.g. after `set_status`):

```rust
    #[tool(
        description = "Scaffold a forge corpus (forge.toml + decisions/ + forces/) in this project's root and load it. Call only after the user has assented. Refuses to overwrite an existing forge.toml."
    )]
    async fn init(&self) -> String {
        let mut engine = self.engine.lock().await;
        if engine.is_some() {
            return serde_json::to_string(&serde_json::json!({
                "status": "already loaded",
            }))
            .unwrap();
        }

        let config_path = match scaffold::ensure_corpus(&self.project_dir) {
            Ok(p) => p,
            Err(e) => {
                return serde_json::to_string(&serde_json::json!({
                    "error": format!("failed to scaffold corpus: {e}"),
                }))
                .unwrap();
            }
        };

        let cfg = match Config::load(&config_path) {
            Ok(c) => c,
            Err(e) => {
                return serde_json::to_string(&serde_json::json!({
                    "error": format!("failed to load config: {e}"),
                }))
                .unwrap();
            }
        };

        let embedder = match default_embedder(&cfg) {
            Ok(e) => e,
            Err(e) => {
                return serde_json::to_string(&serde_json::json!({
                    "error": format!("failed to create embedder: {}", e.0),
                }))
                .unwrap();
            }
        };

        let new_engine = match Engine::new(cfg, embedder) {
            Ok(e) => e,
            Err(e) => {
                return serde_json::to_string(&serde_json::json!({
                    "error": format!("failed to initialize engine: {}", e),
                }))
                .unwrap();
            }
        };

        let snap = new_engine.snapshot();
        info!(
            diagnostics = snap.diagnostics.len(),
            frontier = snap.frontier().len(),
            "forge-mcp corpus loaded via init tool"
        );
        *engine = Some(new_engine);
        serde_json::to_string_pretty(&serde_json::json!({
            "status": "loaded",
            "config": config_path.display().to_string(),
        }))
        .unwrap()
    }
```

> Error invariant: on any failure in `ensure_corpus` / `Config::load` / `default_embedder` / `Engine::new`, the function returns early with an `error` JSON and `engine` stays `None`. The write lock is held throughout, so no concurrent tool ever sees a half-loaded engine. `Config`, `default_embedder`, `Engine`, and `scaffold` are already in scope (`scaffold` is a sibling module via `mod scaffold;`).

**Step 4: Run the test to verify it passes.**

Run: `cargo test -p forge-mcp --test acceptance acceptance_spec_12_init_loads_corpus`
Expected: PASS — `init` listed; loads the pre-placed corpus; search returns hits without hint; second `init` returns `already loaded`.

**Step 5: Run the full suite.**

Run: `cargo test -p forge-mcp`
Expected: PASS — all unit tests + 3 acceptance tests green.

**Step 6: Commit.**

```bash
git add crates/forge-mcp/src/main.rs crates/forge-mcp/tests/acceptance.rs
git commit -m "feat: add init MCP tool to scaffold and load a corpus"
```

---

### Task 5: Document empty mode and the `init` tool in the README

**Files:**
- Modify: `README.md`

**Step 1: Update the tool count and list.**

In `README.md`, change "Seven tools exposed over stdio:" to "Eight tools exposed over stdio:". After the "Write tools" list, add a new subsection:

```markdown
**Setup tool:**
- `init` — Scaffold a forge corpus (forge.toml + decisions/ + forces/) in the project root and load it into the running server. Call only after the user has assented. Refuses to overwrite an existing forge.toml.
```

**Step 2: Document empty mode.**

After the "Setup for agents" subsection's opencode block, add:

```markdown
### Empty mode

With no `forge.toml` in the project (and none pinned via `--config` or `FORGE_CONFIG`), the server starts in **empty mode**: it connects immediately — no embedder is constructed, so there is no model download and no cold-cache timeout. The read tools return empty results with a hint pointing to `init`; the write tools refuse until a corpus is loaded. Call the `init` tool (after user assent) to scaffold a corpus in the project root and hot-load it without restarting the server.
```

**Step 3: Update the first-run timeout note.**

Replace the paragraph beginning "On a cold model cache the first launch can exceed opencode's 5s tool-fetch timeout…" with:

```markdown
In empty mode the server connects instantly (no embedder). When a corpus is loaded — at startup or via the `init` tool — a cold model cache can exceed opencode's 5s tool-fetch timeout; warm the cache by running `forge-mcp` once in the project, or add `"timeout": 120000` to the entry. (The `init` tool itself is a tool *call*, not a tool *fetch*, so it is not subject to the 5s limit — the first-run download happens there.)
```

**Step 4: Verify the build still passes (sanity — no code change).**

Run: `cargo test -p forge-mcp`
Expected: PASS — unchanged.

**Step 5: Commit.**

```bash
git add README.md
git commit -m "docs: document empty mode and the init tool"
```

---

## Verification (final)

After all 5 tasks:

```bash
cargo test --workspace
```

Expected: all tests pass. New tests: `acceptance_spec_11_empty_mode`, `acceptance_spec_12_init_loads_corpus`. Updated unit tests: `miss_returns_none`, `walk_stops_at_git_boundary`, `ensure_corpus_creates_if_missing`, `ensure_corpus_returns_existing_without_clobbering`. No regressions in `acceptance_spec_10_e2e` or the 64 forge-core tests.

Then verify the original bug is fixed end-to-end: from the forge source repo (no `forge.toml` at root), the opencode `forge` entry with `["forge-mcp"]` should now **connect** (empty mode) instead of crashing.
