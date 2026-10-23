# Tasks: 381-health-probe-detail

Single-phase change. Task order is a dependency chain: 1 → 2 → 3 → 4.

NEW symbols introduced by this change (names are binding across tasks):
- Trait methods on `camel_api::HealthSource`: `liveness_report() -> HealthReport`,
  `readiness_report() -> HealthReport`, `startup_report() -> HealthReport`
  (all three `async`, defaulted).
- In `camel-config/src/context_ext.rs`: `async fn evaluate_readiness(&self) -> HealthReport`
  (private shared readiness evaluation) and
  `const LIFECYCLE_FAILED_MESSAGE: &str = "service lifecycle state: Failed"`.
- Timeout entry message format in `camel-health/src/lib.rs`:
  `"{probe} probe exceeded handler timeout"` where `probe` is `liveness`,
  `readiness`, or `startup`.

## camel-api

### Task 1.1: Add defaulted HealthSource report methods

**Files:**
- `crates/camel-api/src/health.rs` (modified)

**Steps:**
1. In `pub trait HealthSource`, add three defaulted async methods after
   `health_report`: `liveness_report` returning
   `HealthReport { status: self.liveness().await, ..Default::default() }`,
   `readiness_report` using `self.readiness().await`, `startup_report` using
   `self.startup().await`. Each gets a `///` rustdoc note stating it defaults
   to the bare-status shape with an empty `services` array and that
   detail-capable sources override it (link only public items).
2. Do NOT change `liveness`, `readiness`, `startup`, `health_report`, or any
   existing impl.

**Tests:** (in the existing `#[cfg(test)] mod tests`)
- `report_defaults_mirror_bare_status`: existing `MinimalSource` (Healthy)
  → call all three report methods → each returns status `Healthy`, empty
  `services`.
- `report_defaults_carry_unhealthy_status`: new test-local source returning
  `Unhealthy` from `readiness()` only → call `readiness_report()` → status
  `Unhealthy`, `services` empty (status-only shape preserved for non-Healthy),
  HTTP mapping unaffected (trait level, no HTTP here).

**Acceptance:**
- `cargo test -p camel-api --lib` passes.
- `cargo clippy -p camel-api -- -D warnings` exits 0.
- `RUSTDOCFLAGS="-D warnings" cargo doc -p camel-api --no-deps` exits 0.

- [x] 1.1

## camel-config

### Task 2.1: ContextHealthSource serves probe detail

**Files:**
- `crates/camel-config/src/context_ext.rs` (modified)

**Steps:**
1. Extract the probe aggregate into `async fn evaluate_readiness(&self) ->
   HealthReport`: aggregate status follows today's `readiness()` semantics
   exactly — lifecycle scan first (any state==2 → `Unhealthy`,
   short-circuit WITHOUT calling the registry), otherwise
   `health_registry.check_all().await.status`. The report's `services` array
   appends the lifecycle entries (name + mapped `ServiceStatus` +
   `LIFECYCLE_FAILED_MESSAGE` when failed) to the registry's entries WITHOUT
   folding lifecycle severity into the aggregate status — a Stopped lifecycle
   entry must not degrade the aggregate. Rewrite `readiness()` to
   `self.evaluate_readiness().await.status` (precedence and awaited checks
   unchanged). Leave `health_report()` UNCHANGED — the diagnostic endpoint
   keeps its own merge semantics.
2. Override `readiness_report()` to return `self.evaluate_readiness().await`.
3. Override `liveness_report()`: build entries from the lifecycle vector only
   — one `ServiceHealth` per entry (`name`, mapped `ServiceStatus`, message
   `Some(LIFECYCLE_FAILED_MESSAGE)` when the state is failed, `None`
   otherwise); status `Unhealthy` iff any entry failed, else `Healthy`. Do
   not consult the registry here.
4. Do NOT override `startup_report()` (stays trait default).

**Tests:** (in the existing in-file tests module; reuse `FixedCheck` /
`HealthCheckRegistry::new(Duration::from_secs(5))` scaffolding)
- `readiness_report_names_failed_lifecycle_entry_without_registry_consult`:
  registry holds a check whose `check()` flips an `AtomicBool` "consulted"
  flag; lifecycle vector holds one failed entry → call `readiness_report()`
  → report status `Unhealthy`, exactly one service entry named after the
  failed lifecycle entry with `Some(LIFECYCLE_FAILED_MESSAGE)` message,
  consulted flag still `false`.
- `readiness_report_aggregate_matches_readiness_status`: lifecycle all
  started, registry with one degraded check → `readiness().await` ==
  `readiness_report().await.status`, and the report contains that check's
  entry with its message.
- `readiness_report_stopped_entry_does_not_change_status`: lifecycle vector
  holds one stopped (0) entry, registry Healthy → report status `Healthy`.
- `liveness_report_lists_lifecycle_entries_ignores_readiness_failures`:
  lifecycle all non-failed, registry with one failed check →
  `liveness_report()` is `Healthy` and contains only lifecycle entries (the
  readiness-only failure must not appear).
- `liveness_report_failed_entry_carries_fallback_message`: one failed
  lifecycle entry → `liveness_report()` status `Unhealthy`, that entry's
  message is `Some(LIFECYCLE_FAILED_MESSAGE)`.

**Acceptance:**
- `cargo test -p camel-config --lib` passes.
- `cargo clippy -p camel-config -- -D warnings` exits 0.

- [x] 2.1

## camel-health

### Task 3.1: Probe handlers serve report bodies with fail-closed timeout synthesis

**Files:**
- `crates/camel-health/src/lib.rs` (modified)

**Steps:**
1. `/healthz`, `/readyz`, `/startupz` handlers: await
   `liveness_report()` / `readiness_report()` / `startup_report()` inside the
   existing `tokio::time::timeout`; on timeout build
   `HealthReport { status: Unhealthy, services: vec![ServiceHealth { name:
   "probe-timeout", status: ServiceStatus::Failed, message: Some(format!(
   "{probe} probe exceeded handler timeout")) }], timestamp: chrono::Utc::now() }`
   with `probe` = `liveness` / `readiness` / `startup` respectively. Status
   code mapping and `is_ok_status` unchanged. `/health` untouched.
2. Extend the existing `MockHealthSource` test helper so tests can supply
   distinct `HealthReport`s per probe domain (liveness/readiness/startup),
   and extend `HangingHealthSource` (or add a sibling) whose report methods
   hang via `std::future::pending`.

**Tests:** (red/green scope: the first six tests exercise NEW behavior and
must fail on main's handler code; the remaining compatibility tests must pass
both before and after — they pin unchanged behavior)
- `readyz_degraded_names_degraded_service`: mock source whose
  `readiness_report` is `Degraded` with one entry
  (`name: "broker"`, degraded, `Some("stale")`) → GET `/readyz` → HTTP 200,
  body status `Degraded`, `services` contains `name == "broker"` with its
  message.
- `healthz_unhealthy_names_failed_service`: mock liveness report `Unhealthy`
  with one failed entry → GET `/healthz` → HTTP 503, entry named.
- `probe_timeout_synthesizes_entry_on_all_three_probes`: hanging report
  source, short handler timeout → GET `/healthz`, `/readyz`, `/startupz` →
  each HTTP 503, status `Unhealthy`, single entry named `probe-timeout`,
  message names its own probe (contains `liveness` / `readiness` /
  `startup`).
- `ordinary_unhealthy_has_no_probe_timeout_entry`: mock readiness report
  `Unhealthy` with a named failed entry, completes in time → GET `/readyz` →
  HTTP 503, entry present, no `probe-timeout` entry.
- `healthy_detail_capable_body_lists_services`: mock readiness report
  `Healthy` with two entries → GET `/readyz` → HTTP 200, `Healthy`, both
  entries present in order.
- `serialized_keys_exact` (NEW behavior — fails on main, whose probe bodies
  always have an empty `services` array so entry keys are unassertable):
  degraded mock with one entry whose `message` is `None` → GET `/readyz` →
  assert on the RAW body: top-level keys exactly `status`, `services`,
  `timestamp`; the populated entry's keys exactly `name`, `status`,
  `message` with `"message": null`.
- `status_only_source_healthy_returns_200_empty_services` (compatibility —
  passes before and after): source overriding only the bare status methods
  (report methods on trait defaults), `readiness()` → `Healthy` → GET
  `/readyz` → HTTP 200, `Healthy`, empty `services`.
- `status_only_source_nonhealthy_preserves_empty_services` (compatibility —
  passes before and after): source overriding only `readiness()` →
  `Unhealthy` (report method default) → GET `/readyz` → HTTP 503,
  `Unhealthy`, empty `services`.
- Existing tests (`test_healthz_returns_200`, `test_readyz_unhealthy_returns_503`,
  `test_readyz_degraded_returns_200`, `healthz_timeout_returns_503`,
  `readyz_timeout_returns_503`, `startupz_timeout_returns_503`,
  `health_timeout_returns_200_with_unhealthy_report`) keep passing unchanged.

**Acceptance:**
- `cargo test -p camel-health` passes.
- `cargo fmt --check` exits 0.
- `cargo clippy -p camel-health -- -D warnings` exits 0.

- [x] 3.1

## docs and verification

### Task 4.1: Endpoint contract docs, consumer inventory, gates

**Files:**
- `docs/src/operations/health.md` (modified)
- `crates/camel-health/CONTEXT.md` (modified)
- `openspec/verification/381-health-probe-detail.json` (new)

**Steps:**
1. `docs/src/operations/health.md`: document that `/healthz`, `/readyz`,
   `/startupz` are cluster probes (liveness/readiness/startup) while
   `/health` is the diagnostic report endpoint; state that non-Healthy probe
   bodies name the offending services and messages; state the Healthy shape
   rule (pass-through; detail-capable sources list services even when
   Healthy; trait-default sources yield empty `services` for any status;
   `probe-timeout` only on handler timeout).
2. `crates/camel-health/CONTEXT.md` "Health probe contract" section: same
   contract, updated for report bodies and the timeout synthesis; keep
   `ste-writing` plain-English discipline.
3. Consumer inventory: `grep -rn "readyz\|healthz\|startupz" crates/
   docs/src --include='*.rs' --include='*.md'` — confirm
   `camel-prometheus/tests/health_test.rs`, `camel-test/tests/
   http_readiness_test.rs`, and `camel-http/src/health*.rs` do not assert
   empty `services` on non-Healthy probe bodies; fix any that do and record
   the inventory result.
4. Write `openspec/verification/381-health-probe-detail.json`: gate names,
   commands, exit codes, commit sha (same shape as
   `openspec/verification/380-jms-health-stale.json`).

**Tests:**
- `doc_guide_test`: `cd docs && mdbook test` passes with the edited page.
- Inventory: no test asserting `services == []` on a non-Healthy probe body
  remains (grep + read the three consumer sites listed in step 3).

**Acceptance:**
- `cargo xtask lint-context-citations`, `cargo xtask lint-log-redaction`,
  `cargo xtask lint-ignore` exit 0.
- `RUSTDOCFLAGS="-D warnings" cargo doc -p camel-api -p camel-config
  -p camel-health --no-deps` exits 0.
- `cd docs && mdbook test` exits 0.

- [x] 4.1
