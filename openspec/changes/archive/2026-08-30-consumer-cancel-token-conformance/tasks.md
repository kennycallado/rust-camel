# Tasks: consumer-cancel-token-conformance

Single-phase change. Five independent tasks, one per consumer. Each task
is independently dispatchable.

NOTE for workers: cargo is not on PATH. Run cargo through the nix
wrapper from the worktree root:
`nix develop /home/kenny/dev/rust-camel/.worktrees/consumer-cancel-token-conformance -c cargo <args>`

## camel-component-kafka

### Task 1.1: Kafka consumer uses the context token

**Files:**
- `crates/components/camel-kafka/src/consumer.rs` (modified)

**Steps:**
1. In `KafkaConsumer::start` (consumer.rs:65), replace
   `let cancel_token = CancellationToken::new();` with
   `let cancel_token = ctx.cancel_token();`. Keep the double-start guard
   above it untouched. `ctx` is still available at this point (it is
   moved into the spawn later).
2. Confirm the event loop (consumer.rs:575-576) still selects on
   `cancel_token.cancelled()`; no loop change is needed.
3. Add the test below to the inline `mod tests` (helpers
   `make_resolved_config` at :768 and `test_rt` at :679 exist; mirror
   the `ConsumerContext::new` pattern at :909-910).

**Tests:** (executable spec)
- `consumer_task_exits_on_context_token_cancel`: setup —
  `make_resolved_config()` (broker `localhost:9092`, unreachable),
  `KafkaConsumer::new(config, test_rt())`, an `mpsc::channel(16)`, a
  `CancellationToken`, and
  `ConsumerContext::new(tx, token.clone(), "test-route".to_string())`;
  action — `consumer.start(ctx).await` returns `Ok`, then
  `token.cancel()`, then
  `let handle = consumer.background_task_handle().expect("handle")`, then
  `tokio::time::timeout(Duration::from_secs(2), handle).await`, then
  `tokio::time::timeout(Duration::from_secs(2), consumer.stop()).await`;
  assert — the handle timeout does NOT elapse and the join yields
  `Ok(Ok(()))` (loop observed the context token; no abort), and `stop()`
  returns `Ok` (token already cancelled, handle already taken).
  command —
  `cargo test -p camel-component-kafka --lib consumer_task_exits_on_context_token_cancel`;
  expected — fails before the fix (handle timeout elapses, task is deaf),
  passes after.

**Acceptance:**
- `cargo test -p camel-component-kafka --lib` exits 0.
- `rg -n "CancellationToken::new()" crates/components/camel-kafka/src/consumer.rs`
  shows hits only inside `mod tests`.
- `cargo clippy -p camel-component-kafka --all-targets -- -D warnings`
  exits 0.

- [x] 1.1

## camel-component-jms

### Task 1.2: JMS consumer uses the context token

**Files:**
- `crates/components/camel-jms/src/consumer.rs` (modified)

**Steps:**
1. In `JmsConsumer::start` (consumer.rs:446), replace
   `let cancel = CancellationToken::new();` with
   `let cancel = ctx.cancel_token();`. `ctx` is cloned into each spawned
   task at :459, so take the token before the closure, as the current
   code does.
2. Add the contract test below to the inline `mod tests`. It mirrors the
   injected-handle harness at :694-703. Real `start()` cannot run
   offline: the pre-flight at :419-437 fails without a bridge binary.
   The test pins the post-fix contract shape (stored token is the
   context token; tasks await it; stop joins fast).

**Tests:** (executable spec)
- `stop_joins_tasks_awaiting_context_token`: setup — build
  `JmsConsumer` as `stop_absorbs_consumer_task_panic` does
  (consumer.rs:717-733: `JmsBridgePool::from_config` with
  `JmsPoolConfig::single_broker("tcp://localhost:61616", BrokerType::Generic)`,
  `JmsEndpointConfig::from_uri("jms:queue:test")`, `jms_reconnect_default()`,
  `rt()`); create a `CancellationToken` `t` and
  `ConsumerContext::new(tx, t.clone(), "test-route".to_string())`;
  set `consumer.cancel_token = Some(ctx.cancel_token())` and
  `consumer.task_handles = vec![tokio::spawn({ let t = t.clone(); async move { t.cancelled().await; Ok(()) } })]`;
  action — `consumer.stop().await` under
  `tokio::time::timeout(Duration::from_secs(1), ...)`;
  assert — stop completes within 1 s and returns `Ok`, and
  `consumer.task_handles.is_empty()`.
  command —
  `cargo test -p camel-component-jms --lib stop_joins_tasks_awaiting_context_token`;
  expected — passes with the fix applied (contract pin; the one-line
  production change is verified by review).

**Acceptance:**
- `cargo test -p camel-component-jms --lib` exits 0.
- `rg -n "CancellationToken::new()" crates/components/camel-jms/src/consumer.rs`
  shows hits only inside `mod tests`.
- `cargo clippy -p camel-component-jms --all-targets -- -D warnings`
  exits 0.

- [x] 1.2

## camel-component-cxf

### Task 1.3: CXF consumer uses the context token

**Files:**
- `crates/components/camel-cxf/src/consumer.rs` (modified)
- `crates/components/camel-cxf/tests/consumer_unit_test.rs` (modified)

**Steps:**
1. In `CxfConsumer::start` (consumer.rs:151), replace
   `let cancel = CancellationToken::new();` with
   `let cancel = ctx.cancel_token();`.
2. Add the integration test below to `tests/consumer_unit_test.rs`,
   mirroring the pool setup of `tests/pool_lifecycle_test.rs:94-101`:
   `CxfBridgePool::from_config(CxfPoolConfig::default())` (adjust to the
   exact default the lifecycle test uses), then
   `BridgeSlot::new_ready_for_test(channel)` (pool.rs:76), then
   `pool.insert_slot_for_test(CxfBridgePool::slot_key(), slot)`
   (pool.rs:150). The slot MUST sit at `CxfBridgePool::slot_key()` so
   the start() pre-flight (consumer.rs:158-163) finds it Ready and never
   spawns a real bridge. `spawn_mock_bridge`
   (tests/support/mock_bridge.rs:126) provides the tonic channel.
3. Use `camel_component_api::NoopRuntimeObservability` (test-support
   feature; already a dev-dependency of camel-cxf) for the runtime arg
   of `CxfConsumer::new(pool, "test".to_string(), ...)`.
4. Import `CxfConsumer` from its pub path in `camel_component_cxf`
   (check `src/lib.rs`), and `Consumer`/`ConsumerContext` from
   `camel_component_api::consumer`.

**Tests:** (executable spec)
- `consumer_task_exits_on_context_token_cancel`: setup —
  `let (port, _state) = spawn_mock_bridge().await`, connect a tonic
  `Channel` to `http://127.0.0.1:{port}`, build the pool with a Ready
  slot at `CxfBridgePool::slot_key()` per step 2,
  `let mut consumer = CxfConsumer::new(pool, "test".to_string(), rt)`,
  an `mpsc::channel(16)`, a `CancellationToken`, and
  `ConsumerContext::new(tx, token.clone(), "test-route".to_string())`;
  action — `consumer.start(ctx).await` returns `Ok`, `token.cancel()`,
  `let handle = consumer.background_task_handle().expect("handle")`,
  `tokio::time::timeout(Duration::from_secs(2), handle).await`;
  assert — the timeout does NOT elapse (the consumer task observed the
  context token and broke its select).
  command —
  `cargo test -p camel-component-cxf --test consumer_unit_test consumer_task_exits_on_context_token_cancel`;
  expected — passes before AND after the fix (conformance pin: the loop
  already dual-selects `ctx.cancelled()` at consumer.rs:194-202; the
  production one-line change is verified by the rg acceptance below).

**Acceptance:**
- `cargo test -p camel-component-cxf --test consumer_unit_test` exits 0.
- `rg -n "CancellationToken::new()" crates/components/camel-cxf/src/consumer.rs`
  shows hits only inside `mod tests`.
- `cargo clippy -p camel-component-cxf --all-targets -- -D warnings`
  exits 0.

- [x] 1.3

## camel-component-wasm

### Task 1.4: Wasm source consumer overwrites its token in start

**Files:**
- `crates/components/camel-component-wasm/src/source_consumer.rs` (modified)
- `crates/components/camel-component-wasm/tests/source_integration.rs` (modified)

**Steps:**
1. In `WasmSourceConsumer::start` (source_consumer.rs:100), add as the
   FIRST statement:
   `self.cancel_token = ctx.cancel_token();`.
   This overwrites the token created in `new()` (:78) before it is
   cloned into `SourceHostState` at :129.
2. Leave `new()` unchanged: its token only fills the field until
   `start()` supplies the Runtime token.
3. Build the test guest once:
   `nix develop /home/kenny/dev/rust-camel/.worktrees/consumer-cancel-token-conformance -c cargo build --manifest-path examples/wasm-source-webhook/guest/Cargo.toml --target wasm32-wasip2`
   (the wasm32-wasip2 std ships in the devshell).
4. Add the integration test below to `tests/source_integration.rs`,
   reusing `make_ctx` (the `make_consumer_context` helper at :94-106,
   which returns ctx + rx + cancel token) and `make_consumer` (:113).
   Mark it `#[ignore = "requires pre-built guest wasm (see module docs)"]`
   like every other test in this file; run it explicitly with
   `-- --ignored` after building the guest.

**Tests:** (executable spec)
- `source_task_exits_on_context_token_cancel`: setup —
  `let port = free_port().await` (unique port; the guest defaults to
  127.0.0.1:8080 and would clash or fail), build the consumer with
  `make_consumer(vec![("bind".into(), format!("127.0.0.1:{port}")),
  ("path".into(), "/webhook".into())])` mirroring
  `test_source_lifecycle_start_stop` (source_integration.rs:202-208),
  `let (ctx, _rx, token) = make_consumer_context("cancel-route", 16)`;
  action — `consumer.start(ctx).await` returns `Ok`, `token.cancel()`,
  `let handle = consumer.background_task_handle().expect("run task")`,
  `tokio::time::timeout(Duration::from_secs(3), handle).await`;
  assert — the timeout does NOT elapse (host-import selects and the
   epoch tripwire observed the context token).
  command —
  `cargo test -p camel-component-wasm --test source_integration source_task_exits_on_context_token_cancel -- --ignored`
  (after the guest build in step 3);
  expected — fails before the fix (run task deaf to the context
  token), passes after.

**Acceptance:**
- `cargo test -p camel-component-wasm --test source_integration` exits 0
  (ignored tests skip), and the new test passes under `-- --ignored`
  after the guest build.
- `rg -n "self.cancel_token = ctx.cancel_token" crates/components/camel-component-wasm/src/source_consumer.rs`
  exits 0 with at least one match.
- `cargo clippy -p camel-component-wasm --all-targets -- -D warnings`
  exits 0.

- [x] 1.4

## camel-component-redis

### Task 1.5: Redis consumer links a child of the context token

**Files:**
- `crates/components/camel-redis/src/consumer.rs` (modified)

**Steps:**
1. In `RedisConsumer::start` (consumer.rs:153), replace
   `let cancel_token = CancellationToken::new();` with
   `let cancel_token = ctx.cancel_token().child_token();`.
   Rationale: `stop()` at :205-207 cancels the stored token; with a
   plain clone a local stop would cancel the Runtime-owned token that
   all clones share. A child token keeps local stops local, while the
   parent cascade from route stop still reaches the loops.
2. Handle the re-start path at :148-153: the old token is cancelled and
   replaced on re-start; with child tokens each start links a fresh
   child of the current context token. No extra change needed, but
   verify the cleanup block still compiles with `Option` semantics.
3. Add the test below to the inline `mod tests` (helpers
   `create_test_config` and `test_rt` exist; mirror
   `test_consumer_start_sets_task_handle` at :757-773).

**Tests:** (executable spec)
- `consumer_task_exits_on_context_token_cancel`: setup —
  `create_test_config(RedisCommand::Blpop)` (Redis at 127.0.0.1 not
  required: the session enters its reconnect loop),
  `RedisConsumer::new(config, test_rt())`, an `mpsc::channel(16)`, a
  `CancellationToken`, and
  `ConsumerContext::new(tx, token.clone(), "redis-test-route".to_string())`;
  action — `consumer.start(ctx).await` returns `Ok`, `token.cancel()`,
  `let handle = consumer.background_task_handle().expect("handle")`,
  `tokio::time::timeout(Duration::from_secs(2), handle).await`;
  assert — the timeout does NOT elapse and the join yields
  `Ok(Ok(()))` (parent cascade crossed the child link; the queue
  session observed cancellation).
  command —
  `cargo test -p camel-component-redis --lib consumer_task_exits_on_context_token_cancel`;
  expected — fails before the fix (session deaf to the context
  token), passes after.
- `local_stop_does_not_cancel_runtime_token`: setup — same
  construction as above, started; action — call `consumer.stop().await`
  while holding the original context token clone;
  assert — stop returns `Ok` AND `token.is_cancelled() == false` (the
  child link kept the local stop local).
  command —
  `cargo test -p camel-component-redis --lib local_stop_does_not_cancel_runtime_token`;
  expected — passes after the fix (before the fix the stored token is
  unrelated, so this is a contract pin for the child-link design).

**Acceptance:**
- `cargo test -p camel-component-redis --lib` exits 0.
- `rg -n "CancellationToken::new()" crates/components/camel-redis/src/consumer.rs`
  shows hits only inside `mod tests` (production `new()` in
  `RedisConsumer::new` does not create a token; verify and leave test
  hits only).
- `cargo clippy -p camel-component-redis --all-targets -- -D warnings`
  exits 0.

- [x] 1.5
