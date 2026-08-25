# Proposal: immediate-start-errors

## Why

bd rc-slvd (discovered from rc-gwvs). When a `ConsumerStartupMode::Immediate`
consumer's `start()` returns `Err`, the route still reports Started and
`start_route`/`resume_route` already returned `Ok`. Mechanism (camel-core
`consumer_management.rs` `spawn_consumer_task`, Immediate branch ~114-129):
the startup receiver handed to the controller is pre-resolved to Ready
(`StartupReceiver::immediate()`), so `await_consumer_startup` returns before
the spawned consumer task has even polled `start()`. The task's error path
(lines ~145-179) only logs `error!`, sends a CrashNotification, and
publishes a runtime failure — none of which fails the lifecycle operation
or the route status projection.

Concrete trigger (the rc-gwvs seda restart bug class): a seda Single-mode
consumer whose `start()` fails "already has a registered consumer" — route
shows Started while `has_active_consumers()` is false and producers fence
every send. Any Immediate consumer with a fast start error (timer validation,
double-start guard, seda ownership) hits the same silence today.

This is the deferred half of rc-gwvs: the seda restart trigger was fixed
(ed8ad4e6); the general silence mechanism for ALL Immediate consumers remains.

## What Changes

- **Fix (camel-core `consumer_management.rs`)**: asynchronous failure
  watcher for Immediate consumers (e_gpt ruling after e_opus REJECT of the
  first synchronous design — see Risk budget). The controller-facing
  startup receiver for Immediate consumers is pre-resolved again
  (`StartupReceiver::immediate()`), so `start_route`/`resume_route`/aggregate
  start/`CamelContext::start()` return without waiting — the fire-and-forget
  timing Immediate consumers always had. The consumer task keeps the
  write-once ok/err latches (biased select, one outcome send total). A
  detached watcher observes the err-latch within the
  `CONSUMER_IMMEDIATE_GRACE` budget and, on error, transitions the Route
  Lifecycle Projection to `Failed` via RuntimeBus `FailRoute`, aborts the
  consumer task, and cancels the consumer token — retrying with the same
  `command_id` after the grace boundary if the first attempt raced a
  pending lifecycle confirmation. The route becomes loudly `Failed` within
  ~grace instead of silently `Started` with producers fenced forever.
- **In-tree census recorded** (design.md): loop-style Immediate = timer,
  file, sql, cron, keycloak (start() runs until cancellation); spawn-and-return
  Immediate = seda, mock, controlbus, log, master, validator, template, exec,
  wasm, opensearch, xj, container, llm; Explicit = direct, http, ws, kafka,
  mqtt, redis, jms, cxf, mcp, surrealdb, grpc.
- **Tests**: unit tests in `consumer_management.rs` (fake Immediate consumer
  failing fast → watcher transitions route to Failed; returning Ok → no
  failure; loop-style → no failure; retry-after-race), route-level
  regression (Immediate consumer with failing start → operation returns Ok,
  route eventually `Failed`, no detached tasks), resume-path equivalent, and
  a deterministic startup-emission reentrancy regression (timer emits during
  start driving a ControlBus command — the controlbus_stops_route class).
- **Docs**: component-api CONTEXT.md startup-handshake section and camel-core
  CONTEXT.md record the watcher semantics.

**Excluded**: migrating the five loop-style Immediate consumers (timer, file,
sql, cron, keycloak) to spawn-and-return; changing `ConsumerStartupMode`
or `StartupReceiver` public API (camel-component-api is an ADR-0049 contract
crate).

## Acceptance criteria

- An Immediate consumer whose `start()` errors before the grace budget
  elapses: the lifecycle operation returns Ok, the route transitions to
  `Failed` within the grace budget plus bounded retry, the consumer task is
  aborted and the token cancelled — no silent `Started`, no permanent fence.
- An Immediate consumer whose `start()` returns Ok resolves the watcher
  without action; a loop-style consumer (start blocks until cancellation)
  starts with zero added latency (no grace wait anywhere in the actor path).
- The controller actor is never delayed by the Immediate handshake: the
  ControlBus reentrancy regression (timer emits during start driving
  StopRoute against a Registered sibling) is covered by a deterministic
  regression test, looped 20×.
- Explicit consumers (bind-failure path) behave exactly as today.
- `cargo test -p camel-core --lib` green; `cargo test -p camel-test --test
  controlbus_test controlbus_stops_route -- --exact` green 20/20; clippy/fmt
  clean.

## Risk budget

Medium. The first design (synchronous latch await in the actor path) was
REJECTED by the final holistic blessing (e_opus): holding the controller
actor through the grace let a timer's immediate first tick reenter the
RuntimeBus with StopRoute against a sibling whose two-phase start had not
committed (`invalid transition: Registered -> Stopped`), regressing
controlbus_stops_route from 3/3 to ~4-5/10. The async-watcher design
(e_gpt ruling) restores the pre-fix actor timing — the reentrancy window
cannot open — while eliminating the silence: prompt errors surface as
`Failed` route status within ~grace (matching existing supervision
semantics where consumer failures become FailRoute from any state).
Accepted residual: an Immediate `start()` error arriving AFTER the grace
budget keeps today's behavior (error log + CrashNotification); Explicit
consumers that emit after mark_ready while the
actor is held have a latent reentrancy exposure (filed as follow-up).
