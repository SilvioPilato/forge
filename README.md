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

Seven tools exposed over stdio:

**Read tools** (always available):
- `search` — Semantic search over frontier decisions and forces
- `get` — Get record by ID with neighborhood and verdict
- `why` — Explain why a decision's premises are stale
- `stale_report` — List all stale frontier decisions

**Write tools** (require user assent):
- `propose_decision` — Preview a new decision (pure, no writes)
- `commit` — Write a proposed decision to disk (call only after user assent)
- `set_status` — Change force or decision status, returns propagation impact

### MCP Client Config

```json
{
  "mcpServers": {
    "forge": {
      "command": "forge-mcp",
      "args": ["path/to/forge.toml"]
    }
  }
}
```

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
