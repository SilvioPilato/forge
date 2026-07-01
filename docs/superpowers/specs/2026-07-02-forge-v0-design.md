# Forge v0 — Design (resolves the open decisions of `forge-v0-build-spec.md`)

**Status:** approved by user, pending spec review
**Parent spec:** `../../../forge-v0-build-spec.md` (the build specification; this document resolves
its §9 open decisions and concretizes the architecture for implementation)
**Date:** 2026-07-02

---

## 1. Resolved decisions (spec §9)

| # | Open decision | Resolution |
|---|---------------|------------|
| 1 | Process model | **Session-lived server.** The MCP server process (spawned by the client over stdio) builds the snapshot and loads the embedding model once at startup, keeps both warm for the session, rebuilds on commit. No separate daemon. |
| 2 | Reindex coherence | **Synchronous.** `commit` and `set_status` return only after the snapshot reflects the write. An agent always reads its own writes. |
| 3 | Derived-index materialization | **In-memory snapshot + on-disk vector cache.** The snapshot is rebuilt from files at startup (pure). Embedding vectors — the only expensive derivation — are cached on disk keyed by `(model_id, content_hash)`. The cache is disposable and never written into record files. |
| 4 | Embedding model | **`intfloat/multilingual-e5-small`** (384-dim, ~120 MB) via ONNX Runtime. Chosen for multilingual quality (records may be Italian, English, or mixed). Weights fetched once into `~/.cache/forge/models/`, offline thereafter. The E5 `query:`/`passage:` prefix convention lives inside the provider. |
| 5 | Severity model | **Status, then distance.** `retired` outranks `changed`; within a status band, smaller distance from the fallen force ranks higher. Ties broken by recency of the fall. Uses exactly the metadata the judge already carries. |
| 6 | De-dup threshold | **Two-band + explicit flag.** Cosine ≥ 0.90: `commit` reuses the existing force id unless the proposed force carries `force_new: true`. 0.75–0.90: commit succeeds, near-matches returned as warnings. Below 0.75: silent accept. Thresholds live in config. |
| 7 | Language/stack | **Rust.** Single binary, in-process embedding via the `ort` crate, official MCP SDK (`rmcp`). |

## 2. Shape of the codebase

A single Cargo workspace, two crates:

- **`forge-core`** (library) — spec components A–H plus J behind a trait. Pure logic; the only
  I/O is file read/write (discovery, guardian) and the embedding provider's model loading.
  No MCP awareness.
- **`forge-mcp`** (binary) — component I. Thin: parses config, builds the snapshot, loads the
  embedding provider, serves the six tools over stdio via `rmcp`.

A third dev-only binary, **`forge-inspect`**, exists for build feedback (§7); it is scaffolding,
not product surface (the spec's deferred CLI is a *user* surface; this is not that).

Borrowed capabilities (spec §7 reuse boundary): `rmcp` (MCP protocol), `ort` + `tokenizers` +
`hf-hub` (embedding runtime and model download), `serde` + a YAML crate (frontmatter). The graph
itself is hand-rolled (HashMap-based arena and adjacency) — graph, temporality, and supersession
are the core we build, per the spec.

## 3. Configuration

A `forge.toml` whose path is passed as an argument to the `forge-mcp` (and `forge-inspect`)
binary, so each MCP client entry points at one workspace. Keys:

- `roots = [...]` — list of directories (required; defines the record universe)
- `dedup.reuse = 0.90`, `dedup.warn = 0.75`
- `embedding.model = "intfloat/multilingual-e5-small"`
- `cache_dir` — override for the model/vector cache location
- `write_dir` — where the guardian places new records (default: `decisions/` and `forces/`
  under the first root), named `<id>.md`

Everything except `roots` has a default.

## 4. Component mapping

- **A. Record layer** — `parse(path, text) -> Decision | Force | ParseError`. Frontmatter split +
  YAML into serde structs; body carried as an opaque `String`. Serialization is the exact
  inverse; round-trip stability (`parse ∘ serialize = identity`) is a tested guarantee.
  `status_log` validated: non-empty, ordered, legal transitions (`holds→changed→retired`,
  no resurrection).
- **B. Discovery** — walk the roots; yield `.md` files whose frontmatter declares
  `type: decision | force`. Other Markdown files are ignored silently.
- **C. Linker** — resolve id-strings against the id map. Diagnostics as data:
  `IdCollision`, `DanglingRef {from, field, to}`, `DependsOnCycle {members}`. Dangling edges
  become first-class `Unresolved(id)` endpoints, never failures. Per amendment A4, anchor
  contents are never treated as id references.
- **D. Graph model** — arena of records + `id → index` map; reverse index
  `force → citing decisions`; `dependsOn` adjacency and its reverse; supersession partial
  order. Pure, no I/O.
- **E. Judge (premise axis)** — a force is *fallen* if the last `status_log` entry is
  `changed` or `retired`. Premise-staleness: reverse transitive closure (BFS) from each fallen
  force over reverse(`cites` ∪ `dependsOn`), carrying distance. Verdict per decision:
  `Fresh` or `PremiseStale { fallen: [(force_id, status, distance)] }`, plus a `Superseded`
  flag (named in another's `supersedes`, or `deprecated`). Frontier = maximal non-superseded.
  Anchors ignored (amendment A3). The closure is monotone and order-independent.
- **F. Snapshot** — `build(roots, config, embedder) -> Snapshot`: discovery → parse → link →
  graph → judge → embed. An immutable value holding the graph, verdicts, all diagnostics
  (parse errors, collisions, dangling, cycles), and vectors. Partial graphs are valid: broken
  records become diagnostics, good records still answer.
- **J. Embedding provider** — `trait Embedder { fn embed(&self, texts: &[&str]) -> Result<Vec<Vector>> }`.
  Default impl: multilingual-e5-small via `ort`. A deterministic fake impl exists for tests.
  Vectors from different providers are not comparable; a provider/model change invalidates the
  vector cache (keyed by model id) and triggers full re-embedding on next build.
- **G. Recall** — brute-force cosine over the snapshot's vectors (no vector DB).
  `search(query, scope) -> ranked hits over the frontier`; `near_matches(text) -> hits above
  the warn threshold`. One engine, two entry points.
- **H. Guardian** — the only file writer. `propose_decision` is pure: composes records,
  validates, attaches near-matches; safe to call repeatedly. `commit`: re-validate against the
  current snapshot → de-dup gate (per §1.6) → write new files append-only → synchronous
  rebuild → return the new snapshot's view of what was written. `set_status` appends one
  transition to `status_log` (the only in-place file change). `supersede` writes a new
  Decision carrying `supersedes`. No raw write / edit / delete exists.
- **I. MCP server** — the six spec-§5 tools mapped 1:1 onto F, G, H. Holds `Arc<Snapshot>`
  swapped atomically on rebuild; synchronous commit = respond only after the swap.

## 5. Concurrency and state

Single-threaded semantics: tool calls are serialized (writes take a lock); the snapshot is an
immutable `Arc` swapped on rebuild. No file watcher in v0 — external edits are picked up on the
next rebuild (session restart or any commit).

## 6. Error handling

Two regimes:

- **Corpus problems are data** — parse errors, collisions, dangling refs, cycles become
  diagnostics in the snapshot and are surfaced through tool responses. The judge and search
  operate on whatever subset is well-formed.
- **System problems are errors** — unreadable config, model download failure, file write
  failure surface as tool-level errors.

## 7. Testing and build-feedback strategy

Every task in the implementation plan ends with a runnable, self-checkable gate — no task is
"done" on code alone.

1. **Fixture corpus in Phase 0, before any component code.** ~15 hand-written records covering:
   happy path, reticolo chains, a `dependsOn` cycle, an id collision, a dangling ref, malformed
   YAML, a supersession chain, and a fallen force. Expected answers (which decisions are stale,
   at what distance, which diagnostics exist) are written once in the corpus README as the
   oracle. Every subsequent task verifies against this corpus.
2. **Per-task acceptance criteria.** Each plan task carries a "you know you're done when"
   clause: a specific test command plus the expected observable result against the fixture
   corpus. The executing agent never judges its own doneness subjectively.
3. **`forge-inspect` from Phase 1.** `forge-inspect <forge.toml> [--json]` dumps the snapshot:
   records parsed, diagnostics, verdicts, and (once recall exists) `--search "query"` results.
   It grows with the pipeline and gives the agent an end-to-end probe after every task instead
   of waiting for the Phase 4 MCP server.
4. **Phase-end checkpoints.** Each phase closes with an integration checkpoint committed as a
   test: inspector/pipeline output on the fixture corpus compared against the oracle, so
   regressions in later phases fire immediately.
5. **TDD within tasks.** Tasks are specified test-first: the plan states the behavior and the
   test before the implementation.

Component-level emphasis: the judge gets table-driven cases for transitivity, distance, and
order-independence plus a property test (verdicts invariant under record shuffling); the record
layer gets round-trip tests; recall is tested against the fake embedder (no model download in
unit tests), with one gated integration test for the real ONNX provider. The final integration
test is spec §10 verbatim, over MCP.

## 8. Phase breakdown

- **Phase 0 — scaffold:** git repo, Cargo workspace, crates, config loading, fixture corpus +
  oracle, `forge-inspect` skeleton, green `cargo test`.
- **Phase 1 — pure core:** A record layer → B discovery → C linker → D graph → E judge →
  F snapshot. Deterministic, fixture-tested, no ML/MCP. Checkpoint: inspector answers
  `why`/`stale_report`-shaped questions against the oracle.
- **Phase 2 — recall stack:** J embedder trait + fake impl → real ONNX provider with model
  download/cache → vector cache → G search + near-matches. Checkpoint: `forge-inspect --search`
  returns sane rankings on the corpus.
- **Phase 3 — guardian:** propose (pure) → commit with de-dup gate and synchronous rebuild →
  `set_status` transitions → supersede. Checkpoint: scripted propose→commit→set_status run
  shows staleness propagating in inspector output.
- **Phase 4 — MCP integration:** `rmcp` server, six tools, config wiring, spec-§10 acceptance
  test, README.

## 9. What "done" means

Unchanged from spec §10: entirely by conversing with an agent over MCP — create a decision and
its forces without creating duplicate forces; mark a force `changed`; see every decision resting
on it, directly or transitively through the reticolo, surface as premise-stale. Files remain
plain Markdown in the user's own directories.
