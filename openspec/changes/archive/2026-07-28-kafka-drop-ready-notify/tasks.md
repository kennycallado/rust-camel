# Tasks: kafka-drop-ready-notify

## camel-component-kafka

### Task 1.1: Remove ReadyContext/Notify redundancy from Kafka consumer

Delete the legacy `ReadyContext` + `Arc<Notify>` test-synchronization path from
`KafkaConsumer`, unifying readiness on the existing `ConsumerStartupMode::Explicit`
+ `ctx.mark_ready()` handshake (rc-gu5n). All edits are interdependent — the file
will not compile with a partial removal — so this is one atomic task.

**Files:**
- `crates/components/camel-kafka/src/consumer.rs` (modified — only file)

**Steps:**
1. **Import — drop Notify (L21):** change `use tokio::sync::{Notify, mpsc};` to
   `use tokio::sync::mpsc;` (drop `Notify`).
2. **Import — drop stranded rdkafka imports (L7, L9–11):** deleting the
   `ReadyContext` impls (Step 4) removes the ONLY uses of three symbols.
   - L7: delete the entire line `use rdkafka::client::ClientContext;`.
   - L9–11: in the `use rdkafka::consumer::{ ... }` block, remove
     `ConsumerContext as RdConsumerContext,` and `Rebalance,`. KEEP
     `Consumer as RdConsumer,` (still used by `.subscribe()` at L499) and
     `StreamConsumer,` (used in the new consumer creation).
3. **Module doc (L31–47):** delete the entire `ReadyContext` doc-comment block
   (the comment that starts `// ---- ReadyContext — notifies when...`).
4. **ReadyContext struct + impls (L48–61):** delete `struct ReadyContext`,
   `impl ClientContext for ReadyContext {}`, and the
   `impl RdConsumerContext for ReadyContext { fn post_rebalance ... }` block.
5. **Type alias (L63):** delete `type ReadyStreamConsumer = StreamConsumer<ReadyContext>;`.
6. **KafkaCommitClient doc + impl retarget (L300–301, L311):** the
   `ReadyStreamConsumer` alias has THREE use sites beyond L63/L490. Retarget
   both to plain `StreamConsumer`:
   - L301: `/// real Kafka broker. The production path passes a &ReadyStreamConsumer,`
     → `&StreamConsumer,`
   - L311: `impl KafkaCommitClient for ReadyStreamConsumer {`
     → `impl KafkaCommitClient for StreamConsumer {`
   (The `.commit()` body at L317 uses fully-qualified
   `rdkafka::consumer::Consumer::commit` and is unaffected.)
7. **KafkaConsumer struct field (L69–70):** delete the doc comment
   `/// Notified once the consumer has received its first partition assignment.`
   and the `ready: Arc<Notify>,` field.
8. **KafkaConsumer::new (L88):** delete `ready: Arc::new(Notify::new()),`.
9. **KafkaConsumer::ready_signal (L94–98):** delete the entire method (doc
   comment + `pub fn ready_signal(&self) -> Arc<Notify> { self.ready.clone() }`).
10. **KafkaConsumer::start (L118):** delete `let ready = self.ready.clone();`.
11. **KafkaConsumer::start (~L133):** in the `tokio::spawn(run_consumer_loop(...))`
    call's argument list, remove the `ready,` argument (keep `config, ctx,
    cancel_token, runtime, route_id`). Note: the `ready,` arg is ~L133, a few
    lines below the `tokio::spawn(` line at L129.
12. **run_consumer_loop signature (L449–L456):** remove the
    `ready: Arc<Notify>,` parameter from the function signature.
13. **Consumer creation (L490–L494):** replace
    ```
    let consumer: ReadyStreamConsumer = client_cfg
        .create_with_context(ReadyContext { ready })
        .map_err(|e| {
            CamelError::ProcessorError(format!("Failed to create Kafka consumer: {}", e))
        })?;
    ```
    with
    ```
    let consumer: StreamConsumer = client_cfg
        .create()
        .map_err(|e| {
            CamelError::ProcessorError(format!("Failed to create Kafka consumer: {}", e))
        })?;
    ```
    (`StreamConsumer` here is `rdkafka::consumer::StreamConsumer` — already
    imported at L9–11. It defaults to `DefaultConsumerContext`.)
14. **Startup-readiness comment — first ReadyContext ref (L517–518):** rewrite
    ```
    // Readiness MUST NOT be gated on the first partition assignment (the old
    // `ReadyContext::post_rebalance` behaviour). That is unsafe for two
    ```
    to drop the `ReadyContext::post_rebalance` name:
    ```
    // Readiness MUST NOT be gated on the first partition assignment (the old
    // post_rebalance-gated behaviour). That is unsafe for two
    ```
15. **Startup-readiness comment — second ReadyContext ref (L530–531):** delete
    the two lines
    ```
    // The `ready` Notify (fired from `ReadyContext::post_rebalance`) remains
    // available for test synchronisation via `KafkaConsumer::ready_signal`.
    ```
    Keep the rest of the L508–L534 comment block (liveness/ordering rationale).
16. **Poll-loop ReadyContext comment (L587–589):** rewrite the three-line comment
    ```
    // The ReadyContext::post_rebalance callback fires `ready.notify_waiters()`
    // when partitions are assigned. No polling loop needed — recv() drives the
    // rebalance protocol automatically.
    ```
    to a single line that does NOT reference `ReadyContext` or `ready`:
    ```
    // recv() drives the rebalance protocol automatically — no polling loop needed.
    ```
17. **Test (~L1138–1144):** delete the entire
    `test_ready_signal_returns_shared_notify_handle` test function (the `#[test]`
    attribute, the `fn` signature, the body, and the closing brace — through
    ~L1144). Confirm no other test in the file references `ready_signal` or the
    `ready` field.

**Tests:** (executable spec — name, arrange, act, assert)
- `grep_verification_no_ready_residue`: arrange = post-edit source tree; act = run
  `rg 'ReadyContext|ReadyStreamConsumer|ready_signal|tokio::sync::Notify|Arc<Notify>' crates/components/camel-kafka/`;
  assert = **zero matches** (all redundant symbols gone from the kafka
  component; `mpsc` remains).
- `build_crate`: arrange = post-edit source; act =
  `cargo build -p camel-component-kafka`; assert = exit code 0, zero warnings.
- `clippy_strict`: arrange = post-edit source; act =
  `cargo clippy -p camel-component-kafka --all-targets -- -D warnings`; assert =
  exit code 0, zero warnings.
- `fmt_check`: arrange = post-edit source; act =
  `cargo fmt --check -p camel-component-kafka`; assert = exit code 0 (no diff).
- `lib_tests_pass_without_removed_test`: arrange = post-edit source with
  `test_ready_signal_returns_shared_notify_handle` deleted; act =
  `cargo test -p camel-component-kafka --lib`; assert = all tests pass, test
  count drops by exactly 1 (the removed test), zero failures.
- `startup_mode_unchanged`: arrange = read the post-edit
  `startup_mode()` fn; act = inspect its return expression; assert = still
  `ConsumerStartupMode::Explicit` (the unified path is preserved verbatim).
- `mark_ready_call_preserved`: arrange = read `run_consumer_loop` post-edit;
  act = locate `ctx.mark_ready()`; assert = the call is still present, still
  positioned after `subscribe()` and before the poll loop, unchanged.

**Acceptance:**
- `rg 'ReadyContext|ReadyStreamConsumer|ready_signal|Notify' crates/components/camel-kafka/`
  returns no hits.
- `cargo build -p camel-component-kafka` succeeds with zero warnings.
- `cargo clippy -p camel-component-kafka --all-targets -- -D warnings` passes.
- `cargo fmt --check -p camel-component-kafka` passes.
- `cargo test -p camel-component-kafka --lib` passes (all remaining tests).
- `startup_mode()` still returns `ConsumerStartupMode::Explicit`.
- `ctx.mark_ready()` call site unchanged.
- `StreamConsumer` (default context) is used; no `create_with_context`.

**Notes for the worker:**
- This is a single-file, purely subtractive refactor. Do NOT touch any other
  crate, the poll loop, commit handling, shutdown sequencing, or the
  `startup_mode()` / `background_task_handle()` impls.
- The file is ~1807 lines; the edits span L7, L9–11, L21, L31–63, L69–70, L88,
  L94–98, L118, ~L133, L300–301, L311, L449–456, L490–494, L517–518, L530–531,
  L587–589, ~L1138–1144. Line numbers will shift as you delete upward edits —
  work top-to-bottom, or use Edit with unique surrounding context rather than
  line numbers. There are THREE comment blocks referencing `ReadyContext`
  (L517–518, L530–531, L587–589) — all must be cleaned.
- Before reporting back: run `cargo fmt --check -p camel-component-kafka` and
  `cargo clippy -p camel-component-kafka --all-targets -- -D warnings`. Fix any
  issues. Run `cargo test -p camel-component-kafka --lib` and confirm green.

- [x] 1.1
