# Tasks: audit-fix-xj-block-in-place

## camel-xj

### Task 1.1: Add OffloadRuntime and rewrite block_on_result

**Files:**
- `crates/components/camel-xj/src/component.rs` (modified)

**Steps:**
1. Add an `OffloadRuntime` struct near the top of `component.rs` (after the
   imports, before `XjComponentConfig`). It wraps an
   `Option<tokio::runtime::Runtime>` built with `new_multi_thread().worker_threads(1)`
   and `thread_name("xj-offload")`. Derive `Debug`. The `Option` wrapping is
   required so the custom `Drop` impl (step 4b) can `take()` the runtime and
   move it to a scoped thread for safe teardown inside async contexts.
2. Implement `OffloadRuntime::new()` — constructs the runtime via
   `tokio::runtime::Builder::new_multi_thread().worker_threads(1).enable_all()
   .thread_name("xj-offload").build()`. Use `expect("xj offload runtime")`
   — construction-time infallibility (same justification as the multi-thread
   builder in `crates/camel-test/tests/support/mod.rs:31`). Store as
   `Some(runtime)`.
3. Implement `OffloadRuntime::handle(&self) -> &tokio::runtime::Handle` —
   returns `self.runtime.as_ref().expect("offload runtime dropped").handle()`.
4. Implement `OffloadRuntime::block_on<F, T>(&self, fut: F) -> T where
   F: Future<Output = T> + Send, T: Send` — uses `std::thread::scope(|s| {
   s.spawn(move || runtime.block_on(fut)).join() })`. The `Send` bounds are
   required by `thread::scope::spawn`. The scoped thread has no ambient
   runtime so `Runtime::block_on` does not panic. The future is moved into
   the scoped thread closure. Convert a join panic by unwrapping with
   `.expect("xj offload thread panicked")`.
4b. Implement `impl Drop for OffloadRuntime` — moves the `Runtime` onto a
    scoped OS thread before dropping: `if let Some(rt) = self.runtime.take()
    { std::thread::scope(|s| { s.spawn(|| drop(rt)); }); }`. This prevents
    the "Cannot drop a runtime in a context where blocking is not allowed"
    panic that occurs when `XjComponent` is dropped inside an async context
    (production shutdown path: context registry drops `Arc<dyn Component>`
    at the end of `async fn run`).
5. Add `offload: Arc<OffloadRuntime>` field to `XjComponent` (after `client`).
6. Update `XjComponent::new()` — construct `Arc::new(OffloadRuntime::new())`
   and store it.
7. Update `XjComponent::with_client_for_testing()` — construct
   `Arc::new(OffloadRuntime::new())` and store it.
8. Rewrite `block_on_result` (currently lines 362-381) to the hybrid approach.
   The `F` bound gains `+ Send` and `T` gains `+ Send` because the offload
   path requires them (the future is moved into a scoped thread):
   ```
   fn block_on_result<F, T>(&self, fut: F) -> Result<T, CamelError>
   where
       F: Future<Output = Result<T, XjError>> + Send,
       T: Send,
   {
       if let Ok(handle) = tokio::runtime::Handle::try_current()
           && handle.runtime_flavor() == RuntimeFlavor::MultiThread
       {
           tokio::task::block_in_place(|| {
               handle.block_on(fut)
                   .map_err(|e| CamelError::EndpointCreationFailed(e.to_string()))
           })
       } else {
           self.offload.block_on(fut)
               .map_err(|e| CamelError::EndpointCreationFailed(e.to_string()))
       }
   }
   ```
   Remove the ephemeral-runtime `else` branch entirely.
9. Rewrite the `block_on_result` doc comment (currently lines 352-361) to
   describe the hybrid approach: `block_in_place` on multi-thread runtime,
   offload runtime (scoped thread + dedicated Tokio runtime) for current-thread
   and no-runtime cases. Note that the offload runtime lives for the Component's
   lifetime so tonic Channel dispatch tasks spawned during bridge startup have
   a stable host. Delete the `XJ-014` code annotation (tracked separately).
10. Add `use tokio::runtime::RuntimeFlavor;` to the imports if not already
    present.
11. Add a test-only `pub(crate) fn offload_weak(&self) -> std::sync::Weak<OffloadRuntime>`
    on `XjComponent` to allow the drop-cleanup test to observe release.

**Tests:** (executable spec — name, setup, action, assert)

- `offload_runtime_runs_simple_future`:
  - setup: `let offload = OffloadRuntime::new();`
  - action: `let result = offload.block_on(async { 42i32 });`
  - assert: `assert_eq!(result, 42);`
  - command: `cargo test -p camel-xj --lib offload_runtime_runs_simple_future`
  - expected: pass after implementation

- `offload_runtime_spawned_task_survives_after_block_on`:
  - setup: Create an `OffloadRuntime`. Spawn a long-lived task on it via
    `offload.handle().spawn(async move { tx.send(()).await })` that sends a
    value through a `tokio::sync::oneshot::channel`. This task models the tonic
    Channel dispatch task — it is spawned on the offload runtime during
    `ensure_bridge_started` and must survive past `block_on` returning.
  - action: Call `offload.block_on(async { () })` (blocks and returns on a
    scoped thread). After it returns, call `offload.block_on(recv)` to receive
    the value.
  - assert: The value is received successfully, proving tasks spawned on the
    offload runtime survive past the `block_on` call boundary. This is the
    unit-level regression for the dead-Channel defect: if the runtime were
    ephemeral (dropped after block_on), the spawned task would be cancelled
    and the receive would fail.
  - command: `cargo test -p camel-xj --lib offload_runtime_spawned_task_survives_after_block_on`
  - expected: pass after implementation

- `block_on_result_works_on_current_thread_runtime`:
  - setup: `#[tokio::test(flavor = "current_thread")]`. Construct
    `XjComponent::default()`.
  - action: Call `component.block_on_result(async { Ok::<i32, XjError>(42) })`
    from within the current-thread async context.
  - assert: Returns `Ok(42)`. Does NOT panic.
  - command: `cargo test -p camel-xj --lib block_on_result_works_on_current_thread`
  - expected: FAILS (panics) before the fix, PASSES after

- `block_on_result_works_without_ambient_runtime`:
  - setup: Construct `XjComponent::default()` in a plain `#[test]` (no
    `#[tokio::test]`).
  - action: Call `component.block_on_result(async { Ok::<i32, XjError>(99) })`.
  - assert: Returns `Ok(99)`. Does NOT panic.
  - command: `cargo test -p camel-xj --lib block_on_result_works_without_ambient_runtime`
  - expected: FAILS (dead channel / wrong result) before the fix, PASSES after

- `block_on_result_works_on_multi_thread_runtime`:
  - setup: `#[tokio::test(flavor = "multi_thread")]`. Construct
    `XjComponent::default()`.
  - action: Call `component.block_on_result(async { Ok::<i32, XjError>(7) })`.
  - assert: Returns `Ok(7)`.
  - command: `cargo test -p camel-xj --lib block_on_result_works_on_multi_thread`
  - expected: PASSES before and after (unchanged behavior)

- `offload_runtime_dropped_with_component`:
  - setup: Construct `XjComponent::default()`. Obtain a `Weak<OffloadRuntime>`
    via `let weak = component.offload_weak();` (the test-only accessor added
    in step 11).
  - action: Drop the component.
  - assert: `weak.upgrade().is_none()` — the OffloadRuntime is released.
  - command: `cargo test -p camel-xj --lib offload_runtime_dropped_with_component`
  - expected: pass after implementation

- `offload_runtime_drop_no_panic_in_async`:
  - setup: `#[tokio::test(flavor = "multi_thread")]`. Construct
    `XjComponent::default()`.
  - action: Drop the component inside the async context (end of test block).
  - assert: No panic during drop. This verifies the `Drop` impl (step 4b)
    correctly moves the runtime to a scoped thread for teardown, avoiding
    the "Cannot drop a runtime in a context where blocking is not allowed"
    panic. This is the production shutdown path: the context registry drops
    `Arc<dyn Component>` at the end of `async fn run`.
  - command: `cargo test -p camel-xj --lib offload_runtime_drop_no_panic_in_async`
  - expected: FAILS (panics on drop) without step 4b, PASSES with it

**Integration test deferred to CI:** The full end-to-end test
(create_endpoint + transform on current-thread runtime with a real bridge
binary) requires the Java xml-bridge sidecar, which is CI infrastructure.
The unit test `offload_runtime_spawned_task_survives_after_block_on` is the
lib-level regression for the dead-Channel defect: it proves tasks spawned on
the offload runtime survive past `block_on` returning, which is the exact
invariant a real Channel dispatch task depends on.

**Acceptance:**
- `cargo fmt --check --all` passes
- `cargo clippy -p camel-xj -- -D warnings` passes
- `cargo test -p camel-xj --lib` passes (all new + existing tests)
- No `block_in_place` call remains when ambient runtime is `CurrentThread`
- No ephemeral `new_current_thread()` builder remains in `block_on_result`

- [ ] 1.1
