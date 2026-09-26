# Tasks: journstart2

## camel-core lifecycle — boot-unique command IDs

### Task 1.1: Journal-derived boot nonce with fail-closed boundary

**Files:**
- `crates/camel-core/src/lifecycle/application/ports/runtime_ports.rs` (modified)
- `crates/camel-core/src/lifecycle/adapters/in_memory.rs` (modified)
- `crates/camel-core/src/lifecycle/application/runtime_bus.rs` (modified)
- `crates/camel-core/src/lifecycle/application/context_lifecycle.rs` (modified)

**Steps:**
1. In `runtime_ports.rs`, add a defaulted method to `RuntimeUnitOfWorkPort` (beside the existing defaulted `recover_from_journal`):
   `async fn recovered_boot_nonce(&self) -> Result<u64, DomainError> { Ok(0) }`
   Doc comment states: journal-derived boot nonce, meaningful only after `recover_from_journal`; default `Ok(0)` for stores without durable journal support; `Err` means the deterministic nonce space is exhausted (an adversarial recorded command ID) and the error message names the offending recorded ID and instructs the operator to clean or rotate the journal.
2. In `in_memory.rs`, add a private pure helper:
   `fn derive_boot_nonce<'a>(ids: impl IntoIterator<Item = &'a str>) -> Result<u64, DomainError>`
   Rule: for each recorded command ID, split from the right on `':'`; if the ID has at least two segments and BOTH final two segments parse as `u64`, collect the penultimate value into `P`. Then: `P` empty → `Ok(0)`; else `m = max(P)`; `m == u64::MAX` → `Err(DomainError::InvalidState(...))` whose message contains the full offending recorded ID verbatim and the phrase `clean or rotate the journal`; otherwise `Ok(m + 1)`. Use `str::rsplit`-style right-to-left parsing so legacy colon-containing route IDs are handled by construction (no segment counting from the left).
3. In `in_memory.rs`, implement `recovered_boot_nonce` for `InMemoryRuntimeStore`: under the same lock guard the other methods use, run `derive_boot_nonce` over every command ID currently in the `seen` set (after replay, `seen` holds exactly the replayed durable command IDs). No other behavior changes.
4. In `runtime_bus.rs`: change field `journal_recovered_once: OnceCell<()>` to `OnceCell<u64>` (constructor `OnceCell::new()` stays). In `ensure_journal_recovered`, inside `get_or_try_init`, after `uow.recover_from_journal().await?`, add `let nonce = uow.recovered_boot_nonce().await?;` and return `Ok(nonce)` from the closure. Add `pub(crate) fn boot_nonce(&self) -> u64 { self.journal_recovered_once.get().copied().unwrap_or(0) }` (unrecovered/no-uow case yields 0).
5. In `context_lifecycle.rs`: change `next_context_command_id(op: &str, route_id: &str)` to `next_context_command_id(boot_nonce: u64, op: &str, route_id: &str)` emitting `format!("context:{op}:{route_id}:{boot_nonce}:{seq}")`; `seq` still comes from `CONTEXT_COMMAND_SEQ` (unchanged semantics: uniqueness within a boot). Update the doc comment to document the five-segment shape and that `boot_nonce` scopes IDs per boot against the durable dedup store. Update all three call sites (start, stop, abort-stop) to pass `runtime.boot_nonce()` (each already holds `runtime: &RuntimeBus`).
6. Run `cargo fmt` and `cargo clippy -p camel-core -- -D warnings` in the worktree; fix findings.

**Tests** (unit tests in the existing `#[cfg(test)] mod tests` of `in_memory.rs`, next to the other `recover_from_journal` tests; they exercise the pure helper directly):
- `derive_boot_nonce_empty_is_zero`: no recorded IDs → `derive_boot_nonce([])` returns `Ok(0)`.
- `derive_boot_nonce_strictly_above_recorded_penultimates`: IDs `["context:start:r:7:0", "context:start:r:3:5", "context:stop:r:7:1"]` → `Ok(8)` (1 + max penultimate 7).
- `derive_boot_nonce_ignores_non_numeric_tails`: IDs `["context:start:hello:0", "context:start:foo"]` (legacy four-segment whose final-two rule fails: `hello`/`0` — `hello` does not parse) → `Ok(0)`.
- `derive_boot_nonce_legacy_colon_route_is_forbidden`: ID `"context:start:foo:0:0"` (legacy write for route `foo:0`) → penultimate `0` is collected → `Ok(1)`, never `Ok(0)`.
- `derive_boot_nonce_max_penultimate_fails_closed`: ID `"context:start:r:18446744073709551615:0"` → `Err` whose `Display` contains the full offending ID `"context:start:r:18446744073709551615:0"` and the phrase `"clean or rotate the journal"`.

**Acceptance:**
- `cargo test -p camel-core --lib derive_boot_nonce` exits 0 (5 new tests).
- `cargo test -p camel-core --lib` exits 0 (no regression in existing unit tests).
- `cargo clippy -p camel-core -- -D warnings` exits 0.
- `cargo fmt --all --check` exits 0.

- [x] 1.1

### Task 1.2: No-silence guard in start_context

**Files:**
- `crates/camel-core/src/lifecycle/application/context_lifecycle.rs` (modified)

**Steps:**
1. Extract the guard into two `pub(crate)` testable helpers in `context_lifecycle.rs`:
   - `pub(crate) fn warn_suppressed_starts(results: &[(String, String, RuntimeCommandResult)])` — for each `(route_id, command_id, result)` where `matches!(result, RuntimeCommandResult::Duplicate { .. })`, emit `warn!(route_id = %route_id, command_id = %command_id, "StartRoute suppressed as duplicate command")`.
   - `pub(crate) fn warn_non_started_routes(statuses: &[(String, String)])` — for each `(route_id, status)` where `status != "Started"`, emit `warn!(route_id = %route_id, status = %status, "auto-startup route not Started after start sequence")`.
2. In `start_context`: collect `(route_id, command_id, result)` for every `execute(StartRoute)` in the auto-startup loop (bind the issued command ID to a local before the call so the warn can name it; keep the existing `?` on `Err` unchanged). After the loop, call `warn_suppressed_starts(&collected)`.
3. After step 2's call, fetch the status of each route from the same `auto_startup_route_ids()` list via the runtime bus query path (`RuntimeQuery::GetRouteStatus { route_id }`), build the `(route_id, status)` pairs, and call `warn_non_started_routes(&statuses)`. The sweep MUST NOT fail the boot: match the query result — on `Ok` with the expected `RouteStatus` shape, use its status string; on a query `Err` or an unexpected response shape, emit `warn!(route_id = %route_id, error = %e, "auto-startup route status unavailable after start sequence")` and continue with the next route. No `?` propagation anywhere in the sweep.
4. Severity stays `warn!` — no error propagation, no behavior change beyond the log emissions. Routes with `auto_startup = false` are never in `auto_startup_route_ids()` and need no code.
5. Run `cargo fmt` and `cargo clippy -p camel-core -- -D warnings`; fix findings.

**Tests** (unit tests in a `#[cfg(test)] mod` at the bottom of `context_lifecycle.rs`; capture WARN output by installing a capturing subscriber for the test's scope — `tracing-subscriber` is a regular dependency of `camel-core`; there is no existing capture pattern in this crate's tests, so build one with `tracing_subscriber::fmt().with_writer(<capturing MakeWriter over Arc<Mutex<Vec<u8>>>>).with_max_level(tracing::Level::WARN)` plus a `set_default` dispatcher guard inside the test):
- `warn_suppressed_starts_names_route_and_command`: capture subscriber active; call `warn_suppressed_starts(&[("r1".into(), "context:start:r1:0:0".into(), RuntimeCommandResult::Duplicate { command_id: "context:start:r1:0:0".into() })])` → captured output contains `"r1"` and `"context:start:r1:0:0"`.
- `warn_suppressed_starts_silent_on_accepted`: same setup, entry with `RuntimeCommandResult::Accepted` → captured output contains no WARN for that entry.
- `warn_non_started_routes_names_route`: call `warn_non_started_routes(&[("r2".into(), "Registered".into())])` → captured output contains `"r2"`.
- `warn_non_started_routes_silent_on_started`: entry `("r3".into(), "Started".into())` → no WARN captured.
- `warn_guards_silent_when_no_entries`: both helpers called with empty slices → no WARN captured.
- `warn_silent_for_auto_startup_disabled_route`: with the capture subscriber active, build a context WITHOUT a journal (in-crate builder), add one route with `.with_auto_startup(false)`, run `start()` to completion → no suppression or not-started WARN is captured for that route (proves the `auto_startup = false` exclusion end-to-end, not just empty-input silence).

Rationale note for reviewers: driving `Duplicate` through the full public boot path is impossible once Task 1.1 is correct (the tail-scan rule is airtight by design — that IS the fix), so the guard's observable scenarios are covered at the helper seam with real `RuntimeCommandResult` values; the boot-cycle test in Task 1.3 covers the fix path through `start_context` — guard emission logic is covered only at the helper seam, by design.

**Acceptance:**
- `cargo test -p camel-core --lib warn_` exits 0 (6 new tests, including `warn_guards_silent_when_no_entries` and `warn_silent_for_auto_startup_disabled_route`).
- `cargo test -p camel-core --lib` exits 0.
- `cargo clippy -p camel-core -- -D warnings` exits 0.
- `cargo fmt --all --check` exits 0.

- [x] 1.2

### Task 1.3: Boot-cycle regression, boundary, and legacy integration tests

**Files:**
- `crates/camel-core/tests/runtime_journal_test.rs` (modified)

**Steps:**
1. Add the boot-cycle regression test (drop-before-reopen pattern of `accepted_command_id_survives_restart`, `new_journal` helper, `JournalDurability::Eventual` for speed): tempdir journal path; boot 1: build the context over the journal-backed store, register one route — `RouteDefinition` defaults `auto_startup=true`; use `.with_auto_startup(true)` explicitly — `start()`, assert route status `Started` and that the journal event sequence for that route contains `RouteRegistered` → `RouteStartRequested` → `RouteStarted` in order; drop the context (releases the redb lock); boot 2 and boot 3 on the same journal path with the same assertions, plus per boot the shutdown stop is accepted and appends `RouteStopped`.
2. Add the fail-closed boundary test: build a journal-backed `InMemoryRuntimeStore` + `RuntimeBus` manually (pattern of `runtime_bus_recovers_projection_from_journal_on_first_query`); execute a register command whose `command_id` is the adversarial `"context:start:r:18446744073709551615:0"` so it is recorded durably; drop; rebuild the store over the same journal; `recover_from_journal().await` then `recovered_boot_nonce().await` → `Err` whose message contains the adversarial ID and `"clean or rotate the journal"`. Then prove the boot fails closed end-to-end: on the recovered bus, `execute(RegisterRoute { .. })` → the call returns `Err` (the recovery failure propagates at every entry point, so no context command ID is issued for that boot).
3. Add the legacy-journal interop test: seed a journal by executing register/start/stop commands with legacy four-segment `command_id`s including `"context:start:foo:0:0"` (route `foo:0` — colon-containing) and `"context:start:hello:0"`; drop; rebuild; `recovered_boot_nonce().await` → `Ok(n)` with `n >= 1` (never 0, because the `foo:0` legacy ID forbids nonce 0); then run a full auto-start cycle through a fresh context over that journal → route reaches `Started` (no suppression).
4. Add the determinism test: after seeding a journal with any tail-numeric recorded IDs, two sequential drop/rebuild/recover cycles derive the same `recovered_boot_nonce` value.
5. Add the no-journal parity test: build a context WITHOUT a runtime journal, register one `auto_startup` route, `start()` → route status `Started` and lifecycle outcomes unchanged (no recovery path involved; command IDs gain only the constant zero nonce segment).
6. Run the full journal battery plus the neighboring batteries to catch regressions: `cargo test -p camel-core --test runtime_journal_test --test runtime_durable_boundary_test --test runtime_idempotency_test --test runtime_replay_test --test runtime_consistency_test` with `TMPDIR=/home/shared/tmp`.

Reopen rule (applies to steps 1-4): dropping the context alone does not guarantee the redb lock is released. Before re-opening the journal, drop EVERY live handle to it — the context, the store, and any locally held `Arc` journal clones (check every binding introduced by the current boot's block; scope them so they die before the reopen). Only then call `new_journal` again.

**Tests** (the five above, as `#[tokio::test]` fns):
- `auto_start_route_starts_on_every_boot_with_journal`: 3 boots × (Started + ordered `RouteRegistered`/`RouteStartRequested`/`RouteStarted` + accepted stop appending `RouteStopped`). Fails on unfixed main (boot 2+ never reaches `RouteStartRequested`); passes after Task 1.1.
- `boot_nonce_fails_closed_on_max_penultimate`: seeded adversarial ID → recovery yields `Err` naming the ID; asserts the exact ID string and `"clean or rotate the journal"` appear in the error message, and that a subsequent `execute(RegisterRoute { .. })` on the recovered bus returns `Err` (boot fails closed, no command ID issued).
- `legacy_journal_boots_without_suppression`: seeded legacy IDs (including colon route) → nonce avoids all recorded penultimates (`n >= 1` given the `foo:0` seed) and a fresh context reaches `Started` on that journal.
- `boot_nonce_deterministic_across_recoveries`: two drop/rebuild/recover cycles over the same journal → identical nonce values.
- `no_journal_boot_lifecycle_unchanged`: context without journal → `start()` → `Started`.

**Acceptance:**
- `cargo test -p camel-core --test runtime_journal_test` exits 0 (5 new tests pass).
- The five-battery command in step 6 exits 0.
- `cargo clippy -p camel-core --all-targets -- -D warnings` exits 0.
- `cargo fmt --all --check` exits 0.

- [x] 1.3
