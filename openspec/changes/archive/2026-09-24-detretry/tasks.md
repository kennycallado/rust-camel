# Tasks: detretry

## camel-component-seda

### Task 1.1: config-conflict terminal marker + typed rejection

**Files:**
- `crates/components/camel-component-seda/src/lib.rs` (modified)

**Steps:**
1. Add variant `EndpointConfigConflict` to the crate-private `enum TerminalConfigError` (currently only `MultipleConsumersWaitConflict`). Extend its `Display` match arm with deliberately non-canonical diagnostic text (e.g. `"seda terminal-config-error rejection (endpoint config conflict)"`) — never equal to a canonical rejection message.
2. Add in-crate constructor `fn endpoint_config_conflict_rejection(detail: String) -> CamelError` building `CamelError::EndpointCreationFailedWithSource(detail, OpaqueErrorSource::new(Arc::new(TerminalConfigError::EndpointConfigConflict)))` — the ONLY way this rejection is built. The `detail` is passed through byte-identical (no reformatting).
3. Convert `get_or_create_state`'s incompatibility path (currently `existing.config.is_compatible_with(config).map_err(CamelError::EndpointCreationFailed)?`) to route the `Err(String)` detail through the new constructor.
4. Update the rustdoc of `is_seda_terminal_config_error` and `is_direct_startup_race` to record the widened marker class (both the multipleConsumers+wait conflict and the endpoint config conflict reject deterministically; the conflict fires at endpoint creation, not producer call). Do not change either predicate's code — the bounded walk already matches any marker variant.
5. Verify no existing seda test pins the conflict as `is_direct_startup_race == true` or as a plain `EndpointCreationFailed` variant; update any such pinning test to the new carrier (detail text unchanged).
6. The delta's foreign-typed-source and walk-boundary scenarios are owned by the existing tests `typed_with_foreign_source_not_terminal_config`, `foreign_terminal_marker_imitation_stays_retryable`, `terminal_config_marker_at_hop_limit_classifies`, and `terminal_config_marker_beyond_limit_stays_retryable` (lib.rs ~3191-3240) — do not modify them; their assertions must remain green after the marker widens (the walk is type-level and variant-agnostic).

**Tests:** (add next to the existing marker/classification unit tests in lib.rs)
- `config_conflict_rejection_carries_terminal_marker`: build the rejection via two `SedaComponent::create_endpoint` calls on the same name with incompatible `size` params → act: run `is_seda_terminal_config_error` on the returned error → assert: true.
- `config_conflict_rejection_not_startup_race`: same arrangement → act: run `is_direct_startup_race` → assert: false.
- `config_conflict_detail_byte_identical`: `create_endpoint("seda:q?size=10")` then `create_endpoint("seda:q?size=5")` → act: destructure the returned `CamelError::EndpointCreationFailedWithSource(detail, _)` → assert: `detail == "endpoint 'q' already exists with different config: size: 10 vs 5"` (exact equality on the detail field, not a substring of the rendered Display — the historical `is_compatible_with` wording, unchanged).
- `foreign_config_conflict_imitation_stays_retryable`: a plain `CamelError::EndpointCreationFailed` whose text byte-matches the conflict wording → assert: `is_seda_terminal_config_error` false, `is_direct_startup_race` true.
- `command`: `cargo test -p camel-component-seda --lib`
- `expected`: fail before the variant/conversion exist, pass after.

**Acceptance:**
- `cargo test -p camel-component-seda --lib` passes, including all pre-existing tests (byte-identical detail keeps Display-pinning tests green) and specifically the four named walk-boundary/foreign-source tests (`typed_with_foreign_source_not_terminal_config`, `foreign_terminal_marker_imitation_stays_retryable`, `terminal_config_marker_at_hop_limit_classifies`, `terminal_config_marker_beyond_limit_stays_retryable`) unmodified.
- `cargo fmt --check` and `cargo clippy -p camel-component-seda -- -D warnings` exit 0.
- `rg -n "map_err\(CamelError::EndpointCreationFailed\)" crates/components/camel-component-seda/src/lib.rs` shows no hit on the `is_compatible_with` path (the conversion replaced it).

- [x] 1.1

## camel-cli

### Task 2.1: typed TransportFailure + deterministic classifier + loop arm

**Files:**
- `crates/camel-cli/src/commands/job/mod.rs` (modified)

**Steps:**
1. Define `enum TransportFailure` (same module, near `SendError`) with three variants: `ComponentNotRegistered { uri: String, scheme: String }`, `EndpointCreation { uri: String, cause: CamelError }`, `ProducerCreation { uri: String, cause: CamelError }`. Implement `Display` (and `std::fmt::Debug` via derive if it does not conflict) reproducing the historical strings byte-identically: `failed to send to {uri}: \`{scheme}:\` component not registered`, `failed to create endpoint {uri}: {cause}`, `failed to create producer for {uri}: {cause}`. Implement `std::error::Error` with `source()` returning the cause where present.
2. Change `SendError::Transport(String)` to `SendError::Transport(TransportFailure)`. Its consumption site (the early exit-2 branch: `tracing::error!("Job send apparatus failure: {detail}")` and `eprintln!("{detail}")`) keeps the `// log-policy: system-broken` annotation and stays Display-driven — no text change.
3. Change `attempt_send`'s outer error from `String` to `TransportFailure`: the registry-miss `ok_or_else` builds `ComponentNotRegistered`, the two `map_err` sites build `EndpointCreation` / `ProducerCreation` carrying the `CamelError` cause instead of formatting it away.
4. Add `fn is_deterministic_transport_failure(f: &TransportFailure) -> bool` with rustdoc citing bd rc-zovuy and the rc-3px7o/rc-utx98 doctrines: `ComponentNotRegistered` → true (registry frozen after boot); `EndpointCreation { cause, .. } | ProducerCreation { cause, .. }` → true when `matches!(cause, CamelError::InvalidUri(_))` (variant decides — bad URI/param parse never becomes valid) or `camel_component_seda::is_seda_terminal_config_error(cause)` (typed marker walk covering both terminal variants); else false (retryable default, foreign-component doctrine).
5. Change the `Err(detail)` arm of `send_with_startup_retry`'s loop: when `is_deterministic_transport_failure(&detail)` return `Err(SendError::Transport(detail))` immediately (before the window check — no sleep); otherwise keep the existing sleep-and-retry window semantics exactly.
6. Update `send_with_startup_retry`'s rustdoc: the transport arm now fails fast on deterministic classes (registry miss, invalid URI, seda terminal-config incl. the endpoint config conflict) and keeps the bounded window for transient plain failures.
7. Update the adjacent `is_retryable_startup_failure` rustdoc (~job/mod.rs:1955-1975): its terminal-config paragraph names only the multipleConsumers+wait conflict — widen it to the marker's full class (both the multipleConsumers+wait conflict and the endpoint config conflict, per the seda crate's widened marker).

**Tests:** (compile-only at this stage; behavior pinned in Task 3.1)
- `command`: `cargo build -p camel-cli`
- `expected`: builds clean; no consumption site needed rewriting beyond the Display-driven ones.

**Acceptance:**
- `cargo build -p camel-cli` exits 0.
- `cargo test -p camel-cli --lib --no-run` exits 0 (compiles the `#[cfg(test)]` consumers of `SendError::Transport`).
- `cargo fmt --check` and `cargo clippy -p camel-cli -- -D warnings` exit 0.
- The three historical transport strings appear unchanged in the new `Display` impl (verbatim fragments `failed to send to`, `failed to create endpoint`, `failed to create producer for`).

- [x] 2.1

### Task 3.1: classifier characterization + behavioral fail-fast tests

**Files:**
- `crates/camel-cli/src/commands/job/startup_retry_classification_tests.rs` (modified)

**Steps:**
1. Add unit characterization tests for `is_deterministic_transport_failure`: registry miss (construct `TransportFailure::ComponentNotRegistered`), `InvalidUri` cause, seda config-conflict rejection cause (build via two `create_endpoint` calls with conflicting `size` on a `SedaComponent`), and a plain `EndpointCreationFailed` cause — assert deterministic for the first three, retryable for the last.
2. Add Display byte-fidelity tests: one per `TransportFailure` variant, asserting the exact historical string (including backticks around `{scheme}:`).
3. Add in-process behavioral tests using the existing harness patterns in this file (`tick_send`, `booted_seda_context`). For the route-dependent arrangements, either start a route consuming `from: seda:q?size=10` before the send (the route-start pattern lives in the sibling `startup_retry_pipeline_tests.rs` ~192-220), or lazily pre-create the conflicting endpoint state via `SedaComponent::create_endpoint("seda:q?size=10")` — the conflict fires at state lookup, consumers irrelevant:
   - `seda_config_conflict_transport_fails_fast`: target `seda:q?size=5&waitForTaskToComplete=Always`, assert the returned error is `SendError::Transport` rendering the conflict detail and the call completes in under 1 second (a retry burn would consume the full 3 s window).
   - `invalid_uri_transport_fails_fast`: target `seda:q?size=abc` (the `InvalidUri` cause renders through the endpoint-creation stage prefix, NOT an `Endpoint creation failed:` prefix), assert `SendError::Transport` rendering substrings `failed to create endpoint` and `Invalid URI: invalid size`, and elapsed under 1 second.
   - `transient_transport_failure_keeps_retry_loop`: register a test-only component (a local `impl Component for FlakyOnceComponent` whose `create_endpoint` fails the FIRST call with a plain `CamelError::EndpointCreationFailed("flaky once")` and succeeds on subsequent calls; the registry accepts test components the way `booted_seda_context` builds its context) under an unused scheme, send to it via `send_with_startup_retry` → assert: the send SUCCEEDS (the plain failure was retried, not failed fast — `SEND_RETRY_SLEEP` is 20 ms so the second attempt arrives well inside the 3 s window) and the component observed exactly 2 endpoint-creation attempts.
4. Update the stale comment at the `component_not_found_stays_non_retryable` test (~308-310): it says registry misses are "retried unconditionally until the deadline" — exactly the behavior this change removes for the transport arm. Align it with the new first-attempt fail-fast classification (inner-arm/pipeline classification of `ComponentNotFound` stays non-retryable as before).

**Tests:** (the tests above are the deliverable)
- `transport_component_not_registered_is_deterministic`: constructed variant → classifier → true.
- `transport_invalid_uri_is_deterministic`: `EndpointCreation` with `CamelError::InvalidUri` cause → classifier → true.
- `transport_seda_config_conflict_is_deterministic`: `EndpointCreation` carrying the marker rejection → classifier → true.
- `transport_plain_creation_failure_stays_retryable`: `EndpointCreation` with plain `EndpointCreationFailed` cause → classifier → false.
- `transport_display_component_not_registered_byte_identical` / `transport_display_endpoint_creation_byte_identical` / `transport_display_producer_creation_byte_identical`: exact-string assertions.
- `seda_config_conflict_transport_fails_fast` / `invalid_uri_transport_fails_fast`: behavioral, elapsed-bounded.
- `transient_transport_failure_keeps_retry_loop`: behavioral — plain `EndpointCreationFailed` cause is retried (second attempt succeeds), pinning the bounded-window semantics the spec requires for non-deterministic classes.
- `command`: `cargo build -p camel-cli --quiet && cargo test -p camel-cli --lib`
- `expected`: fail until Task 2.1 lands (this task runs after); all pass at completion.

**Acceptance:**
- `cargo test -p camel-cli --lib` passes (new tests + all pre-existing).
- `cargo fmt --check` and `cargo clippy -p camel-cli -- -D warnings` exit 0.

- [x] 3.1

## sibling comments + subprocess e2e

### Task 4.1: comment-drift fold + transport-arm subprocess e2e

**Files:**
- `crates/camel-cli/src/commands/test/runner.rs` (modified)
- `crates/camel-cli/src/commands/test_support.rs` (modified)
- `crates/camel-integration-test/src/adapters.rs` (modified)
- `crates/camel-cli/tests/job_one_shot_test.rs` (modified)

**Steps:**
1. Update the comment at the `is_direct_startup_race` call site in `commands/test/runner.rs` (~270-279): it currently describes only the no-active-consumers gate exclusion — extend to name the terminal-config exclusion too (the predicate also fails fast on seda terminal-config rejections: the multipleConsumers+wait conflict and the endpoint config conflict, rc-rif19 + rc-zovuy). Comment-only; no code change.
2. Same fold at `commands/test_support.rs` (~41-44, the "the seda gate fails fast (rc-tgaxf)" comment) — name both exclusions.
3. Same fold at `camel-integration-test/src/adapters.rs` (~885, the "minus the no-active-consumers gate" comment) — name both exclusions.
4. Add subprocess e2e test `seda_config_conflict_job_exits_fast_apparatus_failure` in `tests/job_one_shot_test.rs`, mirroring `seda_terminal_config_job_fails_fast` (same `write_config`/`run_job` harness): route `from: seda:q?size=10`, job send `to: seda:q?size=5`, assert exit code 2, stdout does NOT parse as a JSON report (early exit-2 class), stderr contains `failed to create endpoint` AND `endpoint 'q' already exists with different config: size: 10 vs 5`, and process elapsed < 1.5 s (half the 3 s window — no retry burn).

**Tests:**
- `seda_config_conflict_job_exits_fast_apparatus_failure`: tempdir job (route `seda:q?size=10`, send `seda:q?size=5`) → `run_job` → assert exit 2, non-JSON stdout, stderr fragments `failed to create endpoint` + `endpoint 'q' already exists with different config: size: 10 vs 5`, elapsed < 1500 ms.
- `command`: `cargo build -p camel-cli --quiet && cargo test -p camel-cli --test job_one_shot_test seda_config_conflict`
- `expected`: fail before Tasks 1.1+2.1 land (elapsed would exceed budget; this task runs after both); pass at completion.

**Acceptance:**
- `cargo test -p camel-cli --test job_one_shot_test` passes (new + pre-existing).
- The three call-site comments each name both the gate and terminal-config exclusions (`rg -n "terminal-config" crates/camel-cli/src/commands/test/runner.rs crates/camel-cli/src/commands/test_support.rs crates/camel-integration-test/src/adapters.rs` shows hits; `git diff` on those three files shows comment-only changes).
- `cargo fmt --check` and `cargo clippy -p camel-cli -p camel-integration-test -- -D warnings` exit 0.

- [x] 4.1
