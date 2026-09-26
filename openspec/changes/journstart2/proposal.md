# Proposal: journstart2

## Why

gh#52 (bd rc-rur55, owner-filed, reproduced on v0.50.0 and v0.54.0): with
`runtime_journal` enabled, `camel run` auto-starts routes only on the first
boot against a fresh journal. On every later boot the journal appends
`RouteRegistered` but never emits `RouteStartRequested`; the route stays
registered and dark, the context logs `CamelContext started`, and the process
exits 0 — silent at every log level including DEBUG. Production restarts
leave integrations down with no signal.

Root cause (validated in-worktree by reproduction with temporary tracing,
credit PR #53 for the diagnosis): the durable command dedup store replays
command IDs from prior boots, and context lifecycle command IDs are
`context:{op}:{route_id}:{seq}` with a process-local sequence that resets
every boot. Boot 2 re-issues boot 1's `context:start:{route}:0`; the dedup
check returns `Duplicate` with no logging and the start is suppressed.

## What Changes

- Make context command IDs boot-unique with a journal-derived boot nonce:
  `context:{op}:{route_id}:{nonce}:{seq}`. The nonce is derived at journal
  recovery from the durable dedup store (deterministic per boot, unique
  across boots against every still-recorded ID, no wall clock). If the
  deterministic nonce value space is exhausted (an adversarial-only state),
  the boot fails closed with an explicit startup error instead of issuing a
  command ID.
- Add a no-silence guard: after the auto-startup loop in `start_context`,
  warn when a StartRoute result was `Duplicate` or an auto-startup route is
  registered but not `Started` (gh#52 explicitly requires this signal).
- Regression test: boot → drop → reboot on the same journal asserting
  `RouteRegistered → RouteStartRequested → RouteStarted` on every boot.
- Spec deltas: `runtime-boot` (auto-start on every boot with a durable
  journal), `lifecycle-correctness` (silent suppression is observable).

Affected crate: `camel-core`. Explicitly excluded: the Camel.toml profile
loading gotcha from gh#52 (top-level `[runtime_journal]` silently discarded —
tracked separately as rc-zbyv, NOT bundled here) and any compaction or
dedup-retention policy change.

## Acceptance criteria

- With a durable journal, every boot appends `RouteRegistered →
  RouteStartRequested → RouteStarted` for each `auto_startup` route and the
  route reaches `Started` state on every boot (regression test proves it).
- Command IDs are deterministic functions of journal state: two boots against
  identical journal state derive the same nonce; no issued ID ever equals a
  still-recorded ID (including legacy four-segment IDs from older versions).
  If the deterministic nonce space is exhausted, the boot fails closed with an
  explicit startup error naming the offending recorded ID instead of issuing
  a command ID.
- A suppressed (duplicate) or otherwise not-started auto-startup route
  produces a WARN naming the route; `auto_startup = false` routes stay silent
  by design.
- No lifecycle behavior change when no journal is configured (no journal
  recovery runs; command IDs gain only a constant zero nonce segment, which
  nothing parses).

## Risk budget

- Core lifecycle path is P1 production surface: the fix must be minimal and
  behind the existing dedup semantics — no dedup TTL changes, no journal
  format changes beyond the new ID shape, no new events.
- A behavior change for deployments already relying on the bug (routes dark
  after restart) is accepted and is the point of the fix; the guard warns
  rather than fails the boot.
- Wall-clock nonce (PR #53 baseline) is rejected: the fail-closed boundary
  removes the need for any fallback, and non-determinism (same journal state,
  different IDs across runs) is the reason to avoid it, not correctness.
