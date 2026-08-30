# Tasks: fix-mcp-dead-registry-entry

## camel-component-mcp

### Task 1: Owner tokens and tool-registry liveness

**Files:**
- `crates/components/camel-component-mcp/src/registry.rs` (modified)
- `crates/components/camel-component-mcp/src/consumer.rs` (modified)
- `crates/components/camel-component-mcp/tests/server_registry_test.rs` (modified)
- `crates/components/camel-component-mcp/tests/server_consumer_test.rs` (modified)

**Steps:**
1. In `registry.rs`, add an `owner: std::sync::Weak<()>` field to `ToolEntry`. Do not add the field to the cloned dispatch snapshot (`ToolSnapshot` or the equivalent clone target): liveness is registry-internal. No new type alias: use `Arc<()>` / `Weak<()>` directly so no unused-alias warning can trip clippy.
2. Change `McpToolRegistry::register` to take an `owner: Weak<()>` parameter. In the duplicate branch: if the existing entry's `owner.upgrade()` is `None`, replace the entry and emit a `warn!` log naming the dead `route_id`; if the upgrade succeeds, keep today's `McpError` rejection ("tool '{name}' is already registered").
3. Before the cap check in `register`, sweep the entries map and remove every entry whose owner cannot upgrade, so dead entries under any name release their slots. A same-name replace does not grow the map and therefore bypasses the cap check.
4. Add `pub fn unregister_owned(&self, name: &str, owner: &Weak<()>) -> bool` that removes the entry only when `Weak::ptr_eq(&entry.owner, owner)`; returns whether a removal happened. Keep the existing unconditional `unregister`. Both are `pub` because `tests/server_registry_test.rs` is a separate crate.
5. Make `resolve()` and `list_ready()` skip and remove dead-owner entries before returning, so dead tools stop being advertised and stop answering.
6. Add `pub fn name_taken_by_live_owner(&self, name: &str) -> bool` returning true only when the name maps to an entry whose owner upgrades.
7. In `consumer.rs` `McpConsumer::start()`, mint `let owner = Arc::new(());`, store it as a new `owner: Arc<()>` field on the `Running` struct, and pass `Weak::downgrade(&owner)` into the tool-arm `register` call. This lands the real owner in the same task as the signature change, so the crate never builds with a dead placeholder token.
8. In `consumer.rs`, switch the tool-arm duplicate fast-path (`resolve(&name).is_some()`) to `name_taken_by_live_owner(&name)`, and switch the tool-arm `stop()` unregister to `unregister_owned` with the `Running` owner.
9. Update every in-crate and in-test caller of changed signatures (test callers pass `Arc<()>`-derived `Weak` tokens, keeping the strong `Arc` bound in a local for the test's duration: a dropped `Arc` reads as a dead owner). Cite the replace branch with `// ADR-0068: replace-dead-on-conflict` (the ADR lands in Task 5).
10. Run `cargo fmt`, then `cargo clippy -p camel-component-mcp --all-targets -- -D warnings`, then the test command below.

**Tests:** (registry tests in `tests/server_registry_test.rs` on a fresh `McpToolRegistry::new(max)` per existing style; consumer test in `tests/server_consumer_test.rs` reusing `consumer_for`/`test_context`/`:0`-bind helpers)
- `dead_owner_tool_entry_is_replaced_on_register`: register "t" with live owner A; drop A; register "t" with owner B; assert `register` returns `Ok` and `resolve("t")` returns B's route id.
- `live_duplicate_tool_registration_still_rejected`: register "t" with live owner A; register "t" with live owner B; assert `Err` whose display contains "already registered".
- `late_owner_unregister_does_not_remove_replacement`: register "t" owner A; drop A; register "t" owner B; call `unregister_owned("t", &weak_a)`; assert `resolve("t")` still returns B; call `unregister_owned("t", &weak_b)`; assert `resolve("t")` returns `None`.
- `dead_tool_entry_pruned_from_list_ready_and_cap_reclaimed`: registry with `max` 2; register "t1","t2", mark both ready; drop owner of "t1"; assert `list_ready()` names equal `["t2"]`; register "t3" with a live owner; assert `Ok` (slot reclaimed via lazy prune or prune-on-register, both in scope).
- `dead_entry_under_other_name_releases_slot_on_register`: registry with `max` 2; register "t1" owner A and "t2" owner B, mark both ready; drop A without any list/resolve call; register "t3" owner C; assert `Ok`; then assert a fourth registration "t4" owner D returns the cap-breach `Err` (proves only 2 live entries occupy the cap; `list_ready()` shows `["t2","t3"]`).
- `takeover_at_full_cap_does_not_consume_extra_slot`: registry with `max` 2 full; drop owner of "t1"; register "t1" owner C; assert `Ok`; then assert a fresh name "t4" owner D returns the cap-breach `Err` (the takeover did not add an entry).
- `restart_after_clean_stop_succeeds` (consumer-level): start consumer c1 on `mcp:srv/tool/t`; `stop().await`; start consumer c2 with the same URI; assert `Ok`. Verifies owner wiring on the clean path.
- Command: `cargo nextest run -p camel-component-mcp` (new registry tests fail before steps 1-6, pass after; existing tests stay green after the caller updates).

**Acceptance:**
- `cargo nextest run -p camel-component-mcp` exits 0.
- `cargo clippy -p camel-component-mcp --all-targets -- -D warnings` exits 0.
- `cargo fmt --check` exits 0.
- The 7 tests above exist with exactly those names.

- [x] 1.1

### Task 2: Resource-registry liveness mirror

**Files:**
- `crates/components/camel-component-mcp/src/registry.rs` (modified)
- `crates/components/camel-component-mcp/src/consumer.rs` (modified)
- `crates/components/camel-component-mcp/tests/server_registry_test.rs` (modified)

**Steps:**
1. Mirror Task 1 steps 1-6 in `McpResourceRegistry` keyed by resource URI: `owner: Weak<()>` on the entry, `register` replace-dead-on-conflict ("resource '{uri}' is already registered" on live duplicate), prune-before-cap sweep, `pub fn unregister_owned(&self, uri: &str, owner: &Weak<()>) -> bool`, dead-entry skip in `resolve()`/`list_ready()`, `pub fn uri_taken_by_live_owner(&self, uri: &str) -> bool`.
2. In `consumer.rs`, pass the `Running` owner `Weak` into the resource-arm `register` call, switch the resource-arm fast-path (`resolve(&uri).is_some()`) to `uri_taken_by_live_owner(&uri)`, and switch the resource-arm `stop()` unregister to `unregister_owned`.
3. Update test callers of changed resource-registry signatures. Run `cargo fmt`, `cargo clippy -p camel-component-mcp --all-targets -- -D warnings`, then the test command below.

**Tests:** (in `tests/server_registry_test.rs`)
- `dead_owner_resource_entry_is_replaced_on_register`: register URI "crm://x" with live owner A; drop A; register "crm://x" owner B; assert `Ok` and `resolve` returns B's route id.
- `live_duplicate_resource_registration_still_rejected`: two live owners, same URI; assert `Err` containing "already registered".
- `late_owner_resource_unregister_does_not_remove_replacement`: URI mirror of Task 1's third test (A dead, B took over, `unregister_owned` with A's weak leaves B, with B's weak removes).
- Command: `cargo nextest run -p camel-component-mcp` (resource tests fail before, pass after; Task 1 tests stay green).

**Acceptance:**
- `cargo nextest run -p camel-component-mcp` exits 0.
- `cargo clippy -p camel-component-mcp --all-targets -- -D warnings` exits 0.
- `grep -nE "resolve\(&(name|uri)\)\.is_some\(\)" crates/components/camel-component-mcp/src/consumer.rs` returns no matches (both fast-path arms replaced).

- [x] 2.1

### Task 3: Bind-security plan ownership

**Files:**
- `crates/components/camel-component-mcp/src/registry.rs` (modified)
- `crates/components/camel-component-mcp/src/consumer.rs` (modified)
- `crates/components/camel-component-mcp/tests/server_registry_test.rs` (modified)

**Steps:**
1. In `McpBindSecurity`, change the plans map value to a struct holding the `RouteSecurityPlan` plus `owner: Weak<()>`. `McpBindSecurity::new()` stays private; tests reach a security instance through `McpServerRegistry::global().bind_security("<unique-never-bound-addr>")` (pub, lazy, opens no socket).
2. Add an `owner: Weak<()>` parameter to `register_plan`: when the incumbent plan's owner upgrades, keep the incumbent and return without error (the newcomer fails the duplicate guard later in `start()`); when the incumbent owner is dead, overwrite.
3. Add `pub fn unregister_plan_owned(&self, route_id: &str, owner: &Weak<()>) -> bool` mirroring Task 1 step 4. Make `plan_for` and `plans_snapshot()` (the exposure-gate input, consumer.rs start step b2) treat a dead-owner plan as absent, so a dead route's plan stops influencing the bind exposure gate. Keep the existing unconditional `unregister_plan`.
4. In `consumer.rs`, pass the `Running` owner `Weak` into `register_plan` at start step (b2); switch `stop()` and every failure-path cleanup in `start()` (duplicate rejection at both arms, resource-register error) to `unregister_plan_owned`, so a failed duplicate start cannot remove or overwrite the incumbent's plan.
5. Run `cargo fmt`, `cargo clippy -p camel-component-mcp --all-targets -- -D warnings`, then the test command below.

**Tests:** (in `tests/server_registry_test.rs`, using `bind_security` on a unique address per test)
- `live_owner_plan_not_overwritten_and_dead_plan_replaced`: `register_plan("r1", planA, owner A)`; `register_plan("r1", planB, owner B)`; assert `plan_for("r1")` returns planA; drop A; assert `plans_snapshot()` no longer contains "r1" (dead plan leaves the exposure-gate input); `register_plan("r1", planB, owner B)`; assert `plan_for("r1")` returns planB; then `unregister_plan_owned("r1", &weak_a)`; assert `plan_for("r1")` still returns planB (late stop of the dead owner keeps the replacement's plan).
- `failed_unregister_plan_keeps_incumbent`: `register_plan("r1", planA, owner A)` with A live; `unregister_plan_owned("r1", &weak_b)`; assert `plan_for("r1")` returns planA.
- Command: `cargo nextest run -p camel-component-mcp` (plan tests fail before, pass after; Tasks 1-2 tests stay green).

**Acceptance:**
- `cargo nextest run -p camel-component-mcp` exits 0.
- `cargo clippy -p camel-component-mcp --all-targets -- -D warnings` exits 0.
- The 2 tests above exist with exactly those names.

- [x] 3.1

### Task 4: Crash regression tests

**Files:**
- `crates/components/camel-component-mcp/tests/server_consumer_test.rs` (modified)

**Steps:**
1. Add the tests below using the existing crash seams: obtain the bridge via `background_task_handle()` and abort it, then drop the consumer without `stop()`. A literal bridge-panic injection is not available without a test-only seam; abort+drop reproduces the runtime leak windows (see design.md). `Registration` is Tool XOR Resource per consumer, so the takeover test drives a tool consumer and a resource consumer separately.
2. For the listing test, clone the `Arc<McpListenerHandle>` from the registry before dropping the consumer, then read `handle.tool_registry.list_ready()` after the drop.
3. Run the test command below; all tests must pass against the Tasks 1-3 implementation.

**Tests:** (in `tests/server_consumer_test.rs`, unique `:0` binds per test)
- `aborted_bridge_and_dropped_consumer_same_name_restart_succeeds`: start tool consumer c1a on `mcp:srv/tool/t` and resource consumer c1b on `mcp:srv/resource/r` (URI `crm://x`), same bind; abort `background_task_handle()` of both; drop both without `stop()`; start c2a on the tool URI and c2b on the resource URI; assert both `start()` calls return `Ok`; assert an end-to-end `tools/call` for "t" through the `test_context` receiver answers from c2a, and a `resources/read` for `crm://x` dispatches to c2b's route.
- `dropped_consumer_without_stop_releases_name`: start c1 on `mcp:srv/tool/t`; drop c1 without stop and without aborting anything; start c2 on the same URI; assert `Ok`. Documents that channel liveness alone would not fix this case (the bridge stays detached).
- `live_duplicate_consumer_rejected`: start c1 on a bind with security plan planA for its route id; start c2 with the same tool name on the same bind declaring a distinct security plan planB for the same route id, while c1 is alive; assert c2 `start()` returns `Err` containing "already registered"; then assert `handle.security.plan_for(<c1 route id>)` returns a plan equal to planA (not planB: the failed start neither removed nor overwrote the incumbent's plan); stop c1 cleanly.
- `dead_owner_tool_absent_from_list_ready_via_listener_handle`: start c1, mark ready; clone the listener handle; drop c1 without stop; assert `handle.tool_registry.list_ready()` contains no entry for c1's tool name.
- Command: `cargo nextest run -p camel-component-mcp` (tests 1-2 fail before Tasks 1-2, pass after; test 3 guards existing behavior plus the Task 3 plan invariant; test 4 fails before Task 1's lazy prune).

**Acceptance:**
- `cargo nextest run -p camel-component-mcp` exits 0.
- `cargo check -p camel-component-mcp --all-targets` exits 0.
- `cargo test -p camel-component-mcp` exits 0.
- The 4 tests exist with exactly those names.
- `cargo clippy -p camel-component-mcp --all-targets -- -D warnings` exits 0.

- [x] 4.1

### Task 5: ADR and context refresh

**Files:**
- `docs/adr/0068-mcp-registry-owner-liveness.md` (new)
- `crates/components/camel-component-mcp/CONTEXT.md` (modified)
- `CONTEXT-MAP.md` (modified)

**Steps:**
1. Write `docs/adr/0068-mcp-registry-owner-liveness.md` following the ADR-0067 header convention (`# ADR-0068: MCP Registry Owner-Liveness Entries`, `**Date:** 2026-08-29`, `**Status:** Accepted`, `**Origin:** bd rc-apvm`, `**Cross-refs:** ADR-0007, ADR-0060`, then Context / Decision / Consequences). Decision: registry entries and route security plans carry an owner-liveness `Weak<()>` token minted by `McpConsumer::start()`; `register()` replaces dead-owner duplicates and prunes dead entries before cap enforcement; unregister is owner-conditional. Record the rejected alternatives with one reason each: lease/heartbeat (timers plus self-supervision, contra ADR-0007), pid-scoped keys (registry is an in-memory process-global singleton), `sender.is_closed()` (bridge detaches on drop-without-stop, channel stays open), Drop-guard (runtime takes the bridge handle; still needs owner-conditional unregister).
2. In the crate `CONTEXT.md`, refresh the registry section that currently states "duplicate registration is rejected atomically": state the owner-liveness semantics (dead-owner entries replaced on re-registration, owner-conditional unregister) and cite `ADR-0068`.
3. In `CONTEXT-MAP.md`, update the Key Terms entries "Tool Registry" (line ~177) and "Resource Registry" (line ~178): entries are scoped to a live owner, dead owners are replaced on re-registration, and add `ADR-0068` next to `ADR-0060`.
4. Run `cargo xtask lint-context-citations` from the worktree root to confirm citation integrity.

**Tests:**
- `lint-context-citations`: ADR-0068 exists with Cross-refs citing ADR-0007 and ADR-0060 → `cargo xtask lint-context-citations` exits 0.
- `adr-mentions-rejected-alternatives`: `grep -c "heartbeat\|pid-scoped\|is_closed\|Drop-guard" docs/adr/0068-mcp-registry-owner-liveness.md` returns 4 or more.

**Acceptance:**
- `cargo xtask lint-context-citations` exits 0.
- `grep -q "ADR-0068" CONTEXT-MAP.md` exits 0 and `grep -q "ADR-0068" crates/components/camel-component-mcp/CONTEXT.md` exits 0.

- [x] 5.1
