# Design: redislog

## Context

`crates/components/camel-redis/src/streams.rs:1537-1547` emits one `info!`
line per recovery pass. Steady state is overwhelmingly idle
(`pending=0 claimed=0`), so the informative cases are indistinguishable from
noise. bd `rc-6qoxs`. Zone lease: crate `camel-redis`.

## Goals / Non-Goals

- Goal: separate the idle line (DEBUG) from the operational lines (INFO),
  preserving field names and the `Option` pending rendering.
- Non-Goal: any recovery-loop behaviour change (XAUTOCLAIM, cursor, retry
  accounting, cancellation stay identical).
- Non-Goal: pending gauge / metric — deferred to `rc-e6324` (stream and group
  are user-provided values; an open-label gauge needs `// allow-open-label`
  under ADR-0041 / `lint-metric-labels`).

## Decisions

### D1 — Level split at the call site

One `if/else` in the `Ok(batch)` arm of the recovery pass:

- Compute `pending` (Option rendering, unchanged) and `claimed`
  (`batch.recovered.len()`) before `queue.extend(batch.recovered)` moves the
  batch.
- `claimed > 0 || batch.pending_count.is_some_and(|c| c > 0)` → keep the
  existing `info!` byte-identical (fields `stream`, `group`, `pending`,
  `claimed`; message `"Stream recovery pass completed"`).
- Otherwise → `debug!(stream, group, pending, "Stream recovery pass idle")`
  (three fields; `claimed` is 0 by construction and omitted per the order).

Edge cases: `pending = None` is treated as not-backlog (the split keys on the
inner count); `claimed > 0` is INFO regardless of `pending`, so
`pending = None, claimed > 0` takes the same INFO branch as test 3.

### D2 — Tests assert levels via the existing capture harness

`stream_session_tests.rs` already installs a capturing tracing layer
(`capture_logs`, `CapturedEvent { level, message, fields }`) with no level
filter, so DEBUG events are recorded. Level assertions are expressible — no
harness change needed.

Tests (all in `stream_session_tests.rs`):

1. `recovery_idle_pass_logs_debug_not_info` — batch `Some(0)`, no recovered
   entries. Assert: no `INFO` event whose message contains
   `"recovery pass completed"`; exactly one `DEBUG` event containing
   `"recovery pass idle"` with `stream`, `group`, `pending="0"`.
2. `recovery_pass_logs_pending_count` (existing, line ~3228) — batch
   `Some(2)`, no recovered entries = the backlog diagnostic. Adjust: also
   assert the event `level == "INFO"` (acceptance #3); field assertions
   (`pending="2"`, `claimed="0"`) unchanged.
3. `recovery_pass_with_claims_logs_info` — batch `Some(0)` plus one
   recovered entry. Assert: one `INFO` event with all four fields
   (`stream`, `group`, `pending="0"`, `claimed="1"`) and no DEBUG idle line.
4. `recovery_idle_pass_without_pending_count_renders_dash` — batch `None`,
   no recovered entries. Assert: the `DEBUG` idle event carries
   `pending="-"` (acceptance #4: `-` rendering stays exercised).

### D3 — skip_specs

`skip_specs: true`. No spec governs log **levels**; ADR-0012 covers `error!`
placement, not steady-state `info!` cadence. One adjacent capability
requirement must be acknowledged:
`openspec/specs/redis-streams-consumer/spec.md:313-341` ("XPENDING inspection
exposes a bounded pending summary per recovery pass") mandates that each pass
logs one line with the pending count through the component's existing log
style, with scenario "pass logs the pending summary" (`:336`). That spec pins
no level: the split preserves one log line per pass carrying the pending
count (INFO when `pending > 0 || claimed > 0`, DEBUG idle line that still
carries `pending`), so the requirement and its `:336` scenario stay green —
design test 2 (pending=2, claimed=0 → INFO) continues exercising it. The
change is not a behavioural capability change.

## Risks / Trade-offs

- Log-based alerting keyed on the completed line at INFO: unchanged for the
  cases that matter (fields identical); idle noise leaves the INFO stream.
- DEBUG visibility: idle lines remain available at debug level for triage.
- The branch adds one `is_some_and` per pass — negligible.

## Migration Plan

None. Level decision only; no config, no API, no data.

## Open Questions

None — the order fixes defect, split, and scope.
