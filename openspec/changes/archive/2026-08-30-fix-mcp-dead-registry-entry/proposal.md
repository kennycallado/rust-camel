# Proposal: fix-mcp-dead-registry-entry

## Why

An MCP consumer that dies without `stop()` leaves its tool and resource
entries in the per-listener registries. The duplicate-registration guard
then refuses a same-name restart in the same process. The route stays
unstartable until the process restarts (bd rc-apvm).

Investigation (e_glm, 2026-08-29) confirmed the defect and sharpened its
shape. A plain bridge panic is already cleaned by camel-core's
finally-`stop()`. The durable leak windows are: consumer-task abort at an
await point, panic in the outer consumer task, and drop without `stop()`.
In all three the `McpConsumer` dies while its registry entries live on.

The registries are in-memory and process-global. So pid-scoped keys solve
nothing: the reported case is same-process. Lease/heartbeat adds timers
and self-supervision, which ADR-0007 forbids for consumers.

## What Changes

- Registry entries (`McpToolRegistry`, `McpResourceRegistry`) carry an
  owner-liveness token (`Weak<()>`) minted by `McpConsumer::start()`.
- `register()` replaces an entry whose owner is dead. It still rejects a
  name or URI held by a live owner. This keeps the concurrent
  same-name race guarantee.
- `unregister()` becomes owner-conditional. A late `stop()` of a dead
  owner cannot delete the successor's entry. Route security plans take
  the same ownership discipline: a live owner's plan is not
  overwritten, a dead owner's plan is replaced, and a stale stop
  cannot downgrade dispatch to unauthenticated pass-through.
- `resolve()` and `list_ready()` prune dead-owner entries. A crashed
  tool stops being advertised and its cap slot is reclaimed. Cap
  enforcement prunes dead-owner entries first, so a dead entry under
  any name cannot block a new registration.
- The duplicate fast-path in `start()` consults liveness, so it does not
  veto a legal takeover.
- Docs: a short new ADR (amends ADR-0060) records the trade-off. The
  crate CONTEXT.md and CONTEXT-MAP Key Terms entries are refreshed.

Excluded: the same latent class in camel-http/camel-ws/camel-grpc
(follow-up bd, not verified here); cross-process staleness (no such
path exists); any camel-core change.

## Acceptance criteria

- A consumer aborted or dropped without `stop()` no longer leaves a
  permanent entry. A same-name restart in the same process succeeds.
- A live duplicate is still rejected with the existing error.
- A late `stop()` of a dead owner leaves the replacement's entry and
  security plan intact.
- Dead-owner entries disappear from `tools/list` / `resources/list` and
  release their cap slots.
- Regression tests T1-T6 run green via
  `cargo nextest run -p camel-component-mcp` in the feature worktree.

## Risk budget

- Change stays inside `camel-component-mcp`. No boundary crossings, no
  new config surface, no timers.
- Live-duplicate detection must not weaken. That guard protects the
  check-then-register race documented in `registry.rs`.
- Registry `register`/`unregister` signatures change. Callers are
  in-workspace and pre-1.0, so this is acceptable.

Bd: rc-apvm
