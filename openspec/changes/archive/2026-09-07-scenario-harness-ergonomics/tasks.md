# Tasks: scenario-harness-ergonomics

## Phase 1: Lane FIFO, overflow taxonomy, load-time gates

Goal: silent and misclassed failures become correct-class errors (rc-qogy, rc-7mli, rc-9dpx, rc-j87j).

### camel-integration-test adapters

#### Task 1.1: ClientLane bounded per-key FIFO (rc-qogy)

**Files:**
- `crates/camel-integration-test/src/adapters/http.rs` (modified)
- `crates/camel-integration-test/src/adapters.rs` (modified — `TransportError::LaneFifoOverflow` variant lives here)
- `crates/camel-integration-test/src/adapters_test.rs` (modified, if adapter-level unit tests exist there)
- `crates/camel-integration-test/tests/http_client_lane_test.rs` (modified)

**Steps:**
1. Add `const LANE_FIFO_CAPACITY: usize = 64;` near `ARRIVAL_LANE_CAPACITY` (http.rs:104).
2. Change `ClientLane::in_flight` from `Arc<Mutex<BTreeMap<String, LaneEntry>>>` (http.rs:477) to `Arc<Mutex<BTreeMap<String, VecDeque<LaneEntry>>>>`.
3. In `ClientLane::launch` (http.rs:523): under the lock, if the key's deque has `len() >= LANE_FIFO_CAPACITY`, return the new `TransportError::LaneFifoOverflow { lane_key: String, bound: usize }` variant (add it to `TransportError`, adapters.rs:85-104); otherwise `push_back` a fresh `LaneEntry` (generation stamping unchanged).
4. Change the receive-side `take` (http.rs:580-587) to `pop_front` on the deque and remove the map entry when the deque empties.
5. Update `fail_lane_entry` (http.rs:596-603, free fn ~786) to scan the deque and remove only the entry whose `generation` matches; keep the generation-guard semantics.
6. Update the module doc (http.rs:36-40) that admits the v1 single-entry bound: document the FIFO and the apparatus-class overflow.

**Tests:** (executable spec)
- `same_key_sends_park_fifo` (tests/http_client_lane_test.rs): partner script holds responses with `delay`; three sends under key K with no intervening receives; three receives assert the response bodies in wire order A, B, C (spec scenario shape). Command: `cargo test -p camel-integration-test --features http --test http_client_lane_test same_key_sends_park_fifo`. Expected: fails before the change (third receive finds no entry / wrong body), passes after.
- `lane_fifo_overflow_is_apparatus` (same file): partner script delays all responses past the test window; issue `LANE_FIFO_CAPACITY + 1` sends under one key; assert the 65th send returns `TransportError::LaneFifoOverflow` whose Display names the lane key and `64`. Command: same invocation with filter `lane_fifo_overflow_is_apparatus`. Expected: fails before, passes after.
- Existing `failed_send_does_not_poison_later_receive` and `post_connect_failure_still_parks` (tests/http_client_lane_test.rs:51,112) must stay green. Command: `cargo test -p camel-integration-test --features http --test http_client_lane_test`.

**Acceptance:**
- `cargo test -p camel-integration-test --features http --test http_client_lane_test` exits 0.
- `cargo test -p camel-integration-test --test partner_verification_test --features http immediate_count_assert_e2e` still exits 0 (burst-send compatibility: 3 sends, 0 receives).
- `cargo fmt --check` and `cargo clippy -p camel-integration-test -- -D warnings` exit 0.

- [x] 1.1

#### Task 1.2: Arrival-lane overflow surfaces apparatus-class (rc-7mli)

**Files:**
- `crates/camel-integration-test/src/adapters/http.rs` (modified)
- `crates/camel-integration-test/src/adapters.rs` (modified)
- `crates/camel-integration-test/src/runner.rs` (modified)
- `crates/camel-cli/src/commands/test/scenario.rs` (modified — FENCE-RECORDED sole edit under `commands/test/**`: the `is_apparatus` arm)
- `crates/camel-integration-test/src/http_partner_test.rs` (modified)
- `crates/camel-integration-test/src/runner_test.rs` (modified)

**Steps:**
1. Add a `dropped: AtomicUsize` counter to the arrival-lane state created in `enqueue_arrival`'s lane construction (http.rs:813 area); on `try_send` failure (http.rs:852) increment it and keep the existing warn log.
2. In `await_arrival` (adapters.rs:370-401): when the receive times out and the lane's `dropped` count is > 0, return the new `ReceiveError::Overflow(ArrivalLaneOverflow { endpoint, dropped })` variant (add variant + struct; `endpoint: String`, `dropped: usize`) instead of `ReceiveError::Timeout`.
3. In `runner.rs`, map `ReceiveError::Overflow` to a new `ScenarioFailure::ArrivalLaneOverflow { endpoint: String, dropped: usize }` variant (`#[non_exhaustive]` enum, runner.rs:207-261); Display names the endpoint and dropped count under the class `arrival-lane-overflow`.
4. In `crates/camel-cli/src/commands/test/scenario.rs:57-63`, add `ScenarioFailure::ArrivalLaneOverflow { .. }` to the `is_apparatus` match. This is the ONLY edit under `commands/test/**` in this change (fence record in design.md).

**Tests:**
- `arrival_overflow_is_apparatus_not_receive_timeout` (src/http_partner_test.rs, feature-gated `http` — adapter-level, because `receive` is first-arrival-wins): start a scripted partner with a server-role path; a raw HTTP client loop POSTs `ARRIVAL_LANE_CAPACITY + 6` requests to it without any scenario receive (64 park, 6 drop); then call the adapter's receive in a loop `ARRIVAL_LANE_CAPACITY` times (each must succeed — parked arrivals drain); the next receive with a short deadline → `ReceiveError::Overflow(ArrivalLaneOverflow)` naming the endpoint with `dropped >= 1`, not `ReceiveError::Timeout`. Command: `cargo test -p camel-integration-test --features http --lib arrival_overflow`. Expected: fails before (surfaces `Timeout`), passes after.
- Unit: `arrival_lane_overflow_error_display` in `crates/camel-integration-test/src/runner_test.rs`: construct the failure, assert Display contains `arrival-lane-overflow`, the endpoint, and the count. Command: `cargo test -p camel-integration-test --lib arrival_lane_overflow_error_display`. Expected: fails before, passes after.
- CLI mapping: the `is_apparatus` arm is verified by the runner unit tests above plus the diff-shape acceptance criterion below (adding a CLI-side test would require an edit under the fenced `commands/test/**` area beyond the recorded arm — noted as an accepted coverage gap for the holistic review).

**Acceptance:**
- New tests pass as specified; `cargo test -p camel-integration-test --lib` exits 0.
- `cargo test -p camel-integration-test --features http --test http_partner_scripting_test` exits 0.
- `git -C <worktree> diff crates/camel-cli` shows exactly one hunk under `src/commands/test/` (the `is_apparatus` arm).
- `cargo fmt --check` and `cargo clippy -p camel-integration-test -p camel-cli -- -D warnings` exit 0.

- [x] 1.2

### camel-integration-test document loader

#### Task 1.3: Load-time validation gates for inline routes and authority-less provisioning (rc-9dpx, rc-j87j)

**Files:**
- `crates/camel-integration-test/src/document.rs` (modified)
- `crates/camel-integration-test/src/boot_scenario.rs` (modified — comment only, rejection stays as defense-in-depth)
- `crates/camel-integration-test/src/doc_parse_test.rs` (modified)

**Steps:**
1. Add `DocError::InlineRoutesRejected` (document.rs DocError enum, :455-548): raised in `parse_scenario_document` immediately after route-source selection (:643-646) when the source is `RouteSource::Inline(_)`; Display directs the author to `routeFiles` and states the exit-2 class.
2. Add `DocError::ProvisioningWithoutAuthority { endpoint: String, ref_scheme: String }`: raised during the action/provisioning conversion step (the same step (f) that raises `DocError::UnsupportedProvisioning`, :513-523 precedent) when a `provisioning: harness` entry's ref scheme is `direct` or `fake` AND the entry declares a `bindVar`; Display names the endpoint, the scheme, and the missing bound authority.
3. Keep `boot_scenario`'s inline rejection (boot_scenario.rs:198-203) as defense-in-depth; update its adjacent comment to note the load-time gate now fires first.
4. Do NOT reject `direct:`/`fake:` provisioning entries that declare no `bindVar` (fill_bind_vars skip stays legal when the var is unused).

**Tests:**
- `inline_routes_rejected_at_load` (src/doc_parse_test.rs): a doc whose route source is inline `routes:` → `parse_scenario_document` returns `DocError::InlineRoutesRejected`; Display contains `routeFiles`. Command: `cargo test -p camel-integration-test --lib inline_routes_rejected_at_load`. Expected: fails before (parser accepted it), passes after.
- `harness_provisioning_direct_bindvar_rejected_at_load` (same module): `provisioning: harness` + `direct:x` ref + `bindVar: P` → `DocError::ProvisioningWithoutAuthority` naming the endpoint and scheme. Command: same with that filter. Expected: fails before, passes after.
- `harness_provisioning_fake_without_bindvar_loads` (same module): `provisioning: harness` + `fake:x` ref with NO `bindVar` → parses Ok. Expected: passes before and after (guard against over-rejection).
- Existing `boot_inline_routes_still_rejected` (src/boot_scenario_test.rs:212) stays green. Command: `cargo test -p camel-integration-test --lib boot_inline_routes`.

**Acceptance:**
- `cargo test -p camel-integration-test --lib` exits 0 (all new + existing unit tests).
- `cargo test -p camel-integration-test --lib doc_parse` exits 0.
- `cargo fmt --check` and `cargo clippy -p camel-integration-test -- -D warnings` exit 0.

- [x] 1.3

## Phase 2: Boot root, comment-safe interpolation, sendDeadline

Goal: nested trees boot; comments never fail loads; sends bounded per document (rc-jjzy5, rc-ayke, rc-tr4w).

### camel-cli + camel-integration-test boot

#### Task 2.1: Scenario boot root is the nearest Camel.toml ancestor (rc-jjzy5)

**Files:**
- `crates/camel-integration-test/src/document.rs` (modified)
- `crates/camel-integration-test/src/boot_scenario.rs` (modified)
- `crates/camel-integration-test/src/boot_scenario_test.rs` (modified)
- `crates/camel-cli/src/commands/test.rs` (modified — outside the fenced `commands/test/**` subtree)
- `crates/camel-cli/tests/test_scenario_boot_root.rs` (new)

**Steps:**
1. Add `pub source_path: std::path::PathBuf` to `ScenarioDocument` (document.rs:48-64); set it from the `path` argument in `parse_scenario_document` (document.rs:575). Update every constructor site in the crate (struct literals in src/ and src/*_test.rs).
2. In `boot_scenario` (boot_scenario.rs:72-76): derive `doc_dir = doc.source_path.parent()`; sealed `Camel.toml` load and `routeFilesFromRoot` resolution keep using the `root` parameter; relative `routeFiles` entries in `route_patterns` (boot_scenario.rs:177-197) resolve against `doc_dir`. For flat layouts (doc next to Camel.toml) the two coincide — no behavior change.
3. In `crates/camel-cli/src/commands/test.rs` scenario branch (:392-396): call `find_camel_toml_root(parent_dir)` — it is `pub(crate)` in `commands/test/runner.rs:132` and callable from `test.rs` without editing the fenced file. On `None`, record the document as failed through the same per-doc load-failure plumbing `test.rs` already uses for unreadable documents, with the message `no Camel.toml ancestor for scenario document <path>` (exit class 2). On `Some(root)`, pass `root` where `parent_dir` was previously passed (test.rs:511).
4. New e2e `crates/camel-cli/tests/test_scenario_boot_root.rs`: follow the invocation pattern of the existing unfenced CLI tests in that directory (see `test_beans.rs`/`test_replies.rs` for the command helper).

**Tests:**
- `boot_root_walks_to_ancestor` (src/boot_scenario_test.rs): tempdir layout `root/{Camel.toml, rr/root-route.yaml}` + `root/sub/{doc.test.yaml declaring routeFiles: [local.yaml] and routeFilesFromRoot entry, local.yaml}`; `boot_scenario(doc, root, env)` succeeds and loads both route files (assert via route count or a behavior marker). Command: `cargo test -p camel-integration-test --lib boot_root_walks_to_ancestor`. Expected: fails before (routeFiles resolved against root), passes after.
- `route_files_anchor_to_doc_dir` (same module): route file exists ONLY next to the nested doc → boot succeeds (proves doc-dir anchoring); moving resolution to root would fail. Expected: fails before, passes after.
- `nested_scenario_doc_boots_via_cli` (crates/camel-cli/tests/test_scenario_boot_root.rs): fixture tree `root/{Camel.toml, sub/doc.test.yaml}`; run the CLI test command against the nested doc; assert exit 0 and the scenario passes. Command: `cargo test -p camel-cli --test test_scenario_boot_root`. Expected: fails before (exit 2 cannot-stat), passes after.
- `no_root_fails_named_exit_2` (same file): tempdir with a scenario doc and NO Camel.toml anywhere; run CLI; assert exit code 2 and stderr contains `no Camel.toml ancestor`. Expected: fails before (different error text), passes after.

**Acceptance:**
- All four tests pass; `cargo test -p camel-integration-test --lib` and `cargo test -p camel-cli --test test_scenario_boot_root` exit 0.
- `git -C <worktree> diff crates/camel-cli/src/commands/test/` is empty (fence intact; only `commands/test.rs` outside it changed).
- `cargo fmt --check` and `cargo clippy -p camel-integration-test -p camel-cli -- -D warnings` exit 0.

- [x] 2.1

### camel-dsl

#### Task 2.2: Parse-tree env interpolation — comments never fail resolution (rc-ayke)

**Files:**
- `crates/camel-dsl/src/env_interpolation.rs` (modified)
- `crates/camel-dsl/src/discovery.rs` (modified)
- `crates/camel-dsl/tests/env_escape_regression.rs` (modified) and/or `crates/camel-dsl/tests/discovery_test.rs` (modified)

**Steps:**
1. Refactor the string scanner inside `interpolate_env_with` (env_interpolation.rs:54) into a `fn interpolate_string(s: &str, lookup: &dyn Fn(&str) -> Option<String>) -> Result<String, String>` so both the legacy whole-text path and the new tree walk share one implementation (grammar `${env:X}`, `${env:X:-default}`, `$${env:X}`, `$$` unchanged).
2. Add `pub(crate) fn interpolate_env_tree(raw: &str, lookup: &dyn Fn(&str) -> Option<String>) -> Result<String, TreeInterpolateError>` where `TreeInterpolateError` is `enum { Unresolved(String), Fallback }`: parse `raw` with the crate's canonical YAML shim `noyalib::compat::serde_yaml` (the same alias `parse_yaml` uses, camel-dsl yaml.rs:5-6 — do NOT add a new serde_yaml dependency); on parse error return `Fallback`. Recursively walk mappings (string keys included) and apply `interpolate_string` ONLY to scalars whose text contains a placeholder or escape token (`${` or `$$`) — all other nodes pass through untouched, minimizing round-trip drift; an unresolved var propagates as `Unresolved(name)`. Re-serialize with the same noyalib compat shim; on serialize error return `Fallback`.
3. Documented typing semantics (design decision, camel-config precedent): an interpolated leaf that resolves to numeric/boolean-looking text KEEPS string typing after the walk (raw-splice used to re-parse it as a number; the tree walk cannot preserve plain-scalar style through the YAML Value). Consumers needing numbers compose them inside URI strings. Record this in the `env_interpolation.rs` module docs.
4. In discovery.rs:342-349, replace the direct `interpolate_env_with` call with: try `interpolate_env_tree`; on `Fallback` fall back to legacy whole-text `interpolate_env_with` (parity for text the real parse would reject anyway); on `Unresolved(name)` return the existing `DiscoveryError::Env { var_name }` unchanged.
5. Leave `camel-config` untouched (it already interpolates leaves).

**Tests:**
- `comment_placeholder_does_not_fail` (env_interpolation.rs `mod tests`, :96): input with `# TODO re-enable ${env:MISSING}` comment + valid body; lookup without `MISSING` → `interpolate_env_tree` Ok; serialized output keeps the body and drops the comment. Command: `cargo test -p camel-dsl --lib comment_placeholder`. Expected: fails before (no tree fn), passes after.
- `quoted_hash_survives_interpolation` (same): mapping `text: "a # b ${env:X}"`, X=ok → output mapping keeps `#` and interpolates X. Expected: passes after.
- `block_scalar_interpolates_as_value` (same): literal block scalar containing `${env:X}` with X=ok → X resolves inside the block content (raw-splice parity). Expected: passes after.
- `numeric_leaf_stays_string_after_interpolation` (same): mapping `port: ${env:PORT}` with PORT=8080 → the leaf parses back as the STRING `"8080"` (documented typing semantics — camel-config leaf-interpolation precedent; step 3 records it in the module docs). Expected: passes after.
- `unparseable_input_falls_back_to_legacy_env_error` (same or discovery_test.rs): input that fails the noyalib shim parse AND contains `${env:MISSING}` → discovery surfaces `DiscoveryError::Env` naming MISSING (same class as today — no behavior regression on the fallback path). Command: `cargo test -p camel-dsl --test discovery_test`. Expected: passes before and after (parity).
- Existing escape tests: `cargo test -p camel-dsl --test env_escape_regression` exits 0 before and after.

**Acceptance:**
- `cargo test -p camel-dsl` exits 0 (lib + all test targets without extra features).
- `cargo test -p camel-integration-test --lib boot_scenario` still exits 0 (scenario boot path consumes the new discovery behavior).
- `cargo fmt --check` and `cargo clippy -p camel-dsl -- -D warnings` exit 0.

- [x] 2.2

### camel-integration-test runner

#### Task 2.3: Document-level sendDeadline bound (rc-tr4w)

**Files:**
- `crates/camel-integration-test/src/document.rs` (modified)
- `crates/camel-integration-test/src/runner.rs` (modified)
- `crates/camel-integration-test/src/doc_parse_test.rs` (modified)
- `crates/camel-integration-test/src/runner_test.rs` (modified)
- `crates/camel-integration-test/src/boot_scenario_test.rs` (modified — ScenarioDocument literal updates)
- `crates/camel-integration-test/tests/partner_verification_test.rs` (modified)

**Steps:**
1. Add `pub send_deadline: Option<std::time::Duration>` to `ScenarioDocument` (document.rs:48-64); parse optional top-level `sendDeadline` via the existing `parse_duration` helper (document.rs:919-925) with field name `sendDeadline`; invalid values raise `DocError::Validation` naming `sendDeadline` (mirror the receive `deadline` required-check at :789-802, but optional). Update every `ScenarioDocument` struct literal across `src/` including the `*_test.rs` modules (compile-caught; the two tasks adding fields — this one and 2.1 — both touch literals, sequential execution keeps it cheap).
2. In `runner.rs`, thread the document bound into `send_action`: replace the `SEND_DEADLINE` constant use (:459-466) with `doc.send_deadline.unwrap_or(SEND_DEADLINE)`; keep the constant as the default and the `TransportError::Deadline { after }` mapping (apparatus `ActionTransport` class unchanged).
3. No virtual time: real `tokio::time::timeout` only (ADR-0069 §6).

**Tests:**
- `invalid_send_deadline_is_load_error` (src/doc_parse_test.rs): doc with `sendDeadline: soon` → `DocError::Validation` Display contains `sendDeadline`. Command: `cargo test -p camel-integration-test --lib invalid_send_deadline`. Expected: fails before, passes after.
- `send_deadline_bounds_hung_send` (tests/partner_verification_test.rs, feature-gated `http`): doc with `sendDeadline: 500ms` and a send to a NON-ROUTABLE address (RFC 5737 `192.0.2.1:9` — the connect phase hangs, so the deadline fires; do NOT use connection-refused, which fails fast with a different error class before the deadline); assert the action fails with the deadline error and the elapsed wall time stays under 5 s (fail-fast proof vs the 30 s default; also the tripwire if the CI network routes RFC 5737 away). Command: `cargo test -p camel-integration-test --features http --test partner_verification_test send_deadline_bounds`. Expected: fails before (30 s default, elapsed assertion), passes after.
- `absent_send_deadline_default_unchanged` (src/runner_test.rs or the nearest existing send-path unit test): a doc without `sendDeadline` keeps `SEND_DEADLINE` semantics — assert the default resolves to 30 s via the plumbing the task adds (e.g. a helper `fn effective_send_deadline(doc) -> Duration`). Command: `cargo test -p camel-integration-test --lib effective_send_deadline`. Expected: passes after.

**Acceptance:**
- New tests pass; `cargo test -p camel-integration-test --lib` exits 0; `cargo test -p camel-integration-test --features http --test partner_verification_test` exits 0.
- Suite passes WITHOUT the feature too: `cargo test -p camel-integration-test --test partner_verification_test` exits 0 (new test feature-gated).
- `cargo fmt --check` and `cargo clippy -p camel-integration-test -- -D warnings` exit 0.

- [x] 2.3

## Phase 3: expectReply vocabulary and burst-send recipe

Goal: direct replies assertable through the shared matcher algebra; concurrency recipe canonical (rc-qvz6, rc-3uihb).

### camel-integration-test vocabulary

#### Task 3.1: expectReply on direct sends, consuming camel-matchers (rc-qvz6)

**Files:**
- `crates/camel-integration-test/src/document.rs` (modified)
- `crates/camel-integration-test/src/runner.rs` (modified)
- `crates/camel-integration-test/src/adapters.rs` (modified)
- `crates/camel-integration-test/Cargo.toml` (modified)
- `crates/camel-integration-test/src/doc_parse_test.rs` (modified)
- `crates/camel-integration-test/src/runner_test.rs` (modified)
- `crates/camel-integration-test/src/boot_scenario_test.rs` (modified — ScenarioDocument literal updates)
- `crates/camel-integration-test/tests/direct_reply_test.rs` (new)

**Steps:**
1. `crates/camel-integration-test/Cargo.toml`: make `serde_json` a NON-optional dependency (today it is `optional = true`, enabled only through `http`) — the reply-value path must compile without the feature. Remove `dep:serde_json` from the `http` feature list.
2. Extend the send-action raw struct (document.rs, `RawSend`) with `expect_reply: Option<RawExpectation>`; parse it into `camel_matchers::Expectation` by REUSING the same raw-expectation parser `validate` uses (locate the existing fn that builds `Expectation` from YAML — factor it out if it is inline; do not duplicate verb parsing). Store as `ScenarioAction::Send { .., expect_reply: Option<camel_matchers::Expectation> }`.
3. Load-time gate (same step (f) as Task 1.3): `expectReply` on a send whose ref scheme is anything other than `direct` — i.e. `http`/`https` partners AND `fake:` (verified: `FakeAdapter::send` records sends and returns `Ok(())`, it produces no synchronous reply, adapters.rs:644-660) → new `DocError::ExpectReplyOnUnsupportedSend { index: usize, scheme: String }` naming the action index and the scheme.
4. `DirectStimulus::send` (adapters.rs:777-825): return the parked reply instead of dropping it — change the adapter send path to `Result<Option<camel_core::Exchange>, TransportError>`; `direct:` returns `Ok(Some(reply_exchange))`; partner and fake adapters return `Ok(None)`. Update the trait and all impls in-crate.
5. `runner.rs send_action`: when `expect_reply` is set and the adapter returned `Some(reply)`, convert the reply body to a `serde_json::Value` with a NEW feature-free helper `fn reply_body_value(exchange: &camel_core::Exchange) -> serde_json::Value` (bytes → `serde_json::from_slice`, fallback `Value::String(lossy UTF-8)`) — the existing partner-body extraction helpers are `#[cfg(feature = "http")]` and stay untouched; evaluate `camel_matchers::expectation_matches(&exp, &value)`; on false → `ScenarioFailure::ValidationMismatch` (verdict class) naming the expectation (rendered) and the actual body (via `camel_matchers::stringify`). On `None` → apparatus-class `ActionTransport` error `direct send produced no reply` (fail closed).
6. Do NOT modify `crates/camel-matchers/` — consume only.

**Tests:**
- `expect_reply_on_partner_send_is_load_error` (src/doc_parse_test.rs): send to `http` partner ref with `expectReply: {contains: x}` → `DocError::ExpectReplyOnUnsupportedSend`; Display names the action index, the scheme, and the literal `expectReply` field. Command: `cargo test -p camel-integration-test --lib expect_reply_on_partner_send`. Expected: fails before, passes after.
- `expect_reply_on_fake_send_is_load_error` (same module): `fake:` ref with `expectReply` → same error class. Expected: fails before, passes after.
- `expect_reply_matches_direct_body` (tests/direct_reply_test.rs — no http feature needed): route `direct:echo` that sets a reply body; scenario doc sends `direct:echo` with `expectReply: {contains: "ack"}`; run → pass. Command: `cargo test -p camel-integration-test --test direct_reply_test expect_reply_matches`. Expected: fails before, passes after.
- `expect_reply_mismatch_is_verdict_failure` (same file): `expectReply: {equals: {"wrong": true}}` against a known reply → run fails verdict-class with `ValidationMismatch` whose Display names the expectation and the actual body. Command: same filter `expect_reply_mismatch`. Expected: fails before, passes after.
- `expect_reply_json_subset_on_direct_body` (same file): `expectReply: {jsonSubset: {status: ok}}` against a JSON reply body → passes (proves the shared verb set, not just `contains`).

**Acceptance:**
- `cargo test -p camel-integration-test --test direct_reply_test` exits 0 with AND without `--features http`.
- `cargo test -p camel-integration-test --lib` exits 0.
- `git -C <worktree> diff crates/camel-matchers` is empty.
- `cargo fmt --check` and `cargo clippy -p camel-integration-test -- -D warnings` exit 0.

- [x] 3.1

### docs

#### Task 3.2: Burst-send concurrency recipe (rc-3uihb)

**Files:**
- `crates/camel-integration-test/README.md` (modified)
- `docs/src/testing/index.md` (modified)

**Steps:**
1. Add section `## Concurrency: the burst-send recipe` to `crates/camel-integration-test/README.md`: state the law (back-to-back `send:` actions with no intervening `receive:` dispatch concurrently; the partner recorder is the proof surface), give a worked example mirroring the `IMMEDIATE_COUNT_DOC` shape (three sends to one lane key, `sleep`, `validate: {count: 3}`), cite `tests/partner_verification_test.rs::immediate_count_assert_e2e` and `tests/http_partner_scripting_test.rs` as runnable references, and note the v1 bound (same-key responses park in a bounded FIFO — link the Phase 1 behavior) and the non-goal (no native wall-clock concurrency-comparison primitive; future consideration).
2. In `docs/src/testing/index.md` `### Scenario documents` (:250-258), add one pointer paragraph linking the README recipe section.
3. English, ASD-STE lean phrasing; no new ADR.

**Tests:** (docs — structural checks)
- `grep -n "## Concurrency: the burst-send recipe" crates/camel-integration-test/README.md` returns one line (command run in task verification).
- `grep -c "immediate_count_assert_e2e" crates/camel-integration-test/README.md` returns ≥ 1.
- `grep -n "burst-send" docs/src/testing/index.md` returns ≥ 1 line.

**Acceptance:**
- The three grep checks pass.
- The README example block is valid scenario YAML (eyeball against the grammar reference in the same file).
- No fenced-code markers broken: `grep -c '```' README.md` is even.

- [x] 3.2

## Phase 4: Inbound bound-address (rc-5yon)

Goal: inbound documents without pinned ports. LAST phase — rebase-expected; coordinate via the human before starting if pyramid step 2 landed.

### camel-integration-test inbound

#### Task 4.1: Inbound listener provisioning — port-0 staging + bindVar

**Files:**
- `crates/camel-integration-test/src/document.rs` (modified)
- `crates/camel-integration-test/src/inbound.rs` (new)
- `crates/camel-integration-test/src/lib.rs` (modified — `mod inbound;` + re-exports)
- `crates/camel-integration-test/src/runner.rs` (modified — provision inbound before boot; expose `inbound_bound` on the run outcome)
- `crates/camel-integration-test/src/doc_parse_test.rs` (modified)
- `crates/camel-integration-test/Cargo.toml` (modified)

**Steps:**
1. Recon-verify first (record in the task result): `camel_component_http::ServerRegistry::global()` (package `camel-component-http`, directory `crates/components/camel-http`, src/lib.rs:876) is a `OnceLock` static and `stage_listener` (:951) is `pub async fn` — the harness can stage pre-boot with NO camel-component-http edits. If either is false, STOP and report (design's fence-exception path applies; do not improvise).
2. `crates/camel-integration-test/Cargo.toml`: add `camel-component-http = { workspace = true, optional = true }` as a real dependency (NOT dev-dep — `src/inbound.rs` is lib code) and add `"dep:camel-component-http"` to the existing `http` feature list. Verify `cargo xtask lint-component-deps` accepts the new edge.
3. document.rs: parse optional top-level `inbound:` section — v1 grammar: a single map `inbound: {bindVar: NAME}`; reject unknown fields (mirror the partners-section strictness); store as `pub inbound: Option<InboundListener>` with `pub struct InboundListener { pub bind_var: String }`.
4. New `src/inbound.rs`: `pub async fn provision_inbound(entry: &InboundListener) -> Result<std::net::SocketAddr, CamelError>` — bind `tokio::net::TcpListener` on `127.0.0.1:0`, read `local_addr()`, call `camel_component_http::ServerRegistry::global().stage_listener(listener).await`, return the bound addr. One-shot per document (duplicate staging errors surface from the registry). All of it behind `#[cfg(feature = "http")]`.
5. runner.rs (AMENDED — the original ScenarioVars premise was wrong: action vars never feed route-file discovery interpolation): provisioning lives in the ITEST BOOT PATH — `boot_scenario` provisions when `doc.inbound` is set (feature-gated), extends a cloned `LayeredEnv`'s harness-provisioned layer with `bind_var = format!("http://{bound}")` BEFORE the discovery call, and the discovery closure reads the extended env. The bound address reaches `DocumentOutcome.inbound_bound` through the itest run flow (boot result carries it; the boot-owning itest code fills the slot). The fenced `commands/test/**` subtree receives ZERO inbound edits. If this proves impossible without CLI edits, STOP and report — the human-coordinated fence exception is the escalation, never a silent breach.
6. The no-`http` build must reject `inbound:` at load with a named doc-validation error (mirror the demand-gated activation law, ADR-0069 §8): without the feature, `inbound:` → `DocError` naming the section and the feature.

**Tests:**
- `inbound_binds_port_zero` (src/inbound.rs unit tests or runner_test.rs, feature-gated `http`): provision → returned addr is loopback with port != 0; the run outcome carries `inbound_bound == Some(addr)`. Command: `cargo test -p camel-integration-test --features http --lib inbound_binds`. Expected: fails before, passes after.
- `inbound_unknown_field_is_load_error` (src/doc_parse_test.rs, no feature): `inbound: {bindVar: X, bogus: 1}` → `DocError` naming the field. Expected: fails before, passes after.
- `inbound_without_feature_rejected_named` (src/doc_parse_test.rs, built WITHOUT `http` in the default test run): `inbound: {bindVar: X}` → `DocError` naming the section and the feature gate. Expected: fails before, passes after.
- `inbound_section_optional` (same, no feature): doc without `inbound:` parses Ok (back-compat). Expected: passes before and after.

**Acceptance:**
- New tests pass; `cargo test -p camel-integration-test --lib` exits 0 with AND without `--features http`.
- `git -C <worktree> diff crates/components/camel-http crates/camel-test` is empty (fence intact).
- `cargo fmt --check` and `cargo clippy -p camel-integration-test -- -D warnings` exit 0.

- [x] 4.1

#### Task 4.2: Migrate http_inbound_test.rs off fixed ports (rc-5yon)

**Files:**
- `crates/camel-integration-test/tests/http_inbound_test.rs` (modified)
- `crates/camel-integration-test/tests/fixtures/inbound/consumer.test.yaml` (modified)
- `crates/camel-integration-test/tests/fixtures/inbound/` (new fixture files as needed)

**Steps:**
1. Convert the main inbound e2e to the new grammar: fixture doc declares `inbound: {bindVar: INBOUND}`; the route file's consumer URI becomes `${env:INBOUND}/in` (dsl env-interpolation grammar; the variable value is the FULL URL `http://127.0.0.1:<port>` — matching the partner bindVar precedent, so the URI interpolates the whole origin); the test reads the bound address from the run outcome's `inbound_bound` field (Task 4.1 step 5) and targets it with the client send — never re-derive the port via `bound_addr` (circular under port-0).
2. Delete `CONSUMER_PORT: u16 = 18180`, `CONSUMER_ENDPOINT`, and the `PORT_GUARD` mutex (:32-40, :69) from the ephemeral path.
3. Keep exactly one fixed-port smoke variant (a second fixture pinning a high literal port, guarded if needed) proving the back-compat scenario `fixed-port inbound documents stay valid`.

**Tests:**
- Migrated `consumer_serves_on_staged_listener` (rename or keep the existing test names where possible): the doc boots, an external request to the bound address is processed by the route, assertions unchanged in substance. Command: `cargo test -p camel-integration-test --features http --test http_inbound_test`. Expected: fails before (fixture grammar unknown), passes after.
- `fixed_port_backcompat` (same file): the literal-port fixture boots and serves. Expected: passes before and after migration (the kept smoke variant).
- Parallel-safety proof: run the ephemeral test twice concurrently in the same process is implicit (two #[tokio::test] both provisioning port-0 cannot collide); no shared static port remains: `grep -c 18180 crates/camel-integration-test/tests/http_inbound_test.rs` returns 0.

**Acceptance:**
- `cargo test -p camel-integration-test --features http --test http_inbound_test` exits 0.
- `grep -c "18180" crates/camel-integration-test/tests/http_inbound_test.rs` returns 0; `PORT_GUARD` absent.
- `cargo fmt --check` and `cargo clippy -p camel-integration-test --all-targets -- -D warnings` exit 0.

- [x] 4.2
