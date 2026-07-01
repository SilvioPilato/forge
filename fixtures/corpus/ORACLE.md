# Corpus Oracle

## Counts
- Files discovered: 19; parsed OK: 18 (10 forces incl. both duplicates, 8 decisions); parse errors: 1.

## Diagnostics (exactly these)
- ParseError: forces/malformed.md
- IdCollision: id `f-duplicate` (f-dup-1.md, f-dup-2.md)
- DanglingRef: from d-dangling, field cites, to f-missing
- DependsOnCycle: members {f-cycle-a, f-cycle-b}

## Verdicts
- Premise-stale: d-keep-legacy (f-retired-old, retired, distance 1);
  d-embed-onnx (f-onnx-portable, changed, distance 1);
  d-small-model (f-onnx-portable, changed, distance 2);
  d-old-storage (f-retired-old, retired, distance 1) — but superseded, so NOT in stale_report.
- Fresh: d-use-rust, d-new-storage, d-dangling (unresolved refs don't fall), d-deprecated.
- Superseded / not frontier: d-old-storage (via d-new-storage.supersedes), d-deprecated (status).
- Frontier decisions: d-use-rust, d-embed-onnx, d-small-model, d-keep-legacy, d-new-storage, d-dangling.

## stale_report order (severity: retired > changed, then ascending distance)
1. d-keep-legacy   2. d-embed-onnx   3. d-small-model

## why(d-small-model)
f-model-small (holds) -> dependsOn -> f-onnx-portable (changed).
