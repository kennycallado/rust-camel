# Tasks: rssbase

## Task 1: Weak component-context reference + example anchors

Files:
- crates/camel-core/src/shared/components/domain/registry.rs (modified)
- examples/wasm-example/src/main.rs (modified)
- examples/wasm-bean-example/src/main.rs (modified)
- examples/wasm-streaming-plugin/src/main.rs (modified)
- examples/security-wasm-policy/src/main.rs (modified)

Steps:
1. In `crates/camel-core/src/shared/components/domain/registry.rs`, change
   `RegistryComponentContext.registry` field type from
   `Arc<std::sync::Mutex<Registry>>` to
   `std::sync::Weak<std::sync::Mutex<Registry>>`.
2. In `RegistryComponentContext::new`, store `Arc::downgrade(&registry)`
   (the parameter stays a strong `Arc<Mutex<Registry>>`; the public
   signature is unchanged).
3. In the `ComponentContext` impl's `resolve_component`, upgrade the
   weak reference (`self.registry.upgrade()?` returns `None` when
   unanchored), lock, clone the component out, drop the guard and the
   upgraded `Arc` before returning the component so no guard or anchor
   is retained by the caller's return path.
4. Add a doc comment on the struct stating the non-owning contract:
   resolution works only while a strong registry reference exists
   elsewhere (the owning `CamelContext`, or an anchor the embedder
   retains).
5. In `examples/wasm-example/src/main.rs`, keep the standalone registry
   alive: bind `let registry_anchor = Arc::clone(&registry);` before the
   `Arc` moves into `RegistryComponentContext::new`, with a comment
   stating the anchor keeps weak resolution live; hold it until the run
   completes (leading underscore-free binding kept alive with
   `let _registry_anchor = registry_anchor;` after route setup, or
   simply keep the binding in scope — do not introduce `#[allow(
   unused_variables)]`).
6. Apply the same anchor pattern in `examples/wasm-bean-example/
   src/main.rs` for `registry_arc`.
7. Apply the same anchor pattern in `examples/wasm-streaming-plugin/
   src/main.rs` (registry moved at line ~48) and
   `examples/security-wasm-policy/src/main.rs` (registry moved at
   line ~125): each retains a strong `Arc` clone in scope for the
   program's lifetime.
8. Fix the existing test
   `resolve_component_unaffected_by_observability_params` in the same
   tests module (registry.rs ~line 429): it currently moves its only
   strong registry `Arc` into the wrapper and then expects
   `resolve_component("timer")` to return `Some` — under the weak
   reference that anchor dies at construction. Keep a strong anchor
   binding alive across the assertion (e.g., bind
   `let anchor = Arc::clone(&registry);` before constructing `ctx` and
   hold it until after the assert).

Tests (in the existing `#[cfg(test)] mod tests` of
`crates/camel-core/src/shared/components/domain/registry.rs`; the module
already imports `TimerComponent` and `LogComponent` — use
`TimerComponent::new()` (scheme `timer`) as the registered component; no
new probe type is needed):

1. name: `resolve_component_returns_component_while_anchored`
   setup: `let registry = Arc::new(Mutex::new(Registry::new()));` with
   `TimerComponent::new()` registered under scheme `timer`;
   `let ctx = RegistryComponentContext::new(Arc::clone(&registry),
   None, false);` — the local `registry` binding is the strong anchor
   action: `ctx.resolve_component("timer")`
   assert: returns `Some` whose `scheme()` is `timer`
   command: `cargo test -p camel-core --lib resolve_component_returns_component_while_anchored`
   expected: passes after step 3 (the test itself is new).

2. name: `resolve_component_returns_none_after_last_strong_ref_drops`
   setup: as above, but `drop(registry);` after building `ctx` (that
   binding held the only strong reference)
   action: `ctx.resolve_component("timer")`
   assert: returns `None`
   command: `cargo test -p camel-core --lib resolve_component_returns_none_after_last_strong_ref_drops`
   expected: fails before the weak change (returns `Some`), passes
   after.

Acceptance:
- `cargo test -p camel-core --lib resolve_component` — both new tests
  pass, AND the existing
  `resolve_component_unaffected_by_observability_params` still passes.
- `cargo check -p wasm-example -p wasm-bean-example
  -p wasm-streaming-plugin -p security-wasm-policy` exits 0.
- `cargo fmt --check` and `cargo clippy -p camel-core -- -D warnings`
  exit 0.
- `rg -n 'Arc<std::sync::Mutex<Registry>>' crates/camel-core/src/
  shared/components/domain/registry.rs` shows no owning field inside
  `RegistryComponentContext`.

- [x] rssbase-1

## Task 2: Context drop terminates controller tasks

Files:
- crates/camel-core/src/context.rs (modified)
- crates/camel-core/src/context_tests.rs (modified)

Steps:
1. In `crates/camel-core/src/context.rs`, add
   `impl Drop for CamelContext` after the existing field/method blocks
   near the other context impls: take `self.actor_join` and
   `self.supervision_join` via `Option::take` and call
   `JoinHandle::abort()` on each when present. `take_actor_join`
   (`context.rs:806`) already exists for the abort path — the Drop impl
   must not fight it: if `take_actor_join` was already called, the
   Option is `None` and Drop skips.
2. Doc-comment the impl: drop is a NON-graceful termination of the
   controller actor and supervision tasks; route and service teardown
   remains the caller's `stop()` responsibility; an outstanding
   controller command is interrupted at its await point (same
   cancellation class as `abort()`; ADR-0018 sequencing is not extended
   to the drop path); `stop()`→`start()` restart semantics are
   unchanged because Drop fires only when the context value is
   discarded.
3. In `crates/camel-core/src/context_tests.rs`, add a test-local
   `ProbeComponent { dropped: Arc<AtomicBool> }` implementing `Drop`
   (sets the flag) and `Component` (`scheme() -> "rssbase-probe"`,
   `metadata()` via the same `ComponentMetadata` construction pattern
   as `crates/components/camel-direct/src/lib.rs:217`,
   `create_endpoint` returning `Err(CamelError::ComponentNotFound(
   "rssbase-probe".into()))`).

Tests (in `crates/camel-core/src/context_tests.rs`; multi-thread test
flavor with at least 2 worker threads so the aborted actor's drop
schedules while the test task polls; waits are deadline-bounded using
the markerless pattern that BOTH linters ignore —
`tokio::time::timeout(Duration::from_secs(5), async { while !flag.
load(Ordering::SeqCst) { tokio::task::yield_now().await; } })` — no
`tokio::time::sleep` in the test body, keeping the lint-test-sleep
ratchet count at 470 and satisfying lint-unbounded-wait without an
ADR-0069 marker):

1. name: `drop_runs_exclusive_component_drop_after_stop`
   setup: `let mut ctx = CamelContext::builder().build().await
   .expect("build context");` with `ProbeComponent` registered via
   `register_component`; `ctx.start().await` then `ctx.stop().await`
   complete
   action: `drop(ctx)`; await the bounded yield-poll for the
   `AtomicBool` flag
   assert: the poll completes within the 5-second timeout (flag `true`)
   command: `cargo test -p camel-core --lib drop_runs_exclusive_component_drop_after_stop`
   expected: fails before this change AND before rssbase-1 (the actor
   pin — `DefaultRouteController.registry`, route_controller.rs:67 —
   alone keeps the probe alive at camel-core level), passes after both.

2. name: `stop_start_restart_then_drop_runs_component_drop`
   setup: as test 1
   action: `ctx.start().await`; `ctx.stop().await`; `ctx.start().await`;
   `ctx.stop().await`; `drop(ctx)`; same bounded yield-poll
   assert: the poll completes within the 5-second timeout (restart
   through the living actor works, and the later drop still terminates
   it)
   command: `cargo test -p camel-core --lib stop_start_restart_then_drop_runs_component_drop`
   expected: fails before this change, passes after.

3. Existing `test_stop_keeps_actor_alive_for_restart`
   (`context_tests.rs:1383`) must remain green unchanged — it pins the
   stop→start half of the spec scenario.
   command: `cargo test -p camel-core --lib test_stop_keeps_actor_alive_for_restart`

Spec-scenario ownership note: the delta spec's "wasm bundle does not
pin the component registry" scenario is owned TRANSITIVELY — Task 1
proves the seam cannot pin (weak resolution), and Task 3's end-to-end
batch (full boot with the real wasm bundle; the xj leak signature is
+4 fds/doc) proves the unpinned outcome on the complete composition.

Acceptance:
- Each of these exits 0 (separate invocations; cargo test takes one
  positional filter at a time):
  `cargo test -p camel-core --lib drop_runs_exclusive_component_drop_after_stop`;
  `cargo test -p camel-core --lib stop_start_restart_then_drop_runs_component_drop`;
  `cargo test -p camel-core --lib test_stop_keeps_actor_alive_for_restart`.
- `cargo fmt --check` and `cargo clippy -p camel-core -- -D warnings`
  exit 0.
- `cargo xtask lint-unbounded-wait` exits 0 and
  `cargo xtask lint-test-sleep` shows the ratchet count still ≤ 470
  (the yield-poll pattern adds no counted sleep).

- [x] rssbase-2

## Task 3: End-to-end batch verification (measurement)

Files:
- (none in-repo; scripts and CSV land under
  `/tmp/nix-shell.FRkhPf/opencode/rssbase/verify/`, results are posted
  as a bd comment on rc-wlg8h)

Steps:
1. `cargo build -p camel-cli` in the worktree (dev profile).
2. Regenerate the 218-doc proxy corpus under
  `/tmp/nix-shell.FRkhPf/opencode/rssbase/verify/corpus218/` by copying
   `examples/integration-testing/Camel.toml` into the directory and
   writing 218 copies of `examples/integration-testing/
   partner-crud.test.yaml` named `doc-000.test.yaml` through
   `doc-217.test.yaml`.
3. Create `/tmp/nix-shell.FRkhPf/opencode/rssbase/verify/sample_batch.py`
   (Python 3, stdlib only) implementing the sampler:
   - `subprocess.Popen(["<worktree>/target/debug/camel", "test",
     "<corpus218-abs-path>", "--integration"], cwd=<corpus218>,
     stdout=PIPE, stderr=STDOUT, text=True, bufsize=1)`;
   - read stdout line-by-line; on each line matching the regex
     `\.test\.yaml \[(full\*?|lean)\]$`, read `VmRSS` and `Threads`
     from `/proc/<pid>/status` and count `/proc/<pid>/fd` entries;
     record `(doc_index, rss_kb, threads, fds)`;
   - after `proc.wait()`, compute the OLS slope of `rss_kb` over
     `doc_index` skipping the first 5 samples (warmup);
   - print one summary line
     `docs=<N> rc=<rc> rss=<first>-><last>MB slope=<s>MB/doc threads=<first>-><last> fds=<first>-><last>`;
   - exit 0 only when: `rc == 0`, the final stdout contained
     `1090 passed, 0 failed`, slope ≤ 0.05 MB/doc, thread delta ≤ 1,
     fd delta ≤ 1; otherwise exit 1 naming the violated bound.
4. Run `python3 /tmp/nix-shell.FRkhPf/opencode/rssbase/verify/sample_batch.py`.
5. Post a bd comment on rc-wlg8h with: doc count, exit code, RSS
   first→last, slope MB/doc, thread delta, fd delta.

Tests:
1. name: `batch_rss_slope_below_bound`
   setup: worktree binary built (step 1), corpus generated (step 2)
   action: run the sampler wrapper (step 3) against the 218-doc corpus
   assert: process exit code 0 with `1090 passed, 0 failed`; RSS slope
   ≤ 0.05 MB/doc; thread count delta ≤ 1 over the whole batch; fd count
   delta ≤ 1
   command: `python3 /tmp/nix-shell.FRkhPf/opencode/rssbase/verify/sample_batch.py`
   (the wrapper script created in step 3; it prints a one-line summary
   and exits non-zero when any bound is violated)
   expected: before rssbase-1+2 the slope is ~0.49 MB/doc with +1
   thread/doc and +4 fds/doc (bound violated); after both tasks all
   bounds hold.

Acceptance:
- Sampler summary line shows `docs=218 rc=0`, slope ≤ 0.05 MB/doc,
  threads delta ≤ 1, fds delta ≤ 1.
- bd comment posted on rc-wlg8h containing the four measured numbers.

- [x] rssbase-3
