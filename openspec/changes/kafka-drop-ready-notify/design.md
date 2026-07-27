# Design: kafka-drop-ready-notify

## Approach

Mechanical deletion of a dead test-synchronization path. The Kafka consumer
already participates in the unified startup handshake (`ConsumerStartupMode::Explicit`
+ `ctx.mark_ready()` after `subscribe()`); the `ReadyContext`/`Notify` machinery
is a legacy artifact that never reached a real consumer.

**Concrete edits to `crates/components/camel-kafka/src/consumer.rs`:**

1. **Import (L21):** `use tokio::sync::{Notify, mpsc};` → `use tokio::sync::mpsc;`
2. **Module doc (L31–47):** delete the `ReadyContext` doc comment block.
3. **`ReadyContext` struct + impls (L48–61):** delete `struct ReadyContext`,
   `impl ClientContext for ReadyContext {}`, and the `RdConsumerContext` impl
   (whose only non-default method, `post_rebalance`, fires the doomed `Notify`).
4. **Type alias (L63):** delete `type ReadyStreamConsumer = StreamConsumer<ReadyContext>;`.
5. **`KafkaConsumer` struct (L70):** delete the `ready: Arc<Notify>` field and
   its doc comment (L69).
6. **`KafkaConsumer::new` (L88):** delete `ready: Arc::new(Notify::new()),`.
7. **`KafkaConsumer::ready_signal` (L94–98):** delete the method.
8. **`KafkaConsumer::start` (L118, L129):** delete `let ready = self.ready.clone();`
   and remove `ready,` from the `run_consumer_loop(...)` argument list.
9. **`run_consumer_loop` signature (L453):** drop the `ready: Arc<Notify>,` param.
10. **Consumer creation (L490–494):** `client_cfg.create_with_context(ReadyContext { ready })`
    typed as `ReadyStreamConsumer` → `client_cfg.create()` typed as `StreamConsumer`
    (rdkafka's `DefaultConsumerContext`).
11. **Startup-readiness comment (L530–531):** rewrite to drop the `ready` Notify
    reference; keep the liveness/ordering rationale (it still explains *why*
    `mark_ready()` is eager, not gated on assignment).
12. **Second ReadyContext comment (L587–589):** rewrite to remove the
    `ReadyContext::post_rebalance` / `ready.notify_waiters()` reference. Keep the
    accurate part ("recv() drives the rebalance protocol automatically") but
    reword so it no longer names the deleted context.
13. **Test (L1138–1143):** delete `test_ready_signal_returns_shared_notify_handle`.

After deletion, `Subscribe`/poll/commit/shutdown logic is byte-identical — only
the orphaned `Notify` plumbing and its lone test vanish.

## Affected crates

- **camel-component-kafka** (`crates/components/camel-kafka/src/consumer.rs`):
  the only modified file. ~55 lines removed/edited (import, ReadyContext struct
  + 2 impls + alias, KafkaConsumer field + new() init + accessor, start()
  threading, run_consumer_loop signature + consumer creation, two comment
  blocks, one test). Net negative diff. Source-breaking removal of the
  `pub fn ready_signal()` accessor (zero in-repo callers; superseded by
  `ConsumerContext::startup_signal()` from rc-gu5n).

## Architecture boundaries

This change is entirely within the **Components** layer
(`camel-component-kafka`), consumer-internal. It touches:

- Neither the **Runtime** (no change to `spawn_consumer_task`, the
  `ConsumerStartupMode` enum, or `await_consumer_startup`).
- Nor the **DSL/processor/services/languages/functions** layers.

The data/control-plane boundary is respected: the removed `Notify` lived on the
control plane (test sync) and was never wired to data-plane exchange flow. The
canonical control-plane readiness signal — `ctx.mark_ready()` →
`StartupSignal::mark_ready()` → `StartupReceiver` resolve — is untouched.

## Relevant ADRs (per CONTEXT-MAP.md)

- **ADR-0012** (b-prime metrics): no error/warn sites touched; the removed code
  had no metric calls.
- **Consumer startup handshake** (rc-gu5n, rc-w1u9): this change *completes*
  that unification for Kafka by removing the last pre-unification hook. The
  `startup_mode() -> Explicit` contract and the `mark_ready()` call site are
  preserved verbatim.
- **`crates/components/camel-kafka/CONTEXT.md`** "Crash health ownership": the
  consumer's `runtime` field is metrics-only; this change does not add any
  `force_unhealthy_for_route` call. No CONTEXT.md update required (the doc does
  not mention `ReadyContext` or `ready_signal`).

## Alternatives considered

1. **Keep `ReadyContext` but rip out the `Notify`.** Rejected — `ReadyContext`
   exists *only* to host the `post_rebalance` Notify. With the Notify gone, the
   struct is an empty `ClientContext`/`RdConsumerContext` impl, equivalent to
   rdkafka's `DefaultConsumerContext`. Keeping a vestigial empty context adds
   noise for no benefit.
2. **Repurpose `ReadyContext::post_rebalance` to call `ctx.mark_ready()`.**
   Rejected — this is exactly the gating-on-assignment pattern that the existing
   L517–528 comment documents as a startup deadlock (broker drop during
   coordination; self-producing route graph). The eager `mark_ready()` after
   `subscribe()` is the deliberate, correct design.
3. **Migrate the lone test to `StartupSignal` instead of deleting it.** Rejected
   — the test asserted `ready_signal()` returns a shared `Arc<Notify>`. With the
   accessor gone, there is nothing to assert. The `StartupSignal` API is already
   covered by `camel-component-api/src/consumer.rs` tests
   (`test_startup_signal_*`, `test_consumer_context_mark_ready_drives_signal`).
