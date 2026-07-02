# forge-mcp: empty/no-corpus mode

**Date:** 2026-07-02
**Status:** Design (validated)

## Problem

When opencode launches the `forge` MCP server, it uses the bare global config:

```json
"forge": { "type": "local", "command": ["forge-mcp"], "enabled": true }
```

No `--config` arg, no `FORGE_CONFIG` env var, so the server relies on cwd
discovery. opencode launches it with cwd = the forge source repo (or any
project without a `forge.toml`). Per `discover.rs`, the upward walk **stops at
the `.git` boundary**, and there is no `forge.toml` at the project root, so the
server exits immediately:

```
Error: No forge.toml found (searched upward from C:\Users\Silvio\Dev\forge).
```

The process dies before the MCP handshake, so the client reports "not
connecting". Not a code bug — a config/discovery gap. The README itself notes
that clients with unpredictable cwds should pin the config, but that is a
per-project manual step; end users expect the server to "just connect".

## Goal

The MCP server always connects, with zero setup. A corpus is opt-in per
project, scaffolded on demand from within a session.

## Design

### 1. Startup & state model

`discover::resolve_config` changes return type from `Result<PathBuf, String>`
to `Option<PathBuf>`: `None` when no `forge.toml` exists within the `.git`
boundary. "Not found" is a valid state, not an error.

`main.rs` branches on the result:

- `Some(path)` → current path: load `Config`, `init_subscriber` from
  `cfg.log`, build `Engine`, serve.
- `None` → **empty mode**: `init_subscriber("info", "compact", None)` with
  hardcoded defaults (there is no `forge.toml` to read `[log]` from), build a
  `ForgeServer` with **no Engine**, serve. No embedder is constructed, so no
  model download and no 5s cold-cache timeout — the server connects
  instantly.

`ForgeServer.engine` becomes `Mutex<Option<Engine>>`:

- `None` = empty mode (no corpus).
- `Some(Engine)` = loaded.

Every tool acquires the lock and matches on the `Option`. The MCP handshake
(`initialize`, `tools/list`, `tools/call`) is independent of `Engine`, so the
client connects and lists all tools regardless of corpus state — this is what
makes "not connecting" impossible.

A new 8th tool, `init`, is the only tool that mutates the `Option`
(`None → Some`). It is gated on user assent (see section 3).

### 2. Tool semantics in empty mode

When `engine` is `None`, the 7 existing tools short-circuit with empty results
plus an agent-actionable hint (the hint wording matters — the agent reads it
and knows the next step).

**Read tools** return their normal *empty* shape + `hint`:

- `search` → `{"hits": [], "hint": "No forge.toml in this project. Call the \`init\` tool (after user assent) to scaffold a corpus."}`
- `get` / `why` → `{"error": "not found", "hint": "..."}`
- `stale_report` → `{"stale": [], "diagnostics_summary": {"count": 0, "kinds": []}, "hint": "..."}`

**Write tools** refuse with `hint` (no corpus to write into):

- `propose_decision` / `commit` / `set_status` → `{"error": "no corpus; call \`init\` first (after user assent)"}`

**New 8th tool `init`**:

- *description*: "Scaffold a forge corpus (forge.toml + decisions/ + forces/)
  in this project's root and load it. Call only after the user has assented.
  Refuses to overwrite an existing forge.toml."
- *params*: none. Scaffolds at the server's launch cwd — `main.rs` already
  computes `cwd` and passes it into `ForgeServer` so the tool knows the target
  dir. (opencode launches with cwd = project root, i.e. the `.git` dir, so
  this lands correctly.)
- Reuses `scaffold::init` verbatim (same race-safe `create_new`, same
  refusal-to-clobber), via the `ensure_corpus` helper below.

This keeps the read tools genuinely useful for probing ("is there a corpus?
no → call init") while never silently writing. The `init` tool is the single
`None → Some` transition; the existing "user assent" convention (already used
by `propose` → `commit`) covers the consent gate.

### 3. Hot-reload mechanics & error handling

The `init` tool is the only `None → Some` transition. Under the `engine`
**write** lock it does, in order:

1. **Ensure the corpus** — a new `scaffold::ensure_corpus(dir)` helper: if
   `dir/forge.toml` exists, return its path untouched; otherwise call the
   existing `scaffold::init` (race-safe `create_new`, refuses to clobber).
   This gracefully handles the race where the user ran CLI `forge-mcp init`
   in a terminal while the server sat empty — the tool just loads the
   existing file instead of erroring.
2. **`Config::load(path)`** — the scaffolded template is known-valid, but
   load still canonicalizes roots.
3. **`default_embedder(&cfg)`** + **`Engine::new(cfg, embedder)`** — same as
   `main.rs` does today.
4. **Swap** `*engine = Some(new_engine)`, drop the lock, return
   `{"status": "loaded", "config": path}`.

**Error invariant:** on *any* failure in steps 1–3, the lock is released with
`engine` still `None`. No partial state. The Mutex is held across the whole op
so no concurrent tool ever sees a half-loaded engine.

**First-run download trade-off:** the scaffold template uses
`model = "intfloat/multilingual-e5-small"` (~120MB on first real use). So the
first `init` call in a clean environment blocks on the Hugging Face download —
identical to today's first-run behavior, just triggered via tool call instead
of startup. opencode's 5s limit is on tool-*fetch*, not tool-*calls*, so this
will not time out. (Future nicety: make embedder creation lazy so `init`
returns instantly and the download defers to the first `search`/`propose`.
Out of scope here — YAGNI.)

If `init` is called when `engine` is already `Some`, it returns
`{"status": "already loaded"}` as a no-op — no surprising rebuilds.

### 4. Edge cases & testing

**Edge cases handled:**

- **Backward compat:** `--config` / `FORGE_CONFIG` / a `forge.toml` at the
  project root all bypass empty mode and load at startup exactly as today.
  Only the no-config case changes — from *crash* to *empty mode*. Pure
  improvement, no regression for existing setups.
- **Launched from a subdir of a project that has `forge.toml` at root:**
  discover still finds it before the `.git` boundary (existing
  `config_in_repo_root_is_found_even_with_git_marker` test) — loads normally,
  not empty.
- **Concurrent `init` calls:** the write lock serializes them; the second
  sees `Some` and returns `{"status": "already loaded"}`.
- **Unwritable cwd / permission denied:** `ensure_corpus` surfaces the error;
  engine stays `None`.
- **`init` when already `Some`:** no-op `{"status": "already loaded"}` — no
  surprise rebuild.

**Testing:**

- **`discover.rs`:** `miss_produces_actionable_error` becomes
  `miss_returns_none` (walk-stops-at-boundary tests now assert `None`).
  Boundary semantics unchanged.
- **`scaffold.rs`:** add `ensure_corpus_creates_if_missing` and
  `ensure_corpus_returns_existing_without_clobbering` (file-level only — no
  Engine, no network).
- **`acceptance.rs` (integration):** spawn the server in a temp dir with no
  `forge.toml` (and a `.git` marker). Assert: `tools/list` returns 8 tools
  incl. `init`; `search` returns empty+hint; `init` then `search` returns
  empty *without* hint; second `init` returns `already loaded`. To stay
  network-free, the load path is exercised via a **pre-placed `fake-bucket`
  `forge.toml`** (tests `ensure_corpus`'s return-existing branch); the
  create-if-missing file behavior is covered by the `scaffold.rs` unit tests.
  This cleanly avoids the 120MB download in CI without adding an
  embedder-override env var (YAGNI).
- **README:** document empty mode, the `init` tool (now 8 tools), and note
  the global `["forge-mcp"]` opencode entry now "just connects."

## Out of scope

- Lazy embedder creation (deferred download to first `search`/`propose`).
- A separate `reload` tool for picking up manual edits to an existing corpus
  without restarting the server.
- File-watching for `forge.toml` appearance.
- Auto-scaffolding without explicit `init` (silent file creation rejected in
  favor of user-assented `init`).
