# Forge

A local-first Rust engine for justified, perishable decisions. Markdown files serve as the sole source of truth, with an in-memory graph that propagates premise staleness and detects near-duplicate forces.

## Quick Start

```bash
# Build
cargo build --release

# Inspect a corpus
cargo run -p forge-inspect -- path/to/forge.toml

# Run MCP server (for AI agent integration)
cargo run -p forge-mcp -- path/to/forge.toml
```

## forge.toml Reference

```toml
roots = ["decisions", "forces"]   # directories relative to this file

[dedup]
reuse = 0.90  # cosine threshold: silently reuse existing force
warn = 0.75   # cosine threshold: warn about near-duplicate

[embedding]
model = "fake-bucket"              # "fake-bucket" for tests, or "intfloat/multilingual-e5-small" for real embeddings
```

## MCP Tools

Ten tools exposed over stdio:

**Read tools** (always available):
- `search` — Semantic search over frontier decisions and forces (indexes each record's title, body, and tags)
- `get` — Get record by ID with neighborhood and verdict
- `why` — Explain why a decision's premises are stale
- `stale_report` — List all stale frontier decisions

**Write tools** (require user assent):
- `propose_decision` — Preview a new decision (pure, no writes)
- `commit` — Write a proposed decision to disk with date frontmatter auto-populated (call only after user assent)
- `create_force` — Record a standalone force independent of any decision, so it can be captured before a decision cites it; reuses a living near-duplicate unless `force_new` is set
- `set_status` — Change force or decision status, returns propagation impact
- `reindex` — Re-scan the corpus from disk and rebuild the index (use after manual file edits)

**Setup tool:**
- `init` — Scaffold a forge corpus (forge.toml + decisions/ + forces/) in the project root and load it into the running server. Call only after the user has assented. Refuses to overwrite an existing forge.toml.

### Setup for agents

Install the server once, globally:

```bash
cargo install --path crates/forge-mcp
```

Bootstrap a corpus in any project:

```bash
forge-mcp init          # creates forge.toml, decisions/, forces/ and pre-downloads the embedding model into the cache (skipped when cached)
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

**opencode** uses its own config shape — `opencode.json` at the project root
(or `~/.config/opencode/opencode.json` globally), with `command` as an array:

```json
{
  "$schema": "https://opencode.ai/config.json",
  "mcp": {
    "forge": {
      "type": "local",
      "command": ["forge-mcp"],
      "enabled": true
    }
  }
}
```

### Empty mode

With no `forge.toml` in the project (and none pinned via `--config` or `FORGE_CONFIG`), the server starts in **empty mode**: no model is loaded, read tools return empty results with `no_corpus_*` hints pointing to `init`, and write tools refuse with hints to scaffold a corpus first. Call the `init` tool (after user assent) to scaffold a corpus in the project root and hot-load it without restarting the server.

When a `forge.toml` **is** discovered, the server answers the MCP handshake immediately and loads the corpus in the background. While loading, tools return `{"status": "loading"}`. A `notifications/message` logging notification announces both readiness and loading failures.

**Config resolution order:** `--config <path>` (or positional path) →
`FORGE_CONFIG` env var → walk up from the working directory until a
`forge.toml` is found, stopping at the repository boundary (`.git`).

> **Why not MCP roots?** The protocol's `roots` capability was deprecated by
> [SEP-2577](https://github.com/modelcontextprotocol/modelcontextprotocol/pull/2577)
> with no replacement; args, env vars, and cwd inference are the recommended
> configuration channels.

## First-Run Model Download

When using `model = "intfloat/multilingual-e5-small"`, the first run downloads ~120MB from Hugging Face. Subsequent runs use the local cache.

## v0 Boundaries

Deliberately absent in v0:
- No incremental indexing (full rebuild on every write)
- No multi-user or network access
- No backwards-compatible record format migration
- No configurable embedding dimensions beyond what the model provides

## Development

```bash
cargo test --workspace                    # all tests
cargo test -p forge-core --features onnx -- --ignored  # real embedding test (network required)
```
