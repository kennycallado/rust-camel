# Proposal: aliaswait

## Why

`cargo xtask lint-unbounded-wait` (mission 201, bd rc-3lx2, ADR-0069
§13.2 R1) resolves imported names only for single-segment call paths.
Multi-segment paths are compared literally against the target lists, so:

- `use tokio::time as clock;` then `clock::timeout(d, fut).await` is not
  recognized as a deadline — inner waits are reported (false positive).
- `use tokio::task as task;` then `task::spawn(work()).await` is never
  matched against `tokio::task::spawn` (false negative).
- Glob imports (`use tokio::time::*;` + bare `timeout(...)`) create no
  binding at all — the bd rc-orivx gap that forced master-test loops to
  be rewritten fully-qualified just to sidestep the lint.

This is exactly the retro520 blind-spot class (bd rc-t9h9n, P2): lint
rules bypassed via aliases and qualified paths.

## What Changes

- `scripts/xtask/src/lint_unbounded_wait.rs`: `is_path_target` becomes
  a candidate-set resolver — it expands the first segment of qualified
  call paths through the per-module import chain (module aliases,
  transitive alias chains with leading-name cycle detection,
  set-valued expansion that branches through glob-derived prefixes)
  and synthesizes glob-root candidates, including glob imports in
  nested blocks. Matching is class-aware over provenance-tagged
  candidates: bounding follows precedence rules (a named target
  reading from the fn-body top or a module scope bounds unless
  contradicted by another named reading, a local binding, or an
  inner-scope glob — nested-block named readings only block, never
  bound; a literal/extern target
  reading bounds unless a named reading or local binding exists — an
  explicit, documented scope relaxation; a glob-only singleton target
  bounds); wait/spawn targets match if any candidate hits (ambiguity
  never suppresses a wait finding). Per-module local items are tracked
  namespace-aware (value items terminate single-segment lookups, type/
  module items terminate qualified prefixes — `type spawn = ();` does
  not suppress a value-namespace call), and test-fn-body `use`
  statements become scope-aware (top-level body uses join the body
  scope, nested-block uses union non-terminal candidates — no import
  is ever terminal). Leading-colon absolute
  paths (`::tokio::…`) bypass alias expansion per Rust semantics.
- Value-namespace bindings are scope-aware and terminality follows
  syntactic namespace knowledge ONLY: top-level fn-body item
  statements (fn/const/static/tuple-struct/unit-struct) terminate
  single-segment lookups legitimately; imports (module-level or
  fn-body) and every other local binding (let/closure/patterns,
  nested-block statements) contribute non-terminal candidates, so a
  late, nested, or cross-scope binding never suppresses detection of
  an awaited import call. Bounding behavior is unchanged by this; only
  detection stops being suppressible.
  Qualified-path prefixes are module-namespace, so value locals never
  suppress qualified expansion. Same-scope same-name imports (type +
  value namespace) union as candidate readings instead of
  overwriting; absolute `use ::…` targets and `extern crate … as …`
  aliases are collected and keep their absolute semantics (immune to
  alias rewriting).
- Adversarial unit tests in both directions (bounding recognized /
  waits detected), including shadowed aliases, transitive aliases,
  innermost-wins rebinding, and glob precedence soundness.
- The existing pinned conservative glob test flips expectation (a
  resolved glob import now bounds).
- Module doc + ratchet-file header updated to the new resolution
  contract.

Excluded: stream I/O awaits, sync blocking waits, method-form deadlines
(`fut.timeout(d)`) — documented V1 blind spots unchanged.

## Acceptance criteria

- All adversarial fixtures pass: aliased `clock::timeout`/`timeout_at`
  bound inner waits; aliased `task::spawn`/`spawn_blocking`/
  `net::TcpStream::connect` awaits are detected; glob-imported
  `timeout` bounds loops and future-args; shadow, explicit-import,
  scope-local-item, and nested-block-import precedence hold; ambiguity
  never bounds and never suppresses a wait finding.
- `cargo xtask lint-unbounded-wait` stays green at ceiling exactly 395:
  measured on the live tree, the base detector reports 394 and the
  precedence-rule detector lands at 395 — the baseline plus ONE new
  true finding from the fixed false-negative class (an awaited
  `TcpStream::connect` through a named import, previously missed by
  literal matching; inventoried in the ratchet file with a dated
  justification, tracked as bd rc-krbym). Verified by a base-vs-new
  finding-list diff on the same tree (+1 site, 0 removals; the
  pure-singleton rule was measured at +108 glob-ambiguity over-reports,
  which the precedence rules relieve). Ceiling never rises without new
  true findings; if a fix lowers the count, lower it with a burn-down
  note.
- `cargo fmt --check`, `cargo clippy -p xtask -- -D warnings`, and the
  xtask test suite are green.
- bd rc-orivx is subsumed (glob-imported timeout now bounds loop
  subtrees) and closed with a supersede note.

## Risk budget

Detector-only change under `scripts/xtask` — no runtime crate is
touched. Main risk: unsound bounding (a real unbounded wait suppressed)
through glob synthesis; mitigated by provenance-tagged precedence
rules (named/local/inner-glob contradictions block; nested-block
named readings never bound — block-local visibility is unconfirmable;
the one accepted
relaxation — a glob masquerading an extern crate name — is documented
in the residual ledger with its adversarial-only precondition), the
dual-glob ambiguity compile-error argument, and soundness-pinning
tests including negative cases for each blocking rule. The
opposite risk — suppressed detection — is excluded by construction:
only namespace-known item bindings terminate, and every other reading
is retained. The ratchet ceiling is the regression signal; it may not
move without a findings-diff justification.
