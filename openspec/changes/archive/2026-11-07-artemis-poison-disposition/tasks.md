# Tasks: artemis-poison-disposition

Single-phase change. Task order is the implementation order: fixture query
(1.1) → observation plumbing (2.1) → test wiring (2.2) → docs (3.1).

## camel-test fixture (broker-side query)

### Task 1.1: Artemis queue-stat query with exec deadline and fail-closed parser

**Files:**
- `crates/camel-test/tests/support/artemis.rs` (modified)

**Steps:**
1. Add `#[derive(Debug, Clone, Copy, PartialEq, Eq)] pub struct QueueStat { pub message_count: u64, pub messages_added: u64 }` to `artemis.rs`.
2. Add `fn run_with_deadline(command: &mut std::process::Command, deadline: std::time::Duration) -> Result<std::process::Output, String>`: `stdin(Stdio::null())`, `stdout`/`stderr` piped, spawn, loop `try_wait` with 50 ms sleeps until the deadline; on expiry `kill()` then `wait()` and return `Err("deadline expired: {program} {args}")` interpolating the spawned program path and its arguments; on normal exit reap with `child.wait_with_output()` (returns once exited; drains the piped stdout/stderr). Never call `Command::output()` on the spawned command (it would re-execute the program). Do NOT capture stderr into returned error strings beyond the first 200 chars (the artemis CLI prints broker-side text).
3. Add `pub fn parse_queue_stat(stdout: &str, queue: &str) -> Result<Option<QueueStat>, String>`: trim each line and ignore lines that do not start with `|` (the CLI prints a `Connection brokerURL = ...` preamble line); for `|`-lines strip the enclosing `|` delimiters (leading AND trailing) before `split('|')`, then trim every cell (otherwise cell 0 is an empty string); the default 2.36.0 output wraps column names across TWO header lines — line 1 cells are `NAME, ADDRESS, CONSUMER, MESSAGE, MESSAGES, DELIVERING, MESSAGES, SCHEDULED, ROUTING, INTERNAL` and line 2 carries the qualifiers (`COUNT`, `COUNT`, `ADDED`, ...) — so validate the pair: header line 1 cell 3 == `MESSAGE` with header line 2 cell 3 == `COUNT`, and header line 1 cell 4 == `MESSAGES` with header line 2 cell 4 == `ADDED`; any mismatch is `Err` naming the unexpected header (positional guard if the pin ever changes); empty stdout or a missing/damaged header pair is `Err("no queue stat header found")`, never `Ok(None)`; data rows MUST have at least 10 cells — fewer is `Err("malformed queue stat row: {line}")` interpolating the line; when cell 0 equals `queue`, parse cell 3 as `message_count` and cell 4 as `messages_added` via `str::trim().parse::<u64>()` — a parse failure is `Err` naming the cell and line; rows for other queues are skipped. **Wrapped-name continuation rows** (empirically evidenced on the pinned image: the CLI caps column width near 25 chars and splits longer queue names across two physical lines, e.g. `|queue.test-redelivery-dis|queue.test-redelivery-dis|   0 | 0 | ...` followed by `|  position-artemis        |  position-artemis        |      |   | ...`) — a data row whose cells 3 AND 4 are empty strings is a continuation of the immediately preceding numeric row: append its trimmed cell 0 to the pending row's name (cell 1 is the wrapped ADDRESS remainder and is ignored — only NAME identifies the queue); when the merged name equals `queue`, return the pending numeric counts as `Ok(Some(..))`. A continuation row with no pending numeric row is `Err` (malformed). Only AFTER a validated header pair, no matching row → `Ok(None)`. Column order confirmed against Artemis 2.36.0 `StatQueue.java` (NAME, ADDRESS, CONSUMER_COUNT, MESSAGE_COUNT, MESSAGES_ADDED, DELIVERING_COUNT, MESSAGES_ACKED, SCHEDULED_COUNT, ROUTING_TYPE, INTERNAL); observed pinned-image rows: `|DLQ |DLQ    |   0    |   0   |   0    |    0     |   0    |    0    |ANYCAST| false  |`.
4. Add `impl ArtemisBroker { pub async fn queue_stat(&self, queue: &str) -> Result<QueueStat, String> }`: build `docker exec <container id> /var/lib/artemis-instance/bin/artemis queue stat --url tcp://localhost:61616 --user artemis --password artemis --queueName <queue>` as a sync `std::process::Command`, run under `run_with_deadline` with a 60 s deadline, `Err` when exit status is non-zero (include trimmed stderr head), run `parse_queue_stat` on stdout, map `Ok(None)` to `Err("queue stat returned no row for queue <queue>")` (absence is an error for this fixture, never zero counts), and expose the sync call through `tokio::task::spawn_blocking`.
5. Document the mechanism in doc comments on **every new pub item** (`QueueStat`, `parse_queue_stat`, `queue_stat`) plus `run_with_deadline` (private, still documented per module convention): docker exec + the container's own instance CLI, pin-scoped stability (verified on `apache/activemq-artemis:2.36.0-alpine` only: no console wars deployed, so no Jolokia exists), credentials visible in container `ps` (test-only artemis/artemis), and what the counts do not prove.

**Tests:** (all live in a `#[cfg(test)] mod queue_stat_tests` inside `artemis.rs`; run via `cargo test -p camel-test --features integration-tests --test jms_test queue_stat_tests` — a precise module-path filter, no live-broker test collides with it)
- `parse_queue_stat_reads_pinned_row`: setup — the exact observed 2.36.0 filtered output for `DLQ` including the `Connection brokerURL` preamble line and the two wrapped header lines → action — `parse_queue_stat(stdout, "DLQ")` → assert `Ok(Some(QueueStat { message_count: 0, messages_added: 0 }))` and a second synthetic row with counts `7`/`9` parses to `7`/`9`.
- `parse_queue_stat_absent_row_is_none`: setup — preamble + BOTH validated header lines, no data row → action — `parse_queue_stat(stdout, "DLQ")` → assert `Ok(None)`.
- `parse_queue_stat_empty_output_is_err`: setup — empty string → action — parse → assert `Err` (never `Ok(None)` on empty output).
- `parse_queue_stat_missing_second_header_is_err`: setup — only the first header line, then a data row → action — parse → assert `Err` naming the missing header.
- `parse_queue_stat_swapped_columns_is_err`: setup — header lines with cells 3/4 swapped (`MESSAGES` before `MESSAGE`) → action — parse → assert `Err` naming the unexpected header.
- `parse_queue_stat_malformed_row_is_err`: setup — a data row with 4 cells → action — parse → assert `Err` whose message contains `malformed queue stat row`.
- `parse_queue_stat_non_numeric_count_is_err`: setup — a full-width row whose cell 3 is `many` → action — parse → assert `Err` naming the non-numeric cell.
- `parse_queue_stat_merges_wrapped_name_rows`: setup — the exact observed pinned-image filtered output for the 41-char queue name `queue.test-redelivery-disposition-artemis` (numeric row with truncated name `queue.test-redelivery-dis` and counts `2`/`5`, then the continuation row `|  position-artemis       |  position-artemis       |      |     |` with empty cells 3/4) → action — parse → assert `Ok(Some(QueueStat { message_count: 2, messages_added: 5 }))`.
- `parse_queue_stat_orphan_continuation_is_err`: setup — validated headers, then an empty-cells-3/4 row with no preceding numeric row → action — parse → assert `Err` (malformed).
- `run_with_deadline_kills_sleeping_child`: setup — `sh -c "sleep 30"` with a 300 ms deadline → action — run → assert `Err` containing `deadline expired` and elapsed time under 10 s (kill + reap worked).
- `run_with_deadline_returns_output_in_time`: setup — `sh -c "echo stat-ok"` with a 5 s deadline → action — run → assert `Ok` with stdout containing `stat-ok`.

**Acceptance:**
- `cargo test -p camel-test --features integration-tests --test jms_test queue_stat_tests` passes (0 tests matched means the feature flag was omitted — never count that as a pass).
- `cargo fmt --check --all` exits 0; `cargo clippy -p camel-test --features integration-tests --all-targets -- -D warnings` exits 0; `cargo xtask lint-unwrap` exits 0 (no new `unwrap`/`expect` outside the existing panic style).
- Tests fail to compile before steps 1–4 land (TDD red), pass after.

- [x] 1.1

## camel-test disposition cases (wiring)

### Task 2.1: ArtemisDestination observation and precedence classification

**Files:**
- `crates/camel-test/tests/support/jms_disposition_cases.rs` (modified)

**Steps:**
1. Add `struct ArtemisDestination { source: QueueStat, dlq: QueueStat, expiry: QueueStat }` (import `QueueStat` from `super::support::artemis`) with methods: `acknowledged_and_removed()` — `source.message_count == 0 && dlq.messages_added == 0 && expiry.messages_added == 0`; `native_dead_lettered()` — `source.message_count == 0 && dlq.messages_added > 0`; `native_expired()` — `source.message_count == 0 && dlq.messages_added == 0 && expiry.messages_added > 0`; `describe()` — a line starting with `source MESSAGE_COUNT={n} DLQ MESSAGES_ADDED={n} ExpiryQueue MESSAGES_ADDED={n}`, then the outcome phrase in parentheses, exactly one of: `acknowledged and removed by the bridge terminal policy; no DLQ or expiry delivery` / `broker-native dead-letter path (DLQ)` / `broker-native expiry path (ExpiryQueue)` / `inconclusive: message still on the source queue`.
2. Add field `artemis_destination: Option<ArtemisDestination>` to `ThresholdObservation`.
3. Rewrite the `DispositionBroker::observe_destination` Artemis branch to return the three-element tuple blessed in step 5: bounded wait (`wait_until`, 30 s / 200 ms poll) on `queue_stat(queue)` until `source.message_count == 0` (poll errors are retried like the Classic drain loop; a wait timeout is ignored there exactly as Classic does), then read `DLQ` and `ExpiryQueue` via `queue_stat`; on any `Err` return `(None, None, "broker-side queue-stat query failed: {error}")` naming the mechanism; on success return `(None, Some(ArtemisDestination { .. }), destination.describe())`. Keep the Classic branch byte-identical in behavior, returning `(Some(..), None, note)`.
4. Extend `classify_precedence`: after the existing Classic branch, `if let Some(destination) = &obs.artemis_destination` — return `NativeLimitFirst { evidence: destination.describe() }` when `native_dead_lettered()` OR `native_expired()`, else `Unknown { reason: "no terminal COMMIT and Artemis broker evidence does not confirm the native dead-letter path: {describe()}" }`. A missing `COMMIT` with an empty-source/empty-DLQ Artemis observation stays `Unknown`, never native-first.
5. Change `observe_destination`'s signature to a three-element tuple `(Option<ClassicDestination>, Option<ArtemisDestination>, String)` (Classic branch returns `(Some(..), None, note)` unchanged in behavior; Artemis branch per step 3) and update `run_threshold`'s call site and `ThresholdObservation` construction to destructure all three elements into `classic_destination`, `artemis_destination`, and `destination_note`.
6. Update `classify_precedence`'s final fallback reason string ("no stable broker-side disposition query on this fixture (bd rc-5c6yv)") to distinguish the two cases: when `artemis_destination` is `None` the reason says the broker-side query returned no evidence; when a destination exists the reason comes from step 4. The stale query-absent wording must not survive for runs where the query is present but inconclusive.

**Tests:** (run via `cargo test -p camel-test --features integration-tests --test jms_test sensitive_assertions` — the existing `sensitive_assertions` module plus the new synthetic cases, no broker needed; the plain `artemis_` substring filter would also select the live-broker auth case and must NOT be used here)
- `missing_commit_with_artemis_dlq_evidence_is_native_first`: setup — `ThresholdObservation { commit_frames: 0, artemis_destination: Some(source 0 / dlq added 1 / expiry 0), ..Default::default() }` → action — `classify_precedence` → assert `matches!(Precedence::NativeLimitFirst { .. })`.
- `missing_commit_with_artemis_expiry_evidence_is_native_first`: setup — same shape with expiry `messages_added: 1` → assert `NativeLimitFirst`.
- `missing_commit_with_artemis_no_evidence_is_unknown`: setup — source 0 / dlq 0 / expiry 0 → assert `Unknown`.
- `artemis_destination_describe_names_each_outcome`: setup — the three destination variants → action — `describe()` → assert each contains the exact outcome phrase from step 1.
- `artemis_acknowledged_and_removed_requires_all_three_queues`: setup — destination with dlq `messages_added: 1` → assert `!acknowledged_and_removed()`; with expiry `messages_added: 1` → assert `!acknowledged_and_removed()`.

**Acceptance:**
- `cargo test -p camel-test --features integration-tests --test jms_test sensitive_assertions` passes, including all pre-existing synthetic cases (10 pre-task; 15 total after the 5 new cases).
- `cargo fmt --check --all` exits 0; `cargo clippy -p camel-test --features integration-tests --all-targets -- -D warnings` exits 0.
- Classic test functions (`jms_bridge_threshold_reject_then_log_activemq`, `jms_bridge_redelivery_disposition_activemq`) are byte-identical to before this task.

- [x] 2.1

### Task 2.2: Assert the broker-side outcome in the Artemis disposition case

**Files:**
- `crates/camel-test/tests/support/jms_disposition_cases.rs` (modified)

**Steps:**
1. In `record_and_report_disposition`'s `Precedence::BridgeCapFirst` arm, for `label == "artemis"`: take `obs.artemis_destination`; on `None` `panic!("GAP rc-5c6yv [artemis]: broker-side disposition query returned no evidence — destination_note: {}, commit_wait_error: {:?}", obs.destination_note, obs.commit_wait_error)`; on `Some(destination)` assert `destination.acknowledged_and_removed()` with a panic message that embeds `destination.describe()`, then `eprintln!` a `PROOF rc-5c6yv [artemis]` line carrying `destination.describe()` — replacing the current `GAP 5.4 [artemis]` unobserved-destination note. Keep `assert_threshold_contract(obs)` and `log_observation(obs, label)` calls unchanged and keep the Classic (`label != "artemis"`) path byte-identical.
2. Update the `jms_bridge_redelivery_disposition_artemis` doc comment: it now reads the broker-side counts through the fixture query and asserts acknowledged-and-removed (or records native evidence via `classify_precedence`); the case fails when the query is removed, errors, or reports no evidence.
3. Update the module-level `## Disposition evidence boundaries` doc section: the Artemis destination is now observed through `ArtemisBroker::queue_stat` (docker exec + the container's own `artemis queue stat`, pinned image); a missing bridge `COMMIT` with Artemis evidence is classified exactly like Classic (native-first only with numeric DLQ/expiry evidence, else `UNKNOWN`); the load-bearing rule — query removal or error fails the case — is stated.

**Tests:** (run via `cargo test -p camel-test --features integration-tests --test jms_test sensitive_assertions` for the synthetic path plus `cargo test -p camel-test --features integration-tests --test jms_test --no-run` for compile proof)
- `bridge_cap_first_when_commit_and_record_match` (existing synthetic) must still pass — the wiring change must not break the shared classifier contract.
- `artemis_missing_destination_fails_the_case`: setup — a contract-valid `ThresholdObservation` that passes `assert_threshold_contract` (`reserve_frames`, `rollback_frames`, `accepted_events` all `DEFAULT_MAX_DELIVERY_ATTEMPTS` = 6, `commit_frames: 1`, `terminal_check { matching: 1, mislabeled: 0 }`, `payload_in_log: false`, `artemis_destination: None`) → action — `record_and_report_disposition(&obs, "artemis")` → assert panic with `#[should_panic(expected = "returned no evidence")]` — the executable proof of the load-bearing property (query absent/removed/error fails the case); the contract-valid counters are required because the BridgeCapFirst arm runs `assert_threshold_contract` BEFORE the destination check, so zeroed counters would panic earlier with the wrong message.
- Behavior proof (code-inspection acceptance, no live broker in this task): an `Err` from `queue_stat` flows into `artemis_destination: None` and surfaces through the same panic with the mechanism error.
- The exact STAGE 4 live command is `cargo test -p camel-test --features integration-tests --test jms_test jms_bridge_redelivery_disposition_artemis` (requires Docker and the bridge binary; run in the gate battery, not here).

**Acceptance:**
- `cargo test -p camel-test --features integration-tests --test jms_test sensitive_assertions` passes.
- `cargo fmt --check --all` exits 0; `cargo clippy -p camel-test --features integration-tests --all-targets -- -D warnings` exits 0.
- The literal source string `GAP 5.4 [artemis]` (the old unobserved-destination note) no longer appears in the file; the runtime-composed `GAP 5.4 [{label}]` panic prefixes in the `NativeLimitFirst`/`Unknown` arms remain by design for evidence-less failures.
- The live-broker behavior of `jms_bridge_redelivery_disposition_artemis` is verified in the STAGE 4 gate battery (Docker is available on this host), not in this task.

- [x] 2.2

## docs

### Task 3.1: Test-support documentation of mechanism, stability, and non-proofs

**Files:**
- `crates/camel-test/README.md` (modified)
- `crates/camel-test/tests/support/artemis.rs` (modified — module doc comment only)

**Steps:**
1. Add a `## Artemis broker-side disposition query` section to `crates/camel-test/README.md` stating: (a) mechanism — `docker exec` into the dedicated Artemis container running its own `/var/lib/artemis-instance/bin/artemis queue stat --queueName <q>` and parsing the pinned 2.36.0 table (`MESSAGE COUNT`, `MESSAGES ADDED`) for the source queue, `DLQ`, and `ExpiryQueue`; (b) stability argument — scoped to the verified `apache/activemq-artemis:2.36.0-alpine` pin: the CLI ships inside the container (version-matched to the broker), the image deploys no console wars (no Jolokia exists, which is why the Classic-style Jolokia path is unavailable), no extra port is exposed, and `docker exec` has precedent in `jms_crash_probe_cases.rs`; (c) the non-proofs — point-in-time management counts under the isolated dedicated-container scenario only, not journal layout or byte persistence, not a universal proof against message loss, no crash-proofness claim, `MESSAGES ADDED` accumulates over broker lifetime (exact here because each case owns a fresh container); (d) the terminal-kind distinction — the bridge's terminal discard is an acknowledgement (source drains, zero `DLQ`/`ExpiryQueue` additions) while a broker-native dead-letter moves the message to `DLQ` (non-zero `MESSAGES ADDED`), and with `dead-letter-address=DLQ` under `match="#"` there is no silent-drop route, so the counts always distinguish the two.
2. Extend the `artemis.rs` module doc comment with a two-sentence pointer to the README section and the same pin-scoped stability claim.

**Tests:**
- No executable tests: documentation-only task. Verify with `grep -c 'Artemis broker-side disposition query' crates/camel-test/README.md` returning 1.

**Acceptance:**
- The README section contains the four items (a)–(d) verbatim topics plus the positional-parse note from reviewer finding 4 (the parser depends on the pinned column order; the header guard in the parser keeps it honest); `cargo xtask lint-secrets` exits 0 (no credentials beyond the documented test-only artemis/artemis); `cargo fmt --check --all` exits 0; `cargo clippy -p camel-test --features integration-tests --all-targets -- -D warnings` exits 0.

- [x] 3.1
