# Design: mock-declarative-testkit

## Context

Ruling Q3 (2026-08-18): `camel test` boots a `CamelContext` in-process,
registers the real `MockComponent`, runs routes, and asserts **directly on the
`Arc<MockEndpointInner>`** via `MockComponent::get_endpoint`. No IPC, no
RuntimeBus/QueryBus involvement (frozen invariant, re-ratified in change #1's
implementation blessing). Change #1 (58e37e53) shipped the assertion surface
this builds on.

## D1 — Test document format (the deferred contract, decided)

Sidecar documents, NOT inline `expects:` in route files:

```yaml
# <name>.test.yaml
routeFiles: [config/routes.yaml]   # OR inline `routes:` (same schema as route files)
inputs:                            # optional; omitted ⇒ routes must self-start (timer)
  - to: direct:start               # direct: scheme only in v1 (synchronous delivery)
    body: "hello"
    headers: {kind: greeting}
expects:                           # mandatory, ≥ 1 endpoint
  mock:result:
    count: 3                       # exact  → expect_count
    minCount: 2                    # or minimum → expect_minimum_count
    bodies: ["a", "b", "c"]        # ordered → expect_body (anyOrder via mock URI param)
    headers: {kind: greeting}      # → expect_header
settle: 500ms                      # optional quiet-window override (0 < settle <= 5s)
```

Rationale: route files stay pure (no DSL schema change); one document = one
executable test; unknown fields rejected (serde deny); `expects` empty is a
document error. Body scalar rules (v1, strict): string ⇒ `Text`; object/array
⇒ `Json`; null/boolean/number ⇒ document error (exit 2) — widening later is
additive.

## D2 — Multi-document semantics

Documents execute in CLI argument order, sequentially. `routeFiles` paths
resolve relative to the containing test document's directory. A document-level
error (unreadable file, parse error, boot failure, invalid `settle`) is
reported and execution CONTINUES with the next document; within a document,
an expectation failure does not abort the remaining endpoints. Exit precedence
when classes mix: 2 > 1 > 0 (a broken suite outranks an assertion failure).

## D3 — Runner placement

`crates/camel-cli/src/commands/test.rs` + `commands/test/` submodules
(document.rs = serde model, runner.rs = execution). Follows `lint.rs`
precedent: pure `run_test_doc() -> TestOutcome { exit_code, report }` testable
without process spawn; main.rs maps to `process::exit`. No new crate (risk
budget); camel-cli already depends on camel_dsl and camel-component-mock.

## D4 — Boot and route loading (minimal, not run.rs reuse)

`camel run` boot pulls beans/WASM/security/watch — wrong for tests. The runner
builds a lean `CamelContext` registering: mock, direct, timer, log, seda (same
set camel-test's harness proves). Referenced route files load via
`camel_dsl::load_from_file` (the public per-file entry, camel-dsl/src/lib.rs),
which runs the same YAML→declarative→RouteDefinition pipeline `camel run`
uses per file. Difference from `camel run` discovery, acknowledged: the runner
does no globbing, no template materialization of its own, no source-hash
tracking — templates/interpolation inside a route file behave as that parser
defines them.

## D5 — Inputs

`inputs.to` accepts **`direct:` endpoints only** in v1: DirectProducer
delivery is synchronous, so each input completes when its call returns — no
settle dependency for input-driven traffic. Any other scheme is a document
error (exit 2). Self-starting routes (timer) need no inputs; they rely on D6.
Headers set before send.

## D6 — Expectation evaluation and settling

After inputs, the runner sets expectations on each named endpoint obtained via
`mock.get_endpoint(<name>)` (endpoint name = URI suffix after `mock:`), then
asserts with `try_assert_satisfied()` — non-panicking; `MockAssertionError`
Display text flows verbatim into the per-endpoint failure line.
`count`/`minCount` are mutually exclusive per endpoint (document error). A
document-level `expectedCount` on the mock URI coexists: the document's
explicit `expect_count` is set after boot, overwriting it (last-set wins).

Settling (one algorithm, document-wide): the deadline starts when route
execution begins and equals `quiet + 5s` (one full quiet window is budgeted
before the 5-second instability budget — a valid `settle: "5s"` can never
race its own window; ratified at plan-bless round 1). The runner samples ALL expected endpoints'
`received_count()` simultaneously every 50ms; the quiet window (default
250ms, `settle:` override, validated `0 < settle <= 5s` — else exit 2) must
elapse with no sampled count change (any change resets the window). Overshoot
(count above expected) does NOT end settling — only stability does; the
assertion itself decides pass/fail. Deadline hit without stability ⇒ document
fails with a settle-timeout message (exit 1). No fixed sleeps; no per-endpoint
targets.

## D7 — Exit codes & output

`0` all pass; `1` any expectation failure or settle timeout; `2` misuse,
unreadable files, document/route parse errors, invalid `settle`, non-`direct:`
input targets (lint.rs convention: misuse messages to stderr). stdout: one
`PASS|FAIL <doc>#<endpoint>` line per endpoint per document + `N passed, M
failed` summary.

## D8 — camel run non-interference

`*.test.yaml` files are excluded from `camel run`'s route discovery at the
**CLI seam** (camel-cli run.rs: the default-glob construction for initial
load and watcher reload), NOT as a global camel_dsl change — non-CLI callers
of `discover_*` are unaffected. An explicit `--routes` glob naming
`*.test.yaml` is honored (explicit user override; the files then parse as
routes and fail on unknown fields if they are test documents).

## Affected crates & boundaries

camel-cli only: Commands enum + commands/test + the run.rs discovery
exclusion. No Runtime, DSL-core schema, or component changes. Docs: camel-cli
README section + examples/yaml-dsl test document example.

## Risks

- Settle deadline vs slow timers: capped, fails loudly, never hangs.
- run.rs default-glob exclusion must cover initial load AND watch reload
  (two call sites).
