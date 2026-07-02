# forge-mcp Agent-Ready Setup — Design

**Date:** 2026-07-02
**Status:** Approved by user (pre-implementation)
**Scope:** Personal setup ergonomics for running forge as an MCP server under coding agents (Claude Code and other MCP clients). No public distribution work.

## Problem

`forge-mcp` requires a positional path to `forge.toml`, so every project's MCP client
config needs a hand-edited absolute path. There is no install story beyond
`cargo build`, and bootstrapping a new corpus (config file plus record directories)
is manual. The goal: `{"command": "forge-mcp"}` works identically in every project.

## Decisions Made

- **Audience:** the project owner's own machines and repos only.
- **Discovery:** automatic config discovery with explicit overrides.
- **MCP roots capability: rejected.** SEP-2577 deprecated roots (no protocol
  replacement; migration guidance is tool parameters and server configuration).
  `rmcp` 2.0 still implements it but marks it for removal. Building lazy
  initialization on a dying capability is not worth it. The discovery ladder
  below is the standards-aligned approach.
- **Bootstrap:** an `init` subcommand scaffolds a new corpus.

## CLI Surface

Add `clap` (derive feature) to `forge-mcp`.

| Invocation | Behavior |
|---|---|
| `forge-mcp` | Serve MCP over stdio; discover config via the ladder below |
| `forge-mcp <path>` | Existing positional form, still accepted (backward compatible) |
| `forge-mcp --config <path>` | Serve with explicit config path |
| `forge-mcp init [dir]` | Scaffold a new corpus in `dir` (default: cwd) |

Edge cases decided:

- Supplying both a positional path and `--config` is an error (clap
  `conflicts_with`), not a silent precedence rule.
- A config path literally named `init` is shadowed by the subcommand (clap's
  default resolution). Accepted; use `--config init` in that pathological case.

## Config Resolution Ladder

First hit wins:

1. `--config` flag or positional path (explicit always wins)
2. `FORGE_CONFIG` environment variable
3. Walk up from the current working directory looking for `forge.toml`.
   After checking a directory that contains `.git`, stop — discovery never
   escapes the repository boundary. Without a `.git` marker, the walk continues
   to the filesystem root.
4. Fail with an error that names the directory searched from and suggests
   running `forge-mcp init`.

## `init` Behavior

`forge-mcp init [dir]`:

- Writes `forge.toml` with the standard defaults:
  - `roots = ["decisions", "forces"]`
  - `[dedup]` with `reuse = 0.90`, `warn = 0.75`
  - `[embedding]` with `model = "intfloat/multilingual-e5-small"`, plus a
    comment noting the `fake-bucket` test option and the ~120MB first-run
    model download.
- Creates empty `decisions/` and `forces/` directories. This is load-bearing,
  not cosmetic: `Config::load` canonicalizes root paths and fails when the
  directories are missing.
- Creates `dir` itself (and intermediate directories) when it doesn't exist.
- Refuses to overwrite an existing `forge.toml` (error, non-zero exit).
  Existing record directories are left untouched.

## Documentation (README)

Add a Setup section:

- Install: `cargo install --path crates/forge-mcp`
- Claude Code: `claude mcp add forge -- forge-mcp` and a project-scoped
  `.mcp.json` example
- Generic `mcpServers` JSON snippet for other clients (Cursor, VS Code, etc.)
- A short note on why the MCP roots capability is not used (SEP-2577
  deprecation), for future readers who ask the same question.

## Error Handling

- Missing config after the full ladder: actionable message (searched-from
  directory, override options, `forge-mcp init` suggestion), non-zero exit.
- `init` onto an existing `forge.toml`: refuse with error, non-zero exit.
- Invalid config path via flag/env: surface the load error as today.

## Testing

Unit tests in `forge-mcp`:

- Resolution ladder: flag beats env; env beats walk; walk finds `forge.toml`
  in an ancestor; walk stops at a `.git` boundary; total miss produces the
  actionable error.
- `init`: scaffolds config and directories; refuses when `forge.toml` exists;
  creates a missing target directory; an `init`-scaffolded corpus passes
  `Config::load` (proves the created directories satisfy root canonicalization).

Existing tests (including the spec §10 acceptance test) are untouched because
the positional argument form survives.

## Out of Scope

- Publishing to crates.io, prebuilt binaries, npx wrapper (deferred until the
  audience widens).
- MCP roots support (deprecated; revisit only if the spec grows a successor).
- Merging `forge-inspect` into a unified `forge` CLI (possible later; the
  discovery and init logic would carry over).
