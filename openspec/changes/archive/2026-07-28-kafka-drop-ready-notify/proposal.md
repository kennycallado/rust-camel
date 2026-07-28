# Proposal: kafka-drop-ready-notify

## Why

The Kafka consumer (`crates/components/camel-kafka/src/consumer.rs`) carries a
dual source of truth for readiness. After rc-gu5n unified binding consumers to
`ConsumerStartupMode::Explicit`, the Kafka consumer signals startup readiness
via `ctx.mark_ready()` (fired eagerly after `subscribe()` in
`run_consumer_loop`). Alongside that unified path, a **legacy
test-synchronization hook** still exists:

- `struct ReadyContext` — a custom `rdkafka` `ConsumerContext` that fires an
  `Arc<Notify>` from `post_rebalance` when partitions are assigned.
- `KafkaConsumer::ready_signal()` — exposes that `Arc<Notify>` for tests.
- `KafkaConsumer::ready: Arc<Notify>` field threaded from `new()` → `start()`
  → `run_consumer_loop` → `ReadyContext`.

A repo-wide grep confirms `ready_signal()` has **zero external callers**; its
only consumer is the unit test `test_ready_signal_returns_shared_notify_handle`,
which merely asserts two calls return the same `Arc`. The `Notify` is never
`.notified().await`'d by any test.

This is benign today (the two paths signal different events: startup vs.
partition assignment), but it is dead code that invites a future heisenbug:
someone wires `ready.notified()` into a test, that test waits for partition
assignment, and it hangs under broker-drop or self-producing-route-graph
conditions — exactly the deadlocks documented in the consumer.rs "Startup
readiness" note (L508–L534). e_opus verdict on rc-5992: **SHOULD-FIX-SOON**.

## What Changes

**Included:**
- Delete `ReadyContext` struct + its `ClientContext`/`RdConsumerContext` impls.
- Delete `type ReadyStreamConsumer` alias; use plain `StreamConsumer` (rdkafka
  default context).
- Delete `KafkaConsumer::ready` field, its initialization in `new()`, and the
  `ready_signal()` accessor.
- Remove the `ready: Arc<Notify>` parameter from `run_consumer_loop` and its
  call site in `start()`.
- Remove `use tokio::sync::Notify` (keep `mpsc`).
- Delete `test_ready_signal_returns_shared_notify_handle`.
- Update the "Startup readiness" comment block (L508–L534) to drop the `ready`
  Notify reference.
- Delete/rewrite the second dangling `ReadyContext::post_rebalance` comment at
  L587–L589 (it describes the now-removed `ready.notify_waiters()` firing); the
  surrounding "recv() drives the rebalance protocol automatically" note is still
  accurate and stays, but must not reference `ReadyContext`.

**Excluded:**
- The `ctx.mark_ready()` call at L535 — **stays**. This is the unified
  `ConsumerStartupMode::Explicit` path and the controller's await target.
- Any change to `startup_mode()`, `background_task_handle()`, the poll loop,
  commit handling, or shutdown sequencing.
- No new public API. `StartupSignal` (rc-gu5n) already provides the canonical
  test-synchronization surface via `ConsumerContext::startup_signal()`.
- rc-x7t2 (pre-existing `cargo fmt` violation in `scripts/xtask/src/main.rs`)
  is tracked separately and explicitly out of scope.

## Acceptance criteria

- `crates/components/camel-kafka` builds clean: `cargo build -p
  camel-component-kafka`.
- `cargo clippy -p camel-component-kafka --all-targets -- -D warnings` passes.
- `cargo test -p camel-component-kafka --lib` passes (the removed test is gone;
  no other test referenced `ready_signal`).
- `cargo fmt --check -p camel-component-kafka` passes.
- `rg 'ReadyContext|ready_signal|Notify' crates/components/camel-kafka/`
  returns no hits.
- No change to runtime behaviour: `startup_mode()` still returns `Explicit`,
  `ctx.mark_ready()` still fires after `subscribe()`.
- The data/control-plane boundary is unchanged (consumer-internal refactor).

## Risk budget

- **Behavioural risk: LOW.** The removed `Notify` was never awaited in-repo,
  so in-repo startup behaviour is unchanged: `startup_mode()` still returns
  `Explicit` and `ctx.mark_ready()` still fires eagerly after `subscribe()`.
  The removed `ready_signal()` *did* expose a partition-assignment signal that
  downstream consumers (outside this repo) could theoretically have observed;
  removing it eliminates that observable. No such downstream consumer is known.
- **Test risk: LOW.** One trivial unit test removed. Integration tests requiring
  a live Kafka broker are `#[ignore]`'d and unchanged (they never used
  `ready_signal`).
- **API risk: LOW (source-breaking).** `ready_signal()` is `pub` and its removal
  is a source-breaking change for any downstream crate that references it,
  despite zero in-repo callers. Given the function is undocumented, untested
  beyond the handle-identity check, and the unified `StartupSignal` surface
  (rc-gu5n) supersedes it, this is an acceptable breaking cleanup. No
  downstream crate in this workspace is affected.
- Out of bounds: touching any other consumer, the `StartupSignal` API, or the
  `spawn_consumer_task` defensive fallback.
