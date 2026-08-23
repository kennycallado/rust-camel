# Tasks: advice-route-interception

## camel-core: rule model

### Task 1: `InterceptRules` model with `mock:`-only targets and first-match lookup

**Files:**
- `crates/camel-core/src/intercept.rs` (new)
- `crates/camel-core/src/lib.rs` (modified — `pub mod intercept;`)

**Steps:**
1. Create `crates/camel-core/src/intercept.rs` with:
   ```rust
   #[derive(Debug, Clone, PartialEq, Eq)]
   pub struct InterceptRule { pub uri: String, pub action: InterceptAction }
   #[derive(Debug, Clone, PartialEq, Eq)]
   pub enum InterceptAction {
       SkipTo { uri: String },
       DivertCopyTo { uri: String },
   }
   #[derive(Debug, Clone, Default)]
   pub struct InterceptRules { rules: Vec<InterceptRule> }
   ```
2. Implement `InterceptRules::new(rules: Vec<InterceptRule>) -> Result<Self, CamelError>`: iterate with index; for every rule whose action target URI does not start with `mock:`, return `CamelError::Config(String)` (the existing config variant, `crates/camel-api/src/error.rs:129-130`) carrying a message that contains the rule index and the offending target URI (build with `format!`). Duplicates ARE permitted; declaration order preserved; no dedup.
3. Implement `pub fn lookup(&self, send_uri: &str) -> Option<&InterceptAction>` — exact `String` equality, first match in order.
4. Add `pub fn is_empty(&self) -> bool`.
5. Declare `pub mod intercept;` in `lib.rs`.

**Tests:** (in `crates/camel-core/src/intercept.rs` `#[cfg(test)]`; shared command/expected for tests 1-3: command `cargo test -p camel-core --lib intercept::`; expected: fails before implementation — module absent — passes after)
1. `non_mock_action_targets_are_rejected_at_rule_construction` — setup: `let skip_bad = InterceptRule { uri: "kafka:x".into(), action: SkipTo { uri: "direct:y".into() } }; let divert_bad = InterceptRule { uri: "kafka:z".into(), action: DivertCopyTo { uri: "seda:w".into() } };`; action: `InterceptRules::new(vec![skip_bad.clone()])`, `InterceptRules::new(vec![divert_bad.clone()])`, `InterceptRules::new(vec![skip_bad, divert_bad])`; assert: every construction errs as `CamelError::Config`, each error message contains the rule index (0) and the target URI (`direct:y` / `seda:w`); command: `cargo test -p camel-core --lib intercept::`; expected: fails before implementation (module absent), passes after.
2. `duplicate_uris_preserve_declaration_order` — setup: `let r1 = InterceptRule { uri: "seda:out".into(), action: SkipTo { uri: "mock:a".into() } }; let r2 = InterceptRule { uri: "seda:out".into(), action: SkipTo { uri: "mock:b".into() } };`; action: `InterceptRules::new(vec![r1, r2])` then `lookup("seda:out")`; assert: `Ok`, lookup returns `SkipTo { uri: "mock:a" }` (first rule, `PartialEq` assert), and `lookup("seda:out2")` returns `None`. Command as above.
3. `mock_targets_accepted` — setup: `InterceptRules::new(vec![InterceptRule { uri: "kafka:x".into(), action: SkipTo { uri: "mock:y".into() } }, InterceptRule { uri: "kafka:z".into(), action: DivertCopyTo { uri: "mock:w".into() } }])`; action: `new`; assert: `Ok` with both rules retrievable via `lookup("kafka:x")` / `lookup("kafka:z")`. Command as above.

**Acceptance:**
- `cargo test -p camel-core --lib intercept::` exits 0.
- `cargo clippy -p camel-core -- -D warnings` and `cargo fmt --check --all` exit 0.
- `cargo xtask lint-unwrap` exits 0 (test `unwrap`s use `// allow-unwrap` markers per repo convention).

- [x] 1

## camel-processor: divert composition + WireTap restart

### Task 2: `compose_divert` service + `WireTapLifecycle` restart-reopen

**Files:**
- `crates/camel-processor/src/intercept_compose.rs` (new)
- `crates/camel-processor/src/lib.rs` (modified — `pub mod intercept_compose;` plus `pub use intercept_compose::compose_divert;`, mirroring the wire_tap re-export pattern at `lib.rs:47/:109`)
- `crates/camel-processor/src/wire_tap.rs` (modified — lifecycle restart semantics)

**Steps:**
1. In `wire_tap.rs`, change `WireTapLifecycle::start` so that a start-after-shutdown reopens the shared state: set `inner.open = true`, replace `inner.cancel` with a fresh `CancellationToken`, replace `inner.tracker` with a fresh task tracker, AND reset `shutdown_called` to `false` (without this, the second `shutdown` after restart is a silent no-op — admission stays open, in-flight copies never drained). Keep `shutdown` semantics otherwise as-is (`wire_tap.rs:191-225`: close admission, cancel token, drain tracker).
2. Create `intercept_compose.rs` with `pub fn compose_divert(tap: WireTapService, real: BoxProcessor) -> BoxProcessor`. `WireTapService` is `Clone` and shares state (`wire_tap.rs:121`), so the caller keeps a clone for lifecycle wiring. Implementation: call the public `WireTapService` as the copy stage (its `call` already implements detached admission / inline CallerRuns and failure suppression); its returned original exchange then feeds the real-producer stage, which awaits readiness on the SAME real service instance before `call` and returns the real `Result<Exchange, CamelError>` verbatim; readiness errors return verbatim and skip `call`. Do NOT use `WireTapLayer` (it ignores the inner service, `wire_tap.rs:387-392`) and do NOT touch the private `WireTapShared`/`run_tap` items.
3. Lifecycle composition lives in camel-core (Task 5), NOT here: `CompositeStepLifecycle` is `pub(crate)` in camel-core and camel-processor cannot depend on camel-core (publish cycle). This task only guarantees `WireTapService`'s own lifecycle exposes the restart-reopen behavior.

**Tests:** (in `crates/camel-processor/src/intercept_compose.rs` `#[cfg(test)]`; shared command/expected for tests 1-4: command `cargo test -p camel-processor intercept_compose`; expected: fails before implementation — module absent — passes after)
1. `real_producer_readiness_is_driven_before_call_success_order` — setup: `let tap = WireTapService::new(copy_stub); let svc = compose_divert(tap, real_stub)`; real stub pushes `"ready"`/`"call"` events into a `std::sync::Mutex<Vec<&'static str>>` and returns a sentinel `Ok` exchange carrying header `X-Sentinel=real-ok`; action: `svc.oneshot(exchange).await`; assert: events `["ready", "call"]` in order, returned exchange carries `X-Sentinel=real-ok`.
2. `real_producer_readiness_failure_returns_verbatim_and_skips_call` — setup: real stub whose `poll_ready` errs with `CamelError::ProcessorError("sentinel-ready")` and whose `call` pushes `"call"`; action: oneshot; assert: returned error is `ProcessorError("sentinel-ready")` verbatim, events vec empty.
3. `wiretap_lifecycle_start_reopens_admission_with_fresh_token` — setup: `WireTapService` (default bound) + lifecycle + copy target carrying BOTH an arrival `Notify` (positive asserts) and an `Arc<AtomicUsize>` arrival counter (negative asserts — `Notify` alone cannot prove absence); action: `shutdown().await` → send → await send completion → assert counter UNCHANGED (no copy); `start().await` → send → await `Notify` → assert counter +1 (copy arrived); then `shutdown().await` AGAIN → send → await send completion → assert counter UNCHANGED again (second shutdown effective after restart; admission closed). No `sleep`.
4. `copy_call_failure_is_suppressed_and_logged` — setup: copy stub whose `call` returns `Err(ProcessorError("copy-boom"))` and signals completion via `Notify`; real stub returning sentinel Ok; action: oneshot, THEN await the copy stub's completion signal (the copy runs detached; the warn may fire after oneshot resolves); assert: real Ok verbatim and a `tracing` test subscriber captured a `warn` containing `copy-boom`.
**Acceptance:**
- `cargo test -p camel-processor` exits 0.
- `cargo clippy -p camel-processor -- -D warnings` and `cargo fmt --check --all` exit 0.
- `cargo xtask lint-unwrap` exits 0.
- `rg 'sleep' crates/camel-processor/src/intercept_compose.rs` returns no hits in test code.

- [x] 2

## camel-core: plumbing + freeze

### Task 3: builder/controller plumbing and the freeze contract

**Files:**
- `crates/camel-core/src/context_builder.rs` (modified)
- `crates/camel-core/src/context.rs` (modified)
- `crates/camel-core/src/lifecycle/adapters/controller_actor.rs` (modified — command dispatch)
- `crates/camel-core/src/lifecycle/adapters/controller_actor_commands.rs` (modified — `RouteControllerCommand` enum at line 19 + `RouteControllerHandle` at line 178 gain `SetInterceptRules`/`MarkStarted`)
- `crates/camel-core/src/lifecycle/adapters/route_controller.rs` (modified — freeze state on `DefaultRouteController`)
- `crates/camel-core/src/lifecycle/adapters/step_resolution.rs` (modified — `CompilationContext` construction at line 209)
- `crates/camel-core/src/lifecycle/adapters/step_compilers/mod.rs` (modified — struct field + 5 test-module literal fixes at lines 479/572/655/744/875)
- `crates/camel-core/src/lifecycle/adapters/step_compilers/endpoints.rs` (modified — test literal fix at line 327)
- `crates/camel-core/src/lifecycle/adapters/step_compilers/splitting.rs` (modified — test literal fix at line 436)
- `crates/camel-core/src/lifecycle/adapters/step_compilers/core.rs` (modified — 4 literal fixes at lines 688/800/990/1150)
- `crates/camel-core/tests/route_interception_test.rs` (new)

**Steps:**
1. `context_builder.rs`: add `pub fn with_intercept_rules(mut self, rules: InterceptRules) -> Self` storing the rules; at `build()`, extract and pass them into the `DefaultRouteController` construction (same extraction pattern as existing controller configuration, `context_builder.rs:131`).
2. `route_controller.rs` (`DefaultRouteController`): hold `intercept: InterceptRules` plus `frozen: bool`; expose `set_intercept_rules(&mut self, rules: InterceptRules) -> Result<(), CamelError>` returning `CamelError::Config` naming the freeze reason when `frozen`. Freeze trips at exactly two points: `add_route` success, and the `MarkStarted` command. `frozen` is never reset (stop/restart included).
3. `controller_actor_commands.rs`: add `RouteControllerCommand::SetInterceptRules { rules, reply }` and `RouteControllerCommand::MarkStarted { reply }` plus the corresponding `RouteControllerHandle` send methods; `controller_actor.rs`: dispatch both to the controller. Make `CamelContext::start` (`context.rs:677`) await `MarkStarted` before returning success so the freeze trips even with zero routes; command processing stays sequential (existing mpsc actor loop).
4. `context.rs`: add `pub async fn set_intercept_rules(&self, rules: InterceptRules) -> Result<(), CamelError>` sending the actor command and returning its reply.
5. `step_compilers/mod.rs`: add `pub intercept: InterceptRules` to the compiler-context struct (the struct has no `Default` derive — reference fields; every construction site sets the field explicitly); populate it in the production `CompilationContext` construction at `step_resolution.rs:209`, threading from controller state; add `intercept: InterceptRules::default()` to the 11 existing struct-literal constructions: `step_compilers/core.rs:688/800/990/1150`, `step_compilers/mod.rs:479/572/655/744/875`, `step_compilers/endpoints.rs:327`, `step_compilers/splitting.rs:436` (test modules included).

**Tests:** (in `crates/camel-core/tests/route_interception_test.rs`; harness boots a `CamelContext` via `CamelContext::builder()` with mock/direct/seda components — all three are already camel-core dev-deps; shared command/expected for Task 3 tests 1-4: command `cargo test -p camel-core --test route_interception_test`; expected: tests 1-4 fail before the freeze wiring, pass after)
1. `setting_rules_after_the_first_route_registration_is_rejected` — setup: builder-booted context, one route added successfully (`from: direct:in → to: mock:out`); action: `ctx.set_intercept_rules(InterceptRules::new(vec![InterceptRule { uri: "seda:out".into(), action: SkipTo { uri: "mock:z".into() } }]).unwrap()).await`; assert: `Err` (message contains `frozen`); the route still delivers `direct:in → mock:out`.
2. `setting_rules_after_start_of_an_empty_context_is_rejected` — setup: context with zero routes, `ctx.start().await` succeeds; action: `set_intercept_rules` with one valid rule (`SkipTo mock:z` on `seda:out`); assert: `Err` containing `frozen`.
3. `a_failed_start_does_not_freeze_rules` — setup: context with a failing `ConfigCheck` startup check (`add_startup_check` with a check that always errs) so `start()` fails; action: `set_intercept_rules` with one valid rule after the failed start; assert: `Ok(())`.
4. `stop_restart_does_not_unfreeze_rules` — setup: started context (one route); action: `ctx.stop().await`, then `ctx.start().await` (restart), then `set_intercept_rules` with one valid rule (`SkipTo mock:z` on `seda:out`); assert: still `Err` containing `frozen` (restart does not unfreeze).

**Acceptance:**
- `cargo test -p camel-core --test route_interception_test` exits 0 for the four tests above.
- `cargo clippy -p camel-core -- -D warnings` and `cargo fmt --check --all` exit 0.
- `cargo xtask lint-unwrap` exits 0.

- [x] 3

## camel-core: skip semantics

### Task 4: `SkipTo` substitution in the `To` arm before component resolution

**Files:**
- `crates/camel-core/src/lifecycle/adapters/step_compilers/endpoints.rs` (modified)
- `crates/camel-core/tests/route_interception_test.rs` (modified — added tests)

**Steps:**
1. In the `To` arm, before `parse_uri` (`endpoints.rs:31`), consult `ctx.intercept.lookup(&uri)`:
   - `None` or empty rules → existing code path unchanged.
   - `Some(SkipTo { uri: target })` → substitute `uri = target` and fall through to the normal resolve/endpoint/producer/contract/lifecycle path with the substituted URI (the original URI is never parsed; its scheme never resolved).
2. Resolution failure of the substituted target surfaces as the normal `CamelError`, with the error text enriched to include `intercept target: {target}`.

**Tests:** (in `route_interception_test.rs`; shared command/expected for Task 4 tests 1-5: command `cargo test -p camel-core --test route_interception_test`; expected: fail before the To-arm substitution lands, pass after)
1. `exact_uri_match_with_first_match_wins` — setup: rules `[("seda:out", SkipTo mock:first), ("seda:out", SkipTo mock:second)]` set via builder, mock + seda + direct registered, route `from: direct:in → to: seda:out`; action: send one exchange via direct producer; assert: `mock:first` records 1, `mock:second` records 0.
2. `empty_rule_set_leaves_the_send_untouched` — setup: two identically-booted contexts (one with empty `InterceptRules` via builder, one with no interception configuration), route `from: direct:in → to: seda:out` with a seda consumer recording bodies via an await-aware arrival primitive; action: send the same sentinel exchange in both, await each consumer's recorded arrival of the sentinel (Notify/channel); assert: each consumer recorded exactly one exchange with identical body text.
3. `skipped_uri_with_unregistered_real_component` — setup: rules `[("kafka:orders", SkipTo mock:orders)]`, NO kafka component registered, mock/direct registered, route `from: direct:in → to: kafka:orders`; action: add route + send; assert: registration succeeds, `mock:orders` records 1 (await via the mock endpoint's notify-aware count primitive).
4. `skip_target_resolution_failure_is_a_compile_error` — setup: rules `[("kafka:x", SkipTo mock:x)]`, no mock component registered, direct registered; action: `add_route_definition` for `from: direct:in → to: kafka:x`; assert: `Err` whose message contains `mock:x`.
5. `skip_replaces_the_enqueue` (seda send-side fence) — setup: rules `[("seda:q", SkipTo mock:q)]`, running `from: seda:q → to: mock:sink` consumer, route `from: direct:in → to: seda:q`; action: send one exchange through the intercepted send; then enqueue a distinguishable BARRIER exchange directly into `seda:q` (via a separately created seda producer, no interception) and await `mock:sink` recording the BARRIER (the mock's await-aware count/body primitive — downstream completion proof, not merely consumer dequeue); assert: `mock:q` records 1, `mock:sink`'s recorded bodies contain the BARRIER and NOT the intercepted sentinel, and `mock:sink` records exactly 1.

**Acceptance:**
- `cargo test -p camel-core --test route_interception_test` exits 0 (Tasks 3+4 tests).
- `cargo clippy -p camel-core -- -D warnings` and `cargo fmt --check --all` exit 0.
- `cargo xtask lint-unwrap` exits 0.

- [x] 4

## camel-core: divert semantics

### Task 5: `DivertCopyTo` composition with outcome isolation

**Files:**
- `crates/camel-core/src/lifecycle/adapters/step_compilers/endpoints.rs` (modified)
- `crates/camel-core/tests/route_interception_test.rs` (modified — added tests)

**Steps:**
1. In the `To` arm, on `Some(DivertCopyTo { uri: copy_uri })`: resolve the real endpoint/producer/contract/lifecycle exactly as the un-intercepted path does; then resolve the copy endpoint/producer from `copy_uri` (failure ⇒ compile error naming the copy target, same enrichment as Task 4 step 2).
2. Compose: `let tap = WireTapService::new(copy_producer); let processor = camel_processor::intercept_compose::compose_divert(tap.clone(), real_producer);` — the clone shares the same internal state so lifecycle and processor drain the SAME tracker. Body contract from the REAL endpoint.
3. Lifecycle: inline in endpoints.rs with the existing `pub(crate)` `CompositeStepLifecycle`, mirroring the WireTap arm at `endpoints.rs:63-70`: children in order `[copy_endpoint_lifecycle?, tap.lifecycle(), real_lifecycle?]` (omit `None` members). `CompositeStepLifecycle::shutdown` iterates in REVERSE — real tears down first, then the tracker drains, then the copy endpoint closes; that ordering is correct and intended (copies drain before their target endpoint closes) — do not flip the vec.
4. Copy completion is observable through the composed lifecycle drain (route stop) — tests use the mock component's notify-aware count primitives or channel stubs; no polling sleeps.

**Tests:** (in `route_interception_test.rs`; stub producers via a test `Component` whose producer records events/errors per test; where a warn is asserted, await the copy stub's completion signal BEFORE asserting the captured warn — detached copies may finish after oneshot; shared command/expected for Task 5 tests 1-9: command `cargo test -p camel-core --test route_interception_test`; expected: fail before the divert composition lands, pass after)
1. `divert_delivers_both_copy_and_real_message` — setup: rules `[("seda:out", DivertCopyTo mock:tap)]`, running `from: seda:out` consumer recording arrivals with an await-aware arrival primitive (Notify or channel receiver); action: send one exchange through `to: seda:out`, await the consumer's arrival notification for the real message, then stop the route (drain); assert: `mock:tap` records 1 clone and the consumer recorded the real message.
2. `saturated_divert_runs_the_copy_inline_before_the_real_send` — setup: copy-target component whose `call` increments an ordinal counter and signals arrival via `Notify` FIRST, then behavior is per-ordinal: calls 1-20 park unconditionally (on a held permit), call 21+ returns immediately (NO test-thread flag flip — a flippable flag races copy 20: it signals before parking, so the flip could land before the park); shared `Mutex<Vec<u32>>` order log fed by both copy and real targets; action: send 20 exchanges, await the admission barrier (copy-call ordinal counter reaching 20 plus each arrival `Notify`), then send the 21st exchange; assert: in the order log the 21st copy event precedes the 21st real event, and the 21st outcome is the real producer's sentinel `Ok` verbatim. Cleanup: release the parked permits, stop/drain the route so in-flight copies finish before the test ends.
3. `real_ok_outcome_stays_verbatim_when_the_copy_call_fails` — setup: copy stub `call → Err(ProcessorError("copy-boom"))` + completion signal, real producer returning sentinel `Ok`; action: oneshot, await copy signal; assert: sentinel `Ok` verbatim, captured `tracing` warn contains `copy-boom`.
4. `real_err_outcome_stays_verbatim_when_the_copy_succeeds` — setup: copy stub Ok, real producer returning `Err(ProcessorError("real-boom"))`; action: oneshot, await copy completion; assert: `ProcessorError("real-boom")` verbatim.
5. `copy_poll_ready_failure_is_swallowed` — setup: copy stub whose readiness errs (signal completion after the readiness attempt), real producer sentinel Ok; action: oneshot, await copy signal; assert: real Ok returned, warn captured.
6. `copy_target_resolution_failure_is_a_compile_error` — setup: rules `[("seda:out", DivertCopyTo mock:ghost)]`, no mock registered; action: add route `from: direct:in → to: seda:out`; assert: `Err` naming `mock:ghost`.
7. `real_producer_readiness_is_driven_before_call` — two cases at compiled-step level: success order (`["ready","call"]`, sentinel Ok verbatim) and readiness-failure (sentinel error verbatim, events empty).
8. `divert_children_shut_down_in_reverse_order` — in an endpoints.rs `#[cfg(test)]` module: construct the real composition vec `[copy_lifecycle, tap_lifecycle, real_lifecycle]` with three recording lifecycles (each pushing its id on `shutdown`), wrap in `CompositeStepLifecycle`, call `shutdown().await`; assert recorded order is real → tap → copy (reverse iteration, `composite_step_lifecycle.rs:66-67`) — copies drain before their target endpoint tears down.
9. `divert_survives_route_stop_and_restart` — setup: divert rule on `to: seda:out`, seda consumer with await-aware arrival primitive + `mock:tap`; action: stop the route, restart it, send one exchange, await the consumer's arrival notification, stop again (drain); assert: `mock:tap` records the clone and the consumer recorded the real message (both after restart).

**Acceptance:**
- `cargo test -p camel-core --test route_interception_test` exits 0 (all tests so far).
- `cargo clippy -p camel-core -- -D warnings` and `cargo fmt --check --all` exit 0.
- `cargo xtask lint-unwrap` exits 0; `rg 'sleep' crates/camel-core/tests/route_interception_test.rs` returns no hits.

- [x] 5

## camel-core: consistency + boundary

### Task 6: hot-reload rule consistency + hexagonal boundary extension

**Files:**
- `crates/camel-core/tests/route_interception_test.rs` (modified)
- `crates/camel-core/tests/hexagonal_architecture_boundaries_test.rs` (modified)

**Steps:**
1. Add `recompiled_pipelines_keep_the_same_rules` to `route_interception_test.rs`. Mechanism: follow the pattern proven by `crates/camel-core/tests/hot_reload.rs` — construct the controller directly (`make_controller()`-style: `DefaultRouteController` with the frozen `InterceptRules`), use its PUBLIC `compile_route_definition` and `swap_pipeline` (`route_controller.rs:676/819`) to force a recompile of a route with rules `[("seda:out", SkipTo mock:q)]`, then drive traffic. Do NOT use `compile_route_definition_pipeline` (it is `pub(crate)`, `context.rs:166`) or in-module hot-reload helpers — both are unreachable from an integration test. Assert: after recompile + swap, a new exchange still lands in `mock:q`.
2. Extend `hexagonal_architecture_boundaries_test.rs`: use the suite's existing `assert_file_not_contains(path, forbidden)` helper to assert the three interception modules — `crates/camel-core/src/intercept.rs`, the `To`-arm region of `crates/camel-core/src/lifecycle/adapters/step_compilers/endpoints.rs`, and `crates/camel-processor/src/intercept_compose.rs` — contain no `RuntimeBus`, `RuntimeQuery`, or `RuntimeQueryBus` tokens (test name per suite convention: `intercept_modules_have_no_query_plane_dependency`).

**Tests:**
1. `recompiled_pipelines_keep_the_same_rules` — setup: `DefaultRouteController` built via the `make_controller()` pattern (`tests/hot_reload.rs:218`), initialized with frozen `InterceptRules` carrying `[("seda:out", SkipTo mock:q)]`, route `from: direct:in → to: seda:out` compiled and started; action: `compile_route_definition` (`route_controller.rs:676`) again for the same definition, `swap_pipeline` (`:819`) to install the recompiled pipeline, send one exchange via `direct:in`; assert: the exchange lands in `mock:q` (count 1, awaited via the mock's notify-aware primitive); command: `cargo test -p camel-core --test route_interception_test recompiled`; expected: fails before Task 3/4 wiring, passes at end.
2. `intercept_modules_have_no_query_plane_dependency` — setup: the three interception files exist (`crates/camel-core/src/intercept.rs`, the endpoints.rs `To`-arm region, `crates/camel-processor/src/intercept_compose.rs`); action: the suite's `assert_file_not_contains(path, forbidden)` helper runs over the three paths with forbidden tokens `RuntimeBus`, `RuntimeQuery`, `RuntimeQueryBus`; assert: no hit on any file; command: `cargo test -p camel-core --test hexagonal_architecture_boundaries_test intercept`; expected: passes (fails if an interception module imports a query type).

**Acceptance:**
- `cargo test -p camel-core --test route_interception_test` exits 0.
- `cargo test -p camel-core --test hexagonal_architecture_boundaries_test` exits 0.
- `cargo fmt --check --all` and `cargo xtask lint-unwrap` exit 0.

- [x] 6

## docs

### Task 7: documentation surfaces (D5)

**Files:**
- `CONTEXT-MAP.md` (modified)
- `crates/camel-core/CONTEXT.md` (modified)
- `crates/camel-processor/CONTEXT.md` (modified)
- `docs/src/concepts/glossary.md` (modified)
- `docs/src/testing/index.md` (new — verified: no `docs/src/testing/` exists today, so the conditional resolves to create)
- `docs/src/SUMMARY.md` (modified — register the new guide)

**Steps:**
1. `CONTEXT-MAP.md`: add an "Interception (route send-point)" entry with relationships to Route Compilation and WireTap; one-paragraph summary referencing ADR-0064 and the freeze rule.
2. `crates/camel-core/CONTEXT.md`: document `InterceptRules`/`InterceptAction`, the freeze contract (first successful route registration or start; never unfrozen), and the compiler-context threading.
3. `crates/camel-processor/CONTEXT.md`: document `compose_divert` and the WireTap restart-reopen semantics change (including the `shutdown_called` reset).
4. `docs/src/concepts/glossary.md`: add `InterceptRule`, `SkipTo`, `DivertCopyTo` entries (one sentence each, STE).
5. `docs/src/testing/index.md`: "Route interception" section with one `SkipTo` and one `DivertCopyTo` example (builder + rules + route snippet), explicitly noting targets must be `mock:` URIs and rules freeze at first registration/start.
6. Register the guide in `docs/src/SUMMARY.md`.

**Tests:**
1. `context_citation_gate` — setup: the four CONTEXT/CONTEXT-MAP edits done; action: run the repo gate; assert: exit code 0; command: `cargo xtask lint-context-citations`; expected: passes.
2. `mdbook_build` — setup: guide + SUMMARY registration done; action: build the book; assert: exit code 0 (if mdbook is unavailable in the environment, record the skip in the final report instead of failing); command: `nix shell nixpkgs#mdbook -c mdbook build docs`; expected: passes.

**Acceptance:**
- `cargo xtask lint-context-citations` exits 0.
- `test -f docs/src/testing/index.md` succeeds.
- `rg -q 'Interception' CONTEXT-MAP.md`, `rg -q 'InterceptRules' crates/camel-core/CONTEXT.md`, `rg -q 'compose_divert' crates/camel-processor/CONTEXT.md`, `rg -q 'InterceptRule' docs/src/concepts/glossary.md`, `rg -q 'SkipTo' docs/src/concepts/glossary.md`, `rg -q 'DivertCopyTo' docs/src/concepts/glossary.md`, and `rg -q 'testing/index' docs/src/SUMMARY.md` all succeed.
- `git diff --check` exits 0 (whitespace hygiene).

- [x] 7
