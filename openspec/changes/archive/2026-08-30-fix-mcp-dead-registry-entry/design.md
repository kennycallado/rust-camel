# Design: fix-mcp-dead-registry-entry

## Approach

Owner-liveness tokens with replace-dead-on-conflict and lazy prune.

1. `McpConsumer::start()` mints an `Arc<()>` token, stores it in
   `Running`, and passes a `Weak<()>` into `register()`.
2. `ToolEntry` and the resource mirror carry `owner: Weak<()>`. The
   cloned dispatch snapshot does not need the field.
3. `register()` duplicate branch: if the existing entry's owner cannot
   upgrade, replace the entry and log a `warn!` naming the dead route.
   A live owner keeps today's rejection. Before cap enforcement,
   `register()` prunes every dead-owner entry in the registry, so a
   dead entry under any name releases its slot without waiting for an
   unrelated list or resolve operation. The sweep is bounded by the
   cap itself. A replace does not change the count, so it bypasses
   the cap check.
4. `unregister()` gains an owner-conditional form (`Weak::ptr_eq`).
   `stop()` uses it. A late old-owner stop therefore cannot delete a
   replacement's entry.
5. `resolve()` and `list_ready()` skip and remove dead-owner entries.
   Listings stop advertising dead tools and cap slots return without
   waiting for a restart.
6. The consumer fast-path (`consumer.rs` step (d)) consults liveness
   through a registry helper, so the friendly error does not veto a
   legal takeover.
7. `McpBindSecurity` plans take the same ownership discipline:
   - Each plan entry retains the registering consumer's owner token.
   - `register_plan` is owner-scoped. A plan for a `route_id` held by
     a live owner SHALL NOT be overwritten: the newcomer keeps the
     incumbent's plan and fails the duplicate guard at step (d). A
     dead owner's plan is replaced.
   - `unregister_plan` is owner-conditional. A late `stop()` of a dead
     owner cannot remove a live replacement's plan, which would drop
     dispatch to unauthenticated pass-through (`plan_for` returns
     `None`).
   - `plan_for` and the exposure-gate snapshot ignore plans whose
     owner is dead. This is safe for fail-closed dispatch because a
     dead owner's tool or resource entry is pruned at resolve, so no
     live dispatch path can reach a dead plan. An in-flight request
     that resolved an entry before its owner died sends into a dead
     channel and returns a clean MCP error; the route body never
     runs.
   - The failure-path cleanup in `start()` (duplicate rejection,
     resource-register error) uses the owner-conditional unregister.
     A failed duplicate start therefore cannot remove or overwrite
     the incumbent consumer's plan.
   - Winner re-assertion: after `register()` succeeds at step (d), the
     entry winner re-asserts its plan via an overwrite form that
     replaces a plan whose owner is another consumer (a live loser
     kept its plan at step b2 but lost the entry race; its failure
     cleanup removes only that loser plan). Entry ownership proves the
     right to the route identity, so the re-assertion is not blocked
     by a live incumbent plan. This closes the concurrent-restart
     window where a live entry would otherwise end with no plan and
     dispatch would fall to unauthenticated pass-through.

The token dies whenever the `McpConsumer` struct dies: task abort drops
the future and the consumer with it, unwind drops it, plain drop drops
it. A bridge panic with the consumer task alive is already covered by
camel-core's finally-`stop()` (consumer_management.rs:690). If that
finally is later aborted, the consumer dies with the task, so the token
dies too. Every leak window closes.

This design is the sibling of two in-repo precedents that replace state
belonging to dead owners: dead-server eviction via
`monitor_task.is_finished()` (registry.rs:216-230) and
`HealthCheckRegistry::force_unhealthy_for_route` (CONTEXT-MAP.md:118).

## Affected crates

- `camel-component-mcp`: `src/registry.rs` (owner field in both
  registries, replace branch, owner-conditional unregister, lazy prune,
  plan-unregister scope; about 90-130 LoC), `src/consumer.rs` (token
  mint, store, pass-through, fast-path liveness; about 25-40 LoC),
  `tests/server_registry_test.rs` and `tests/server_consumer_test.rs`
  (T1-T6), plus docs.

## Architecture boundaries

The change stays inside the component crate. The data/control plane
split is untouched: dispatch paths keep their shape, and only entry
ownership semantics change. No Runtime, DSL, or camel-core contract
changes. ADR-0007 keeps its posture: consumers do not self-supervise.
There are no timers, heartbeats, or new config knobs (ADR-0038 surface
unchanged). Tests stay in the unit tier of the two-tier contract
(ADR-0064).

Documentation: a short new ADR (`Amends: 0060`) records why
owner-liveness beats lease/heartbeat and pid-scoping. The crate
CONTEXT.md registry section and the CONTEXT-MAP Key Terms entries
("Tool Registry", "Resource Registry") are refreshed in the same change
per the term-landing rule.

## Alternatives considered

- Lease/heartbeat: fixes every crash shape but needs a renewing task
  plus TTL tuning. A restart faster than TTL still hits the duplicate
  guard. Heartbeats are self-liveness machinery, which ADR-0007
  forbids for consumers. Rejected.
- pid-scoped keys: the registry is an in-memory process-global
  singleton. There is no cross-process path and the reported case is
  same-process. Rejected as moot.
- `sender.is_closed()` as the liveness signal: true only when the
  bridge died. On drop-without-stop the bridge detaches and parks on
  `rx.recv()` forever, so the channel stays open. Rejected as
  insufficient.
- Drop-guard on the consumer: covers drop and abort, but the runtime
  takes the bridge handle, so `Drop` cannot abort the detached bridge.
  It still needs owner-conditional unregister to avoid deleting a
  successor's entry. Redundant once replace-on-conflict exists.
  Rejected.
