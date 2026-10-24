# Proposal: unbounded-wait-ratchet

## Why

`cargo xtask lint-unbounded-wait` is red on main: 299 flagged sites against ceiling 296
(`scripts/xtask/ratchet-unbounded-wait.max`). Every mission since 387 has exempted this gate,
which trains the fleet to ignore a red gate. Owner directive (bd rc-yfsw6): burn the backlog so
main is green, then lock the ceiling so it cannot creep back.

## What Changes

Classify-then-adjudicate all 299 sites (inventory: `classify/group-{a,b,c}.tsv`, rubric in
`classify/RUBRIC.md`):

- **Bucket A — genuinely unbounded (54 sites)**: bound at the call site with the new shared
  `timeout_wait` helper (or direct `tokio::time::timeout` where a test needs the `Elapsed`
  distinction). Clusters: tonic/TCP/TLS connects (cxf, jms, validator, xslt, grpc, rabbitmq,
  sql), task joins, harness pool/lock acquires.
- **Bucket B — bounded-by-construction (245 sites)**: `// allow-test-wait:` marker with a
  construction-citing one-line reason (test-owned uncontended mutexes, test-fed channels,
  joins of finite harness-owned tasks).
- **Bucket C — production (0 sites)**: none found; every flagged site is test-scoped.

Plus: one shared helper `timeout_wait` in `camel_component_api::test_support` (feature
`test-support`); one doc line in the ratchet file header explaining marker legitimacy;
ceiling lowered 296 → final count (target 0) once the tree is clean. Lint source
(`lint_unbounded_wait.rs`) is NOT modified.

Affected crates: `camel-component-api` (helper), plus adjudication edits across ~40 crates'
test code (camel-test suites, camel-integration-test, components cxf/jms/grpc/stream/validator/
xslt/rabbitmq/sql/redis/llm/mcp/wasm, camel-core, camel-processor, camel-cli, camel-dsl,
camel-bridge, camel-lsp).

## Acceptance criteria

- `cargo xtask lint-unbounded-wait` exits 0 with the lowered ceiling on the final tree.
- Ceiling file value equals the final unadjudicated finding count; ceiling never raised.
- All 299 sites appear in the classification TSVs with bucket + action; C count stated (0).
- No test becomes flaky-slow or silently skips: bounding wraps fail loudly, distinguish
  timeout from channel closure, use no `tokio::time::sleep`.
- One `timeout_wait` helper only; no per-crate reimplementation; lint source untouched.
- Ratchet file header carries one line documenting when a marker is legitimate.

## Risk budget

Acceptable: wide mechanical marker churn (one comment line per site, mechanical, reviewable by
diff). Not acceptable: masking genuine hangs with markers (marker reasons must cite
construction, not convenience), touching production wait behavior, raising the ceiling,
modifying lint scan logic. Historical ratchet-arithmetic scenarios in the spec (431, 296) are
superseded by the full-adjudication end state recorded in the delta spec.

Bd: rc-yfsw6
