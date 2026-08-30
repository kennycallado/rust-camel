# ADR-0068: MCP Registry Owner-Liveness Entries

**Date:** 2026-08-29
**Status:** Accepted
**Origin:** bd rc-apvm
**Cross-refs:** ADR-0007, ADR-0060

## Context

The MCP server role registers one tool or resource route per consumer
start. Each entry maps a name or URI to a route. The registries are
process-global singletons (ADR-0060). A consumer that dies without a
stop leaves its entries behind. The entries are dead: their channels
never deliver again.

A dead entry blocks a legal takeover. A restart of the same route hits
the duplicate guard and fails. The dead entry also holds a catalog cap
slot. Listings advertise a tool that can never answer.

## Decision inputs

### Input 1: lease or heartbeat rejected

A lease or heartbeat renews liveness on a timer. It fixes every crash
shape, but needs a renewing task and TTL tuning. A restart faster than
the TTL still hits the duplicate guard. Heartbeats are self-liveness
machinery. ADR-0007 forbids consumers from self-supervising. Rejected.

### Input 2: pid-scoped keys rejected

Pid-scoped keys would key entries by process id. The registry is an
in-memory process-global singleton. There is no cross-process path. The
reported case is same-process. Rejected as moot.

### Input 3: sender.is_closed() rejected

`sender.is_closed()` is true only when the bridge died. On
drop-without-stop the bridge detaches and parks on `rx.recv()` forever.
The channel stays open. Rejected as insufficient.

### Input 4: Drop-guard rejected

A Drop-guard on the consumer covers drop and abort. The runtime takes
the bridge handle, so `Drop` cannot abort the detached bridge. It still
needs owner-conditional unregister to avoid deleting a successor's
entry. Redundant once replace-on-conflict exists. Rejected.

## Decision

`McpConsumer::start()` mints an `Arc<()>` token and stores it in
`Running`. Each registry entry and route security plan carries a
`Weak<()>` view of that token. The token dies when the consumer dies:
task abort, unwind, or plain drop.

`register()` replaces a duplicate entry whose owner token is dead. A
live owner keeps today's rejection. Before cap enforcement, `register()`
prunes every dead-owner entry, so a dead entry under any name releases
its slot. `resolve()` and `list_ready()` skip and remove dead-owner
entries. The unregister used by `stop()` and failure cleanup is
owner-conditional. `unregister_owned` removes an entry only when the
caller's token matches the entry's token (`Weak::ptr_eq`). A late stop
of a dead owner cannot delete a replacement's entry.

`McpBindSecurity` plans take the same discipline. `register_plan` is
owner-scoped: a plan held by a live owner is kept, a dead owner's plan
is replaced. The unregister used by `stop()` and failure cleanup is
owner-conditional. `unregister_plan_owned` removes a plan only when the
caller's token matches the entry's token (`Weak::ptr_eq`). `plan_for`
ignores plans whose owner is dead.

## Consequences

### Dead entries stop blocking takeover

A restart of a dead route replaces the stale entry. The duplicate guard
rejects only live-owner duplicates. A failed duplicate start cannot
remove or overwrite the incumbent consumer's plan.

### Dead entries release cap slots

The prune-before-cap sweep reclaims slots without waiting for an
unrelated list or resolve operation. The sweep is bounded by the cap
itself.

### Listings stop advertising dead tools

`list_ready` prunes dead-owner entries before listing. Dead tools and
resources disappear from `tools/list` and `resources/list`.

### In-flight requests fail cleanly

A request that resolved an entry before its owner died sends into a dead
channel. It returns a clean MCP error. The route body never runs.

### Plan removal cannot open an auth hole

A missing plan means unauthenticated pass-through. A late stop of a dead
owner cannot remove a live replacement's plan. Dispatch stays
authenticated.

### No new supervision machinery

There are no timers, heartbeats, or new config knobs. ADR-0007 keeps its
posture: consumers do not self-supervise.