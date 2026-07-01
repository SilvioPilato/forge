# Forge v0 — Build Specification

**Status:** ready to decompose
**Audience:** an agentic coding tool (OpenCode) splitting this into operational tasks.
**Scope of v0:** the *premise axis*, end to end. The code axis and everything else are deferred — their *form* is reserved in the format, their *behavior* is out of v0.

> Read §9 before creating tasks. Several decisions are intentionally open; do not resolve
> them by guessing — surface them.

---

## 1. Product in one line

A local-first engine for justified, perishable decisions: you write decisions and the
reasons behind them by talking to an agent, without duplicating reasons, and you learn
which decisions wobble when a reason falls. Files stay yours; everything else is derived.

---

## 2. Core principles (invariants — must hold across every component)

1. **Files are the sole source of truth.** The index/graph/snapshot is derived and MUST be
   fully reconstructable from files alone.
2. **Derived state is never written back to files.** Staleness verdicts, the graph, and
   vectors live only in the snapshot (in memory), never in frontmatter.
3. **Immutability with lifecycle.** A Decision's content is immutable; change = a new
   Decision that `supersedes` it. A Force's *content* is immutable, but its *status* is a
   dated log (see §3), not an overwritten field.
4. **Writes only through the guardian, append-only.** No tool mutates immutable content.
   The only permitted in-place change is appending a status transition.
5. **Borrow capabilities, not engines.** Build the core; depend only on thin, replaceable
   seams (embedding provider, MCP SDK; git later). No external service, no server.

---

## 3. Data model

Two record types, one file each, UTF-8 Markdown with YAML frontmatter. Frontmatter is the
machine contract; the body is prose the machine ignores.

### Decision
```
id           : string, globally unique, stable (the address; survives file moves)
type         : "decision"
title        : string
status       : "proposed" | "accepted" | "rejected" | "deprecated"
date         : ISO-8601 (when status last set)
cites        : [force-id]          # the "why"
supersedes   : [decision-id]       # optional
relates      : [decision-id]       # optional
anchors      : [anchor]            # OPTIONAL; RESERVED, ignored by the v0 judge (§6)
tags         : [string]            # optional
```

### Force
```
id           : string, globally unique, stable
type         : "force"
title        : string (the claim, as a proposition)
dependsOn    : [force-id]          # RETICOLO: a force may rest on other forces
status_log   : [ {status, since} ] # DATED LOG, not a single value
                                   #   status ∈ "holds" | "changed" | "retired"
                                   #   ordered; last entry = current status
supersededBy : force-id            # optional (set when retired in favor of a successor)
tags         : [string]            # optional
```

### The four amendments (relative to the earlier format draft)
- **A1 — Reticolo:** `Force.dependsOn` exists. Propagation is transitive over `cites` ∪
  `dependsOn` (§5, component E).
- **A2 — Force status as dated log:** `status_log` replaces a scalar `status`. Preserves
  history so future as-of queries are possible; v0 reads only the last entry.
- **A3 — Anchors optional & deferred:** `anchors` may be present but the v0 judge does not
  evaluate them. A decision with no anchors is normal, not degraded.
- **A4 — Absent referent ≠ dangling:** dangling detection applies only to *id* references
  (`cites`, `supersedes`, `relates`, `dependsOn`). An anchor pointing at a non-existent
  code referent is never a dangling error (moot in v0 since anchors are inert, but the
  linker MUST NOT treat anchor targets as id references).

---

## 4. Components and responsibilities

Each is a candidate workstream. Signatures are conceptual, not prescriptive.

**A. Record layer** — parse a file into `Decision | Force | ParseError`; serialize a record
back to a file. Owns the frontmatter schema and validation of individual records. Body is
carried as an opaque string.

**B. Discovery** — given the configured *roots* (a list of directories), walk them and
return candidate record files. Roots define the universe; a record's physical location is
incidental.

**C. Linker** — turn id-strings into a resolved graph. Detects and reports, as data (not
exceptions): id collisions (global uniqueness), dangling id references, and **cycles over
`dependsOn`** (a force may not rest on itself directly or transitively). Produces
first-class "dangling edge" endpoints rather than failing.

**D. Graph model** — the in-memory structure: records arena, `id → record` map, reverse
index `force → decisions that cite it`, `dependsOn` adjacency (and its reverse), and the
supersession partial order. Pure; no I/O.

**E. Judge (premise axis only)** — compute the derived verdict per record:
- *premise-stale*: a Decision is premise-stale if any Force reachable via `cites` then
  transitively via `dependsOn` has current status `changed`/`retired`. Monotone transitive
  closure over the reverse of (`cites` ∪ `dependsOn`); order-independent; carries distance
  from the fallen root as metadata.
- *superseded/frontier*: a Decision is superseded if named in another's `supersedes` or is
  `deprecated`. The "present" is the frontier (maximal, non-superseded).
- The code axis is NOT computed in v0.

**F. Snapshot + build** — the pure function `roots → snapshot`. Snapshot is an immutable
value holding D+E outputs plus diagnostics (collisions, dangling, cycles, parse errors) as
data. A partial graph is valid: broken records become diagnostics, good records still
answer. Rebuildable from scratch at any time.

**G. Recall** — semantic anchoring for search and de-duplication. Holds a brute-force
vector store (vectors live in the snapshot, cosine over an in-memory array — no vector DB).
Exposes `search(query, scope?) → ranked candidates`. Provides the de-dup gate: given a
candidate force, return near-matches above threshold. Depends on J.

**H. Guardian / write layer** — the only writer of files. `propose_*` is pure (composes the
record + validation result + near-match info, touches nothing). `commit` re-validates
against the current snapshot, applies the de-dup gate, writes append-only, triggers a
reindex. `set_status` appends a legal status transition. `supersede` writes a new Decision
linking the old. No raw write / edit / delete exists.

**I. MCP server** — wires the six tools (§5) to G, H, and the snapshot. Integration layer;
built last.

**J. Embedding provider** — a thin seam `text → vector`. Default: an embedded model run
in-process (ONNX Runtime or llama.cpp-class), weights downloaded once and cached on disk
(`~/.cache/forge/models/…`), offline thereafter. Alternatives (external API, existing local
server) sit behind the same seam. Note: vectors from different providers are not
comparable; changing provider requires recomputing all vectors on the next reindex (cheap,
since vectors are derived).

---

## 5. MCP surface (six tools)

- `search(query, scope?)` — semantic search over the frontier (force/decision/both). Called
  *before* proposing, so the agent cites real ids.
- `get(id)` — full record + its graph neighborhood.
- `why(id)` — traverse `cites` (and transitively `dependsOn`) to the forces; report each
  force's current status.
- `stale_report(filter?)` — records with a non-fresh premise verdict; filterable/orderable
  by severity (severity model is open — §9).
- `propose_decision(...)` — **pure.** Returns the composed record, validation, the forces
  that would be created, and their existing near-matches. Safe to call repeatedly.
- `commit(proposed)` — writes the proposed bundle after re-validation + de-dup gate; append
  only; triggers reindex. Runs only after the user's in-band assent.
- `set_status(id, status)` — appends a legal status transition (Decision
  `accepted→deprecated`; Force `holds→changed→retired`). The tool that lights up
  propagation.

De-dup is not a separate mechanism: `search` and the gate share one engine, two entry
points. In `propose` near-matches are information; in `commit` they are policy (above
threshold → reuse the existing id unless an explicit "this is genuinely new" signal).

---

## 6. v0 boundary

**In v0:** data model (both records, reticolo); files-as-truth + roots; discovery; linker
(collisions, dangling, cycles); graph model; judge (premise axis + frontier only); force
status as dated log; guardian authoring (propose→assent→commit, set_status, supersede,
append-only); recall (search + de-dup, embedding via pluggable provider, brute-force
vectors); read queries (get, why, stale_report on the present).

**Deferred — form reserved, behavior absent:** the code axis / `anchors` evaluation (field
present, judge ignores it; git/drift borrowed later); as-of queries (data exists via
`status_log`, querying does not); CLI, file watcher, incremental reindex.

**Out entirely for now:** GUI, graph visualizer, profiles beyond ADR, embedded vector DB,
external substrates (Graphiti).

---

## 7. Reuse boundary (build vs borrow)

- **Build:** the whole core — record layer, discovery, linker, graph, judge, guardian
  semantics, de-dup policy, snapshot. This is the differentiator and where the learning is.
- **Borrow (capabilities, behind seams):** the embedding model (via ONNX/llama.cpp-class
  runtime); the MCP protocol (SDK). Later, git/drift for the code axis.
- **Do not adopt engines** that want to own the graph, temporality, or supersession (e.g.
  Graphiti) — that is the core.

---

## 8. Suggested work split

Ordered by dependency; items on the same tier are parallelizable.

- **Tier 0 (no deps):** A Record layer · B Discovery · J Embedding provider seam (+ default
  model download/cache).
- **Tier 1 (needs A/B):** C Linker · D Graph model.
- **Tier 2 (needs C/D):** E Judge (premise) · G Recall (needs A + J).
- **Tier 3 (needs C/D/E):** F Snapshot + build function (assembles the pipeline) · H
  Guardian (needs A for serialize, C for validation, G for de-dup gate).
- **Tier 4 (integration, last):** I MCP server wiring the six tools to F/G/H.

Natural parallel fronts: the *pure core* (A→C→D→E→F) and the *recall stack* (J→G) can be
built independently and meet at H/I. The guardian (H) is the other seam that joins writing
to the pure core.

---

## 9. Decisions deliberately NOT fixed — do not invent; surface these

1. **Process model:** long-running daemon (snapshot kept warm, embedding model loaded once)
   vs one-shot command (rebuild + reload per invocation). Affects cold-start cost.
2. **Reindex coherence:** after `commit`, is reindex synchronous (commit returns only once
   the snapshot reflects the write) or asynchronous? Determines whether an agent that writes
   then reads sees its own effect immediately.
3. **Derived-index materialization:** snapshot purely in-memory (rebuilt each start) vs
   materialized on disk (e.g. SQLite). This choice also decides where vectors live.
4. **Embedding model + dimension:** specific model, vector dimension, on-disk cache format.
5. **Severity model** for `stale_report` ordering (inputs are fixed by E; the ranking is
   not).
6. **De-dup threshold** value and the exact "genuinely new" override signal.
7. **Target language/stack:** not load-bearing to the design (all components are defined by
   responsibility, not implementation). A single-binary, in-process-embedding, local-first
   profile favors a systems language; confirm before scaffolding.

---

## 10. What "done" means for v0

You can, entirely by conversing with an agent over MCP: create a decision and its forces
without creating duplicate forces; mark a force `changed`; and see every decision that
rests on it — directly or transitively through the reticolo — surface as premise-stale.
On files that remain plain Markdown in your own directories. No code axis, no time travel,
no interface beyond MCP.
