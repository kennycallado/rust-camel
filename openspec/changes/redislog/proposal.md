# Proposal: redislog

## Why

`camel-redis` stream recovery emits `info!("Stream recovery pass completed")` on
every recovery pass (`crates/components/camel-redis/src/streams.rs:1537`). The
pass cadence follows `streamRecoverMinIdleMs` (default 60 000 ms,
`config.rs:914`), so one stream+group pair writes ~1 440 identically-shaped
lines per day in steady state, and integrations with several streams multiply
it. The line mixes three situations of very different value:

- `pending=0 claimed=0` — normal state, no value; the overwhelming majority.
- `claimed>0` — real operational event: entries were reclaimed.
- `pending>0 claimed=0` — the useful diagnostic: group backlog without ack;
  sustained means a consumer is not acking or is blocked.

All three are `info!` and indistinguishable, so the case that matters drowns
in zeros. bd: `rc-6qoxs` (p2).

## What Changes

Split the log level, keep the fields: emit `info!` only when
`batch.recovered.len() > 0 || pending_count > 0`; emit
`debug!(stream, group, pending, "Stream recovery pass idle")` when both are
zero. The four INFO field names (`stream`, `group`, `pending`, `claimed`)
stay identical so existing log queries keep working. `pending` keeps its
`Option` rendering (`-` when the server did not report a count).

No behaviour change in the recovery loop: same XAUTOCLAIM, same cursor, same
retry accounting, same cancellation handling. No new metric — a pending gauge
is a policy decision that belongs to `rc-e6324`.

Capability not affected: no spec governs log **levels**; the line's existence
and pending count are governed by `redis-streams-consumer`
(`openspec/specs/redis-streams-consumer/spec.md:313-341`) and are preserved.
`skip_specs: true` — no spec deltas. ADR-0012 covers `error!` placement, not
steady-state `info!` cadence.

## Acceptance criteria

- Idle pass (`pending=0`, `claimed=0`): no `INFO` event with that message;
  the `DEBUG` idle line is emitted.
- Pass with `claimed>0`: `INFO` with the four fields intact.
- Pass with `pending>0 claimed=0`: `INFO` (backlog diagnostic).
- The `-` rendering for an absent pending count stays exercised.
- `cargo test -p camel-component-redis` green; recovery-loop behaviour
  unchanged.

## Risk budget

- Affected crates: `camel-component-redis` only (`streams.rs` + stream tests).
- Out of bounds: SCAN family (`rc-r3m6x`), pubsub/queue paths, recovery
  semantics, new public API, new config keys, message text beyond the new
  idle line, the pending gauge (`rc-e6324`).
- Low risk: log-level branch only; no consumer-visible behaviour change.
