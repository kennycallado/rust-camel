# Tasks: jms-bridge-080

Shared context. Every shell command runs in the worktree with:
`CARGO_TARGET_DIR=/home/shared/rust-camel-worktrees/356-jmsbridge080/target`,
`CARGO_BUILD_JOBS=4`, `TMPDIR=/home/shared/tmp`,
`GRADLE_USER_HOME=/home/shared/rust-camel-worktrees/356-jmsbridge080/.gradle-user`.
Java unit tests run through Docker because `java` is not on PATH. The documented
command (from `bridges/README.md`) adapted to the worktree env is:

```bash
BRIDGE_DIR=/home/shared/rust-camel-worktrees/356-jmsbridge080/bridges/jms
docker run --rm --user root \
  --volume="$BRIDGE_DIR:/project:z" \
  --volume=/home/shared/rust-camel-worktrees/356-jmsbridge080/.gradle-user:/gradle-user:z \
  --workdir=/project --env=GRADLE_USER_HOME=/gradle-user --env=HOME=/tmp \
  --entrypoint bash \
  quay.io/quarkus/ubi9-quarkus-graalvmce-builder-image:jdk-25 \
  -c "rm -rf /project/build && ./gradlew test --no-daemon --project-cache-dir /tmp/gradle-project-cache 2>&1"
```

Prerequisite apparatus (verified read-only, no build run yet): `java` is not on
PATH, so Java unit tests run through Docker. Docker server `29.7.2` is running.
The builder image `quay.io/quarkus/ubi9-quarkus-graalvmce-builder-image:jdk-25`
is present locally at digest
`sha256:6edcfc58e95498aebe646a13c3d2fcd8bc655368362f1aaf011d0b9caf318c7c`.
`bridges/jms/gradlew` is present. Integration tests need Docker broker
containers and a locally built native bridge for `CAMEL_JMS_BRIDGE_BINARY_PATH`:
0.7.1 (rebuilt in 5.2a) for Phase 5, 0.8.0 (built in 6.4) for Phase 6.
Disk stop guard applies to `/home/shared` only (53% now, STOP at 78%).

## Phase 1: Credential-safe producer header filtering

### Task 1.1: Add the `jmsHeaderAllowlist` endpoint option

**Files:**
- `crates/components/camel-jms/src/config.rs` (modified)
- `crates/components/camel-jms/src/metadata.rs` (modified)

**Steps:**
1. Add `pub jms_header_allowlist: Vec<String>` to `JmsEndpointConfig`.
2. In `from_uri`, parse `jmsHeaderAllowlist` and `jms_header_allowlist` as a comma-separated list; trim entries, drop empties, and default to `Vec::new()`. Match existing camelCase/snake_case alias style.
3. Add `pub(crate) fn normalize_header_name(key: &str) -> String` in `config.rs` that casefolds and removes `-`, `_`, and `.`.
4. Add `#[uri_param(name = "jmsHeaderAllowlist")]` to the `JmsMetadataDescriptor` option entry in `metadata.rs`, matching the alias handling of the existing options.

**Tests:**
- `jms_header_allowlist_parses_comma_list`: arrange `jms:queue:orders?jmsHeaderAllowlist=a,b`; act `JmsEndpointConfig::from_uri`; assert `jms_header_allowlist == ["a","b"]`; command `cargo test -p camel-component-jms --lib jms_header_allowlist_parses_comma_list`; expected red before the field exists, green after.
- `jms_header_allowlist_defaults_empty`: arrange `jms:queue:orders`; act `from_uri`; assert the list is empty; command `cargo test -p camel-component-jms --lib jms_header_allowlist_defaults_empty`.
- `normalize_header_name_folds_separators`: arrange `api-key`, `API_KEY`, `api_key`; act `normalize_header_name`; assert all equal `apikey`; command `cargo test -p camel-component-jms --lib normalize_header_name_folds_separators`.
- `metadata_has_jms_header_allowlist_option`: setup the metadata descriptor; act list its options; assert `jmsHeaderAllowlist` is present (assertion near `metadata.rs:74`); command `cargo test -p camel-component-jms --lib metadata_has_jms_header_allowlist_option`.

**Acceptance:**
- `cargo test -p camel-component-jms --lib` passes.
- `cargo clippy -p camel-component-jms --all-targets -- -D warnings`, `cargo fmt --check`, and `cargo xtask lint-single-source` exit 0.

- [x] 1.1

### Task 1.2: Deny-before-allowlist in `extract_send_headers`

**Files:**
- `crates/components/camel-jms/src/headers.rs` (modified)
- `crates/components/camel-jms/src/producer.rs` (modified)

**Steps:**
1. In `config.rs`, change `BROKER_SENSITIVE_KEY_SUBSTRINGS` from private to `pub(crate)` so `headers.rs` reuses the single list.
2. Change `extract_send_headers` to `pub fn extract_send_headers(exchange: &Exchange, allowlist: &[String]) -> HashMap<String, String>`.
3. Add `pub(crate) fn is_denied_send_header(key: &str) -> Option<&'static str>` in `headers.rs`. It normalizes the key and returns the denial reason when it matches: a reserved JMS name, a `jms` provider-control prefix (`JMS`, `JMSX`, `JMS_`), a `camel` or `xcamel` framework prefix, or a credential substring from the shared `crate::config::BROKER_SENSITIVE_KEY_SUBSTRINGS` plus `apikey`, `authorization`, `bearer`, and `principal`.
4. In `extract_send_headers`, deny first, then keep a header only when the normalized allowlist contains the normalized name. Keep only values with `as_str()`.
5. In `producer.rs`, pass `&endpoint_config.jms_header_allowlist` into `extract_send_headers`. Do not move the QoS header writes (`JMSExpiration`, `JMSPriority`, `JMSDeliveryMode`), which run after extraction.
6. Leave `apply_jms_headers` (inbound) unchanged.

**Tests:**
- `extract_send_headers_empty_allowlist_drops_user_headers`: setup an exchange with `x-custom=val`; act `extract_send_headers(&ex, &[])`; assert the map is empty; command `cargo test -p camel-component-jms --lib extract_send_headers_empty_allowlist_drops_user_headers`.
- `extract_send_headers_forwards_listed_business_header`: setup `customerId=42`; act with allowlist `["customerId"]`; assert `customerId` present; command `cargo test -p camel-component-jms --lib extract_send_headers_forwards_listed_business_header`.
- `extract_send_headers_denies_reserved_and_framework_names`: setup `JMSMessageID`, `JMSXDeliveryCount`, `CamelStop`, `x-camel-leader-epoch`; act with an allowlist naming all four; assert none is present; command `cargo test -p camel-component-jms --lib extract_send_headers_denies_reserved_and_framework_names`.
- `extract_send_headers_denies_credential_variants`: setup `password`, `Authorization`, `api-key`, `API_KEY`; act with an allowlist naming all; assert none is present; command `cargo test -p camel-component-jms --lib extract_send_headers_denies_credential_variants`.
- `inbound_apply_jms_headers_unchanged`: setup a `JmsMessage` with a `ContentType` header; act `apply_jms_headers`; assert `Content-Type` and the property map to exchange headers; command `cargo test -p camel-component-jms --lib inbound_apply_jms_headers_unchanged`.
- `qos_headers_set_after_extraction`: setup an exchange and a producer with TTL/priority; act the call set-up path; assert `JMSExpiration` and `JMSPriority` are set even with an empty allowlist; command `cargo test -p camel-component-jms --lib qos_headers_set_after_extraction`.

**Acceptance:**
- All six tests pass.
- `cargo clippy -p camel-component-jms --all-targets -- -D warnings` exits 0.

- [x] 1.2

### Task 1.3: Java `HeaderPolicy` backstop and producer wiring

**Files:**
- `bridges/jms/src/main/java/org/rustcamel/jms/HeaderPolicy.java` (new)
- `bridges/jms/src/main/java/org/rustcamel/jms/JmsProducer.java` (modified)
- `bridges/jms/src/test/java/org/rustcamel/jms/HeaderPolicyTest.java` (new)

**Steps:**
1. Create `HeaderPolicy` with `public static boolean isDenied(String key)` and `public static boolean isAllowed(java.util.List<String> allowlist, String key)` using the same normalize-and-deny rules as the Rust predicate.
2. In `JmsProducer.send`, before `msg.setStringProperty`, skip any key where `HeaderPolicy.isDenied(key)` is true. Do not change the `ContentType` dedicated handling.
3. Add `HeaderPolicyTest` with a table of reserved, framework, credential, and business names.

**Tests:**
- `header_policy_denies_reserved_framework_and_credentials`: setup a list of denied names; act `HeaderPolicy.isDenied`; assert true for each; command the Docker Gradle `test` command in Shared context; expected one passing class.
- `header_policy_allows_only_listed_business_names`: setup allowlist `["customerId"]`; act `isAllowed`; assert true for `customerId` and `CUSTOMERID`, false for `region`; same command.

**Acceptance:**
- Docker Gradle `test` exits 0 with `HeaderPolicyTest` passing.
- `JmsProducer` sets no denied property.

- [x] 1.3

### Task 1.4: Cross-language header parity corpus

**Files:**
- `bridges/jms/src/test/resources/header-policy-corpus.tsv` (new)
- `bridges/jms/src/test/java/org/rustcamel/jms/HeaderPolicyParityTest.java` (new)
- `crates/components/camel-jms/src/headers.rs` (modified — test module)

**Steps:**
1. Create `header-policy-corpus.tsv` with tab-separated `name<TAB>denied|allowed` rows covering reserved, framework, credential variants, and business names.
2. Add `HeaderPolicyParityTest` that reads the corpus from the classpath and asserts `HeaderPolicy.isDenied(name)` matches the expected column.
3. Add `rust_header_policy_matches_shared_corpus` in `headers.rs` tests that reads the corpus from `CARGO_MANIFEST_DIR/../../../bridges/jms/src/test/resources/header-policy-corpus.tsv` and asserts `is_denied_send_header(name).is_some()` matches the column.

**Tests:**
- `rust_header_policy_matches_shared_corpus`: setup the corpus path; act read and classify each row; assert classification matches; command `cargo test -p camel-component-jms --lib rust_header_policy_matches_shared_corpus`.
- `HeaderPolicyParityTest.header_policy_matches_shared_corpus`: same corpus through Java; Docker Gradle `test` command.

**Acceptance:**
- Both tests pass against one corpus file.
- `cargo clippy -p camel-component-jms --all-targets -- -D warnings` exits 0.

- [x] 1.4

## Phase 2: JournalService standalone (Java only, unit-tested, unwired)

### Task 2.1: `JournalService` framing, replay, and torn tail

**Files:**
- `bridges/jms/src/main/java/org/rustcamel/jms/JournalService.java` (new)
- `bridges/jms/src/main/java/org/rustcamel/jms/JournalPermanentException.java` (new)
- `bridges/jms/src/test/java/org/rustcamel/jms/JournalServiceTest.java` (new)

**Steps:**
1. Create checked `public class JournalPermanentException extends Exception`.
2. Define `public final class JournalService` with a versioned magic header and framed records `[u32 length][u8 type][payload]`. Type is `RESERVE`, `COMMIT`, `ROLLBACK`, or `RETIRE`; the payload carries the message ID and last-touched time.
3. Add package-private `static JournalState replay(Path file)` returning the in-memory `JournalState`. Record semantics: `RESERVE` increments the count, refreshes last-touched, and marks the entry in-flight; `ROLLBACK` clears in-flight only; `COMMIT` and `RETIRE` remove the entry.
4. Treat an incomplete trailing frame as a torn tail: discard the partial frame and keep every earlier confirmed count.

**Tests:**
- `replays_framed_records_preserving_counts`: setup a file with RESERVE records; act `replay`; assert the count per message ID in `JournalState`.
- `torn_tail_is_discarded_and_confirmed_counts_kept`: setup a valid file plus a truncated last frame; act `replay`; assert the truncated record is absent and earlier counts remain.
- `rollback_clears_in_flight_only`: setup RESERVE then ROLLBACK; act `replay`; assert the count is unchanged and the entry is not in-flight.
- `commit_and_retire_remove_entries`: setup RESERVE then COMMIT, and RESERVE then RETIRE; act `replay`; assert both entries are gone.
- Command: Docker Gradle `test`.

**Acceptance:**
- `JournalServiceTest` passes; no JVM tests touch `.gradle-user` outside the mapped volume.

- [x] 2.1

### Task 2.2: Journal instance API, lazy lock, and owner-private files

**Files:**
- `bridges/jms/src/main/java/org/rustcamel/jms/JournalService.java` (modified)
- `bridges/jms/src/test/java/org/rustcamel/jms/JournalServiceTest.java` (modified)

**Steps:**
1. Add the constructor `public JournalService(Path root, long maxBytes, long retentionMs, java.util.function.LongSupplier clockMs)`.
2. Add the instance method `public JournalHandle acquire(String brokerKey, String destinationKey, long lockWaitMs) throws JournalPermanentException` and nested `public static final class JournalHandle implements AutoCloseable` with `reserve(String messageId)`, `commit(String messageId)`, `rollback(String messageId)`, `long count(String messageId)`, and `close()`.
3. Acquire an OS file lock on a separate `<destination>.lock` file via `FileChannel.tryLock`; never rename, replace, or delete the lock file. The kernel releases the lock at process exit.
4. Busy lock: retry until `lockWaitMs` elapses, then throw `JournalPermanentException` naming `JMS_ATTEMPT_JOURNAL_DIR`.
5. Create the broker subdirectory with `PosixFilePermissions` `rwx------` (0700) and the journal, lock, and snapshot files with `rw-------` (0600) at creation time.

**Tests:**
- `lazy_acquire_release_allows_sequential_owner`: setup two sequential `acquire` calls on one destination; act release then re-acquire; assert the second succeeds and sees the prior count.
- `busy_lock_waits_bounded_then_fails_precondition`: setup one held handle; act a second `acquire` with a short wait; assert `JournalPermanentException` whose message contains `JMS_ATTEMPT_JOURNAL_DIR`.
- `lock_file_is_never_replaced`: setup an acquired handle; act `reserve` and `close`; assert the `.lock` inode is unchanged (read `Files.readAttributes(...).fileKey()` before and after).
- `journal_files_are_owner_private`: setup acquire and reserve; act inspect modes; assert the broker dir is 0700 and the journal, lock, and snapshot files are 0600.
- Command: Docker Gradle `test`.

**Acceptance:**
- All four tests pass; the lock file is stable and files are owner-private.

- [x] 2.2

### Task 2.3: Reservation append, fsync, space reservation, per-journal ceiling

**Files:**
- `bridges/jms/src/main/java/org/rustcamel/jms/JournalService.java` (modified)

**Steps:**
1. In `reserve`, append a RESERVE frame and `force(false)` before returning; the message must not leave the bridge until this returns.
2. Before any append or removal, reserve the bytes the write needs. Track the byte ceiling `JMS_ATTEMPT_JOURNAL_MAX_BYTES` per destination journal.
3. Include the temporary snapshot file in the ceiling accounting.

**Tests:**
- `reserve_persists_before_return`: setup a handle; act `reserve`; assert a fresh `open` sees the reservation (proves the fsync ran).
- `ceiling_includes_snapshot_peak`: setup a small ceiling; act a compaction; assert the peak (live + snapshot) is accounted and the ceiling is not exceeded silently.
- Command: Docker Gradle `test`.

**Acceptance:**
- Both tests pass; `reserve` returns only after the durable append.

- [x] 2.3

### Task 2.4: Compaction snapshot, atomic replace, and open trigger

**Files:**
- `bridges/jms/src/main/java/org/rustcamel/jms/JournalService.java` (modified)

**Steps:**
1. Implement compaction: write a temporary snapshot, `force(true)` it, atomically replace the journal, then fsync the parent directory.
2. Run compaction when the journal opens and before any fail-closed decision at the ceiling.

**Tests:**
- `snapshot_replace_is_atomic_and_parent_dir_synced`: setup a journal; act compaction; assert the journal path holds the new content and the temp file is gone.
- `compaction_runs_at_open_and_before_ceiling`: setup a journal with stale entries; act `open` near the ceiling; assert compaction ran before any ceiling failure.
- Command: Docker Gradle `test`.

**Acceptance:**
- Both tests pass; replacement uses an atomic move.

- [x] 2.4

### Task 2.5: Retention retirement

**Files:**
- `bridges/jms/src/main/java/org/rustcamel/jms/JournalService.java` (modified)

**Steps:**
1. Store a last-touched time per reservation; refresh it on each `reserve`. A `RESERVE` marks the entry in-flight; a `ROLLBACK` clears in-flight only.
2. During compaction, emit RETIRE for entries untouched longer than `JMS_ATTEMPT_JOURNAL_RETENTION_MS` (default 86400000).
3. Never retire an entry in the in-flight set (the current reservation). Never evict a live reservation to admit another.

**Tests:**
- `stale_entries_retire_at_compaction`: setup an entry older than the window; act compaction; assert it is retired and a later reserve restarts at one.
- `current_reservation_is_not_retired`: setup an in-flight reservation older than the window; act compaction; assert it remains.
- `no_live_eviction_fails_closed`: setup a full journal of unretirable entries; act a new reserve; assert `JournalPermanentException` and no entry removed.
- Command: Docker Gradle `test`.

**Acceptance:**
- All three tests pass; the cap is documented as not absolute forever in the class javadoc.

- [x] 2.5

### Task 2.6: Configuration bounds validation

**Files:**
- `bridges/jms/src/main/java/org/rustcamel/jms/JournalService.java` (modified)
- `bridges/jms/src/main/java/org/rustcamel/jms/BridgeConfig.java` (modified)
- `bridges/jms/src/test/java/org/rustcamel/jms/JournalConfigBoundsTest.java` (new)

**Steps:**
1. In `BridgeConfig`, read `JMS_ATTEMPT_JOURNAL_RETENTION_MS`, `JMS_ATTEMPT_JOURNAL_LOCK_WAIT_MS`, `JMS_ATTEMPT_JOURNAL_MAX_BYTES`, and `JMS_MAX_DELIVERY_ATTEMPTS`.
2. Validate ranges: retention `[60000, 31536000000]`, lock wait `[100, 300000]`, max bytes `[65536, 268435456]`, max attempts `[1, 100]`. Defaults: 86400000, 30000, 4194304, 6.
3. An out-of-range or non-numeric value raises a permanent configuration error used by the phase 4 startup signal.

**Tests:**
- `out_of_range_value_is_permanent`: setup each variable out of range; act the accessor; assert a permanent configuration error naming the variable.
- `defaults_match_bounds`: setup unset variables; act the accessors; assert the four defaults.
- Command: Docker Gradle `test`.

**Acceptance:**
- Both tests pass; `BridgeConfig` advertises the ranges in javadoc.

- [x] 2.6

### Task 2.7: Corrupt journal and I/O fail-closed paths

**Files:**
- `bridges/jms/src/main/java/org/rustcamel/jms/JournalService.java` (modified)
- `bridges/jms/src/test/java/org/rustcamel/jms/JournalServiceTest.java` (modified)

**Steps:**
1. On a bad magic, bad frame length, or unsupported record type, throw `JournalPermanentException`; never reset a confirmed count.
2. On an I/O error at open, append, or compaction, throw `JournalPermanentException`.
3. Ensure no path truncates or replaces a journal on error.

**Tests:**
- `corrupt_journal_fails_closed`: setup a file with a bad frame length; act `open`; assert `JournalPermanentException` and the original file bytes unchanged.
- `io_error_is_permanent`: setup an unwritable journal path; act `open`; assert `JournalPermanentException`.
- Command: Docker Gradle `test`.

**Acceptance:**
- Both tests pass; no silent reset exists.

- [x] 2.7

## Phase 3: Settlement end-to-end and journal wiring (Java and Rust together)

### Task 3.1: Extend the proto with completion settlement

**Files:**
- `bridges/jms/src/main/proto/jms_bridge.proto` (modified)
- `crates/components/camel-jms/proto/jms_bridge.proto` (modified)
- `crates/components/camel-jms/src/lib.rs` (modified — comment only)
- `crates/components/camel-jms/src/bridge_client_test.rs` (modified — `MockJmsBridge`)
- `crates/components/camel-jms/src/consumer.rs` (modified — test `RecordingBridge`)

The bridge proto and the crate-local proto are two copies of one wire contract. Task 3.1 applies identical message, enum, and RPC additions to both. `crates/components/camel-jms/build.rs` compiles the crate-local `proto/jms_bridge.proto`; the bridge proto keeps its `option java_package = "jms_bridge"` and `option java_multiple_files = true`.

**Steps:**
1. Add `rpc Complete(CompletionRequest) returns (CompletionResponse);` to `BridgeService` in both proto files.
2. Add `enum CompletionOutcome { COMPLETION_OUTCOME_UNSPECIFIED = 0; COMMIT = 1; ROLLBACK = 2; }` to both.
3. Add `message CompletionRequest { string subscription_id = 1; uint64 subscription_generation = 2; string delivery_token = 3; CompletionOutcome outcome = 4; }` and `message CompletionResponse { bool accepted = 1; }` to both.
4. Add `string delivery_token = 8; uint64 subscription_generation = 9; uint32 reservation_count = 10;` to `JmsMessage` in both.
5. Give `MockJmsBridge` (`bridge_client_test.rs`) and `RecordingBridge` (`consumer.rs` tests) a `complete` impl that returns an accepted response, so both mock services implement the new RPC.
6. In the Rust consumer, copy `reservation_count` into the exchange property `CamelJmsReservationCount` (a property, not a header), so the outbound header path is unchanged.
7. Preserve the Java options in the bridge proto; do not add them to the crate-local copy.

**Tests:**
- `proto_completion_types_exist`: setup the generated Rust `proto` module; act construct `CompletionRequest::default()` and reference `CompletionOutcome::Commit` and `CompletionResponse`; assert the symbols compile; command `cargo test -p camel-component-jms --lib proto_completion_types_exist`.
- `protos_have_identical_wire_additions`: setup both proto files; act remove `option ` lines and blank lines and compare; assert identical; command `diff <(grep -v '^option ' bridges/jms/src/main/proto/jms_bridge.proto | grep -v '^[[:space:]]*$') <(grep -v '^option ' crates/components/camel-jms/proto/jms_bridge.proto | grep -v '^[[:space:]]*$')`.
- `mock_services_implement_complete`: setup both mocks; act call `complete`; assert an accepted response; command `cargo test -p camel-component-jms --lib mock_services_implement_complete`.
- `build_exchange_sets_reservation_count_property`: setup a `JmsMessage` with `reservation_count = 4`; act `build_exchange`; assert `exchange.property("CamelJmsReservationCount") == 4` and no header of that name exists; command `cargo test -p camel-component-jms --lib build_exchange_sets_reservation_count_property`.
- Command for Java: Docker Gradle `test`.

**Acceptance:**
- The two protos are identical except the Java `option` lines.
- Rust and Java proto compilations succeed; both mock services implement `complete`.
- `cargo clippy -p camel-component-jms --all-targets -- -D warnings` exits 0.

- [x] 3.1

### Task 3.2: Owner-thread settlement in `JmsConsumer`

**Files:**
- `bridges/jms/src/main/java/org/rustcamel/jms/JmsConsumer.java` (modified)
- `bridges/jms/src/main/java/org/rustcamel/jms/JmsBridgeService.java` (modified)
- `bridges/jms/src/test/java/org/rustcamel/jms/JmsConsumerCompletionTest.java` (new)

**Steps:**
1. Open the session with `CLIENT_ACKNOWLEDGE`. Assign a `subscription_generation` at subscribe and a fresh `delivery_token` per delivery.
2. Add a per-subscription completion queue. `JmsBridgeService.complete(...)` validates the subscription, generation, and token, then enqueues the decision; `JmsConsumer.complete(...)` returns whether it was accepted.
3. The owner thread settles at one linearization point: `COMMIT` acknowledges once; `ROLLBACK` recovers; a duplicate or late completion never acknowledges again.
4. A cancel, stop, or disconnect before the decision closes without ack; after the decision the ack stands. A lost commit response stands and Rust never retries.
5. Emit `delivery_token` and `subscription_generation` on each `JmsMessage`.

**Tests:**
- `successful_route_commits_exactly_once`: setup a stub session; act COMMIT then a duplicate COMMIT; assert one `acknowledge()` call.
- `propagated_error_rolls_back`: act ROLLBACK; assert `recover()` once and no `acknowledge()`.
- `cancel_before_decision_prevents_ack`: act stop before any completion; assert no `acknowledge()` and no `recover()`.
- `cancel_after_decision_does_not_undo`: act COMMIT then stop; assert one `acknowledge()` and no second call.
- `lost_commit_response_stands`: act COMMIT with a dropped response; assert one `acknowledge()` and no retry.
- `stale_generation_or_token_ignored`: act a completion with a wrong token or generation; assert `accepted == false` and no ack.
- `slow_route_stays_pending`: act no completion within a time slice; assert the reservation stays pending and no deadline fires.
- Command: Docker Gradle `test`.

**Acceptance:**
- All seven tests pass.

- [x] 3.2

### Task 3.3: Conversion-failure recover, session options, and message types

**Files:**
- `bridges/jms/src/main/java/org/rustcamel/jms/JmsConsumer.java` (modified)
- `bridges/jms/src/test/java/org/rustcamel/jms/JmsConsumerMessageTypesTest.java` (modified)
- `bridges/jms/src/test/java/org/rustcamel/jms/JmsConsumerContentTypeTest.java` (modified)

**Steps:**
1. On a conversion failure, count a reservation failure and `session.recover()` before the next `receive`. Do not receive again until recover completes. If recover fails, close without ack.
2. Keep `CLIENT_ACKNOWLEDGE` regardless of endpoint acknowledgement or transaction options; `transactionMode=Session` keeps a warning naming bridge-managed settlement.
3. Update `JmsConsumerMessageTypesTest` to assert `CLIENT_ACKNOWLEDGE`, no body accessor, and the terminal-discard exception.

**Tests:**
- `conversion_failure_recovers_before_next_receive`: setup a stub that fails conversion; act the loop; assert `recover()` occurs before the next `receive()` and no `acknowledge()`.
- `message_types_use_client_acknowledge`: setup Object/Map/Stream messages; act forwarding; assert `CLIENT_ACKNOWLEDGE`, no accessor invoked, no ack before route success except terminal discard.
- Command: Docker Gradle `test`.

**Acceptance:**
- Both tests pass; `JmsConsumerMessageTypesTest` green.

- [x] 3.3

### Task 3.4: Wire lazy journal ownership into subscribe

**Files:**
- `bridges/jms/src/main/java/org/rustcamel/jms/JmsConsumer.java` (modified)
- `bridges/jms/src/main/java/org/rustcamel/jms/JmsBridgeService.java` (modified)
- `bridges/jms/src/main/java/org/rustcamel/jms/JournalService.java` (modified)
- `bridges/jms/src/main/java/org/rustcamel/jms/PortAnnouncer.java` (modified)
- `bridges/jms/src/test/java/org/rustcamel/jms/JournalSubscribeWiringTest.java` (new)

**Steps:**
1. Add `static void JournalService.validateStartup(String dirEnv, String brokerKeyEnv) throws JournalPermanentException`. It accepts unset values and rejects set-but-invalid ones: the broker key must match `^[0-9a-f]{32}$`, and the directory must resolve and be owner-private.
2. Call `validateStartup` from `PortAnnouncer.onStart` before the ready line, through the existing `System.exit(1)` path. Task 4.1 replaces that path with the permanent-failure line and exit 78.
3. At the first subscribe for a queue destination, call the instance `journal.acquire(brokerKey, destinationKey, lockWaitMs)` before the first receive, where `journal` is `new JournalService(root, maxBytes, retentionMs, clockMs)`.
4. The `destinationKey` comes from `static String JournalService.destinationKey(String destinationName)`: the first 32 lowercase hex characters of SHA-256 over UTF-8 `"queue:" + name`.
5. Release the handle when the last subscription to that domain ends, including cancel, disconnect, stop, and shutdown.
6. When `JMS_ATTEMPT_JOURNAL_DIR` is unset or `JMS_BRIDGE_BROKER_KEY` is unset, report readiness, keep producers working, and fail only a queue subscribe with `FAILED_PRECONDITION` naming the missing variable.
7. When the directory or key is set but invalid, signal a startup permanent failure before readiness.
8. Readiness opens no journal. Producer and topic paths never acquire a journal. A busy lock surfaces `FAILED_PRECONDITION` at subscribe; it stays transient until phase 4 classifies it.

**Tests:**
- `lock_acquired_at_first_subscribe`: setup a queue subscribe; act; assert the handle was acquired before the first receive.
- `lock_released_on_stream_end`: setup an active subscription; act stream cancel; assert the handle is released and a later subscribe acquires it.
- `producer_and_topic_paths_never_lock`: setup a producer and a topic subscribe; act; assert no journal acquire.
- `busy_lock_fails_precondition`: setup a held handle; act a subscribe with a short wait; assert `FAILED_PRECONDITION` naming `JMS_ATTEMPT_JOURNAL_DIR`.
- `unset_journal_env_readiness_producer_ok_subscribe_precondition`: setup unset `JMS_ATTEMPT_JOURNAL_DIR` or unset `JMS_BRIDGE_BROKER_KEY`; act readiness, a producer send, and a queue subscribe; assert readiness succeeds, the producer send succeeds, and the subscribe fails `FAILED_PRECONDITION` naming the missing variable.
- `invalid_dir_is_startup_permanent_before_readiness`: setup a symlink-escape or unresolved directory; act start; assert a startup permanent failure and no readiness.
- `invalid_broker_key_is_startup_permanent`: setup a key not matching `^[0-9a-f]{32}$`; act `validateStartup`; assert `JournalPermanentException`.
- `readiness_opens_no_journal`: setup a valid directory and no subscribe; act readiness; assert no journal file was opened or created.
- `two_subscriptions_same_domain_share_cap`: setup two subscriptions on one queue domain in one bridge process; act reservations through both; assert one shared cap.
- Command: Docker Gradle `test`.

**Acceptance:**
- All nine tests pass.

- [x] 3.4

### Task 3.5: Rust `send_and_wait` mapping

**Files:**
- `crates/components/camel-jms/src/consumer.rs` (modified)

**Steps:**
1. Replace the fire-and-forget `ctx.send(exchange)` with `ctx.send_and_wait(exchange)` inside a `tokio::select!` with the cancel token.
2. Map `Ok` to `CompletionOutcome::Commit`; map `Err` including `ChannelClosed` to `Rollback` or drop the stream; never map an error to `Commit`. Keep the `b-prime:jms:consumer-send` metric and `// log-policy: outside-contract` annotation.
3. On a completion RPC failure, drop the stream; do not retry.

**Tests:**
- `send_and_wait_ok_reports_commit`: setup a mock context returning `Ok`; act the loop body; assert one `Complete(COMMIT)`.
- `send_and_wait_err_reports_rollback`: setup `Err(ProcessorError)`; act; assert one `Complete(ROLLBACK)`.
- `channel_closed_never_commits`: setup `Err(ChannelClosed)`; act; assert no `Complete(COMMIT)` and the metric label path is used.
- `handled_and_stop_map_to_ok`: setup `Ok` (Completed or Stopped); act; assert `COMMIT`.
- `completion_failure_drops_stream`: setup a completion RPC error; act; assert the stream is dropped and no commit is sent.
- Command: `cargo test -p camel-component-jms --lib` (covers all five tests).

**Acceptance:**
- All five tests pass.
- `tests/send_and_wait_b_bridged_regression.rs` stays green: `cargo test -p camel-component-jms --test send_and_wait_b_bridged_regression`.

- [x] 3.5

### Task 3.6: Rust-owned key and directory env wiring

**Files:**
- `crates/components/camel-jms/src/config.rs` (modified)
- `crates/components/camel-jms/src/component.rs` (modified)
- `crates/components/camel-jms/Cargo.toml` (modified)
- `crates/services/camel-bridge/src/process.rs` (modified)
- `crates/camel-test/tests/support/jms_bridge_procs.rs` (modified)
- `crates/camel-test/tests/jms_test.rs` (modified)

**Steps:**
1. Add pure `pub fn resolve_attempt_journal_dir(xdg_state_home: Option<OsString>, home: Option<OsString>) -> Result<PathBuf, CamelError>`: `$XDG_STATE_HOME/rust-camel/jms-bridge` when set, else `$HOME/.local/state/rust-camel/jms-bridge`; an empty or relative `XDG_STATE_HOME` counts as unset; error when neither resolves.
2. Make `pub fn default_attempt_journal_dir() -> Result<PathBuf, CamelError>` a thin env wrapper over the pure function.
3. Add `pub fn ensure_attempt_journal_dir(&Path) -> Result<(), CamelError>`: creates a missing directory recursively with `std::os::unix::fs::DirBuilderExt` mode `0o700`, never chmods an existing directory. Rust calls it before every launch.
4. Add `pub fn attempt_journal_broker_key(broker_name: &str, broker_type: &BrokerType, broker_url: &str) -> String`: a stable SHA-256 hex (first 16 bytes, 32 chars) of name, type, and the redacted URL.
5. In `crates/components/camel-jms/Cargo.toml`, add `sha2 = { workspace = true }` and `hex = { workspace = true }` to `[dependencies]`, and `tempfile = { workspace = true }` to `[dev-dependencies]`. All three already exist in the workspace and `camel-bridge` uses them; no new dependency is introduced.
6. Extend `BridgeProcessConfig::jms` with `journal_dir: Option<String>` and `broker_key: Option<String>` that append `JMS_ATTEMPT_JOURNAL_DIR` and `JMS_BRIDGE_BROKER_KEY` env vars.
7. In `component.rs` `start_bridge_process`, compute the key, call `ensure_attempt_journal_dir`, and pass the resolved dir on the initial launch and every restart launch.
8. In `support/jms_bridge_procs.rs`, `spawn_jms_bridge` passes a per-test temp dir and key.
9. In `jms_test.rs`, add helper `next_message_committed(client, subscription_id, stream, what)` that sends `Complete(COMMIT)` with the message's token and generation and returns the next message. Make `subscribe_bounded` return the `subscription_id`. Use both helpers in `jms_two_bridge_processes_same_binary_coexist` and `jms_message_ids_unique_across_bridge_restart`, so one in-flight delivery per session cannot hang the second read.

**Tests:**
- `resolve_attempt_journal_dir_uses_xdg_then_home`: setup the pure fn with XDG set, XDG empty, XDG relative, and HOME only; act; assert the exact paths and the no-home error. The test never calls `std::env::set_var` (edition 2024).
- `ensure_attempt_journal_dir_creates_0700`: setup a tempdir; act ensure on a missing child; assert it exists with mode 0700; act again on an existing dir; assert the mode is not changed.
- `broker_key_hash_is_stable`: setup the same inputs twice; act; assert equal; change the broker name; assert different.
- `journal_env_passed_on_launch_and_restart`: setup a captured `BridgeProcessConfig`; act `jms(...)`; assert both env keys present.
- `jms_two_bridge_processes_same_binary_coexist` and `jms_message_ids_unique_across_bridge_restart`: both green with the helpers; commands `cargo test -p camel-test --features integration-tests --test jms_test <name>`.
- Command: `cargo test -p camel-component-jms --lib` (covers the pure, ensure, key, and env tests).

**Acceptance:**
- Set-but-invalid directory or key is a startup permanent failure; the pure resolver returns the expected paths.
- `rg -n 'local/state' crates bridges --glob '!target'` matches only `crates/components/camel-jms/src/config.rs`.
- `cargo clippy -p camel-bridge -p camel-component-jms --all-targets -- -D warnings` exits 0.

- [x] 3.6

### Task 3.7: Scope, shared budget, and topic behavior

**Files:**
- `bridges/jms/src/main/java/org/rustcamel/jms/JournalService.java` (modified)
- `bridges/jms/src/test/java/org/rustcamel/jms/JournalServiceTest.java` (modified)
- `bridges/jms/src/test/java/org/rustcamel/jms/JournalSubscribeWiringTest.java` (modified)

**Steps:**
1. Use `JournalService.destinationKey`, introduced in task 3.4; do not redefine it. It returns the first 32 lowercase hex characters of SHA-256 over UTF-8 `"queue:" + name`.
2. Scope the journal path to `<dir>/<broker-key>/<destination-key>.journal`. Validate the broker key against `^[0-9a-f]{32}$`; never use a raw URL or raw message ID as a path.
3. Share one budget among consumers served by one bridge process; a sequential owner inherits counts through lock handoff.
4. Keep topics on a subscription-scoped volatile counter with a documented non-durable cap. Document the N-duplicate bound where N is the number of distinct journal domains (directory, broker key, destination key) consuming the queue within one retention window.

**Tests:**
- `journal_path_uses_hashed_keys` (Java, `JournalServiceTest`): setup broker and destination keys; act path derivation; assert the path uses the 32-hex broker key and the SHA-256 destination key and contains no raw URL or message ID.
- `topic_uses_volatile_counter` (Java, `JournalSubscribeWiringTest`): setup a topic subscribe; act; assert no journal file is created and the counter is per subscription.
- Command: Docker Gradle `test`.

**Acceptance:**
- Both Java tests pass; path derivation is deterministic and Java-side.

- [x] 3.7

### Task 3.8: Reservation threshold and reject-then-log

**Files:**
- `bridges/jms/src/main/java/org/rustcamel/jms/JmsConsumer.java` (modified)
- `bridges/jms/src/test/java/org/rustcamel/jms/JmsConsumerThresholdTest.java` (new)

**Steps:**
1. Before handoff, read the reservation count for the message ID from the journal handle and compare it to `JMS_MAX_DELIVERY_ATTEMPTS` (default 6).
2. On the receipt whose count exceeds the maximum, apply the terminal policy without a handoff: acknowledge once and log at `error!` with the message ID and reservation count only, never the body or headers.
3. Never forward to a dead-letter destination.

**Tests:**
- `six_reservations_then_terminal`: setup count 6 for one message ID; act a seventh receipt; assert no handoff and reject-then-log; command Docker Gradle `test`.
- `terminal_logs_id_and_count_only`: setup a terminal; act; assert the log line carries the ID and count and no payload or header bytes; command Docker Gradle `test`.

**Acceptance:**
- Both tests pass; no dead-letter forwarding exists.

- [x] 3.8

## Phase 4: Permanent-failure propagation

### Task 4.1: Permanent-failure signal from the bridge

**Files:**
- `bridges/jms/src/main/java/org/rustcamel/jms/PortAnnouncer.java` (modified)
- `crates/services/camel-bridge/src/process.rs` (modified)
- `crates/components/camel-jms/src/component.rs` (modified)

**Steps:**
1. In `PortAnnouncer.onStart(@Observes StartupEvent ev)`, replace the phase-3 `System.exit(1)` path: print `{"status":"permanent-failure","reason":"..."}` on stdout and exit with the reserved code `78`; never print `{"status":"ready"}`. There is no `main`-method hook; `PortAnnouncer`'s `@Observes StartupEvent` is the startup hook.
2. In `process.rs`, add typed `BridgeError::PermanentFailure(String)`. The ready loop returns it on the `{"status":"permanent-failure"}` line.
3. On EOF before ready, do a bounded `child.wait()` (2 s) and map exit code `78` to `BridgeError::PermanentFailure`.
4. In `component.rs` `start_bridge_process` (`component.rs:583`), match `BridgeError::PermanentFailure(reason)` from `BridgeProcess::start_and_connect` **before** the `map_err` at `component.rs:616` erases it into a string. Return the crate-private `enum StartBridgeError { Permanent(String), Other(CamelError) }`, so 4.2 maps `Permanent` to `Parked` without substring matching.

**Tests:**
- `parses_permanent_failure_status_line`: setup a captured stdout line; act the ready loop; assert `BridgeError::PermanentFailure` carrying the reason; command `cargo test -p camel-bridge --lib parses_permanent_failure_status_line`.
- `eof_with_exit_78_maps_to_permanent`: setup an EOF before ready plus exit code 78; act the bounded wait; assert `BridgeError::PermanentFailure`; command `cargo test -p camel-bridge --lib eof_with_exit_78_maps_to_permanent`.
- `start_bridge_error_maps_permanent`: setup `start_and_connect` returning `BridgeError::PermanentFailure`; act `start_bridge_process`; assert `StartBridgeError::Permanent`; command `cargo test -p camel-component-jms --lib start_bridge_error_maps_permanent`.

**Acceptance:**
- All three tests pass; a `ready` line never maps to permanent; the subscribe path emits no permanent-failure line.

- [x] 4.1

### Task 4.2: `Parked` pool state

**Files:**
- `crates/components/camel-jms/src/component.rs` (modified)
- `crates/components/camel-jms/src/consumer.rs` (modified — `BridgeState` match sites)

**Steps:**
1. Add `BridgeState::Parked(String)`.
2. Map `StartBridgeError::Permanent` from `start_bridge_process` to `Parked`; never route it to `Restarting`.
3. Update every `BridgeState` match site: the monitor at `component.rs:438-575`, `get_or_create_slot`, `consumer.rs:101`, `consumer.rs:426`, and `LazyJmsProducer` in `component.rs:851-940`.
4. `get_or_create_slot` and the health monitor never respawn a `Parked` slot.

**Tests:**
- `startup_permanent_failure_parks_slot`: setup a start returning `StartBridgeError::Permanent`; act the pool start; assert `BridgeState::Parked`.
- `parked_slot_is_not_respawned`: setup a parked slot; act the monitor tick; assert no restart attempt.
- Command: `cargo test -p camel-component-jms --lib` (covers both tests).

**Acceptance:**
- Both tests pass; no `BridgeState` match site is left non-exhaustive.

- [x] 4.2

### Task 4.3: Parked waiters fail fast

**Files:**
- `crates/components/camel-jms/src/consumer.rs` (modified)
- `crates/components/camel-jms/src/component.rs` (modified — `LazyJmsProducer`)

**Steps:**
1. In `await_ready_channel`, add a `BridgeState::Parked(reason)` arm that returns an error naming the operator repair.
2. In the consumer pre-flight probe, treat `Parked` like a hard error.
3. In `LazyJmsProducer::poll_ready`/`call` (`component.rs:851-940`), return the error for `Parked` so sends fail fast. Leave `JmsProducer::poll_ready` in `producer.rs` unconditional and unchanged; it is pinned by `poll_ready_returns_ok_unconditionally`.

**Tests:**
- `await_ready_on_parked_returns_err`: setup a parked slot; act `await_ready_channel`; assert `Err` within a bounded time naming the repair.
- `consumer_preflight_parked_fails`: act `start`; assert the pre-flight error.
- `lazy_producer_poll_ready_parked_fails_fast`: act `LazyJmsProducer::poll_ready`; assert the error.
- Command: `cargo test -p camel-component-jms --lib parked`.

**Acceptance:**
- All three tests pass; `poll_ready_returns_ok_unconditionally` stays green.

- [x] 4.3

### Task 4.4: Route-only `FAILED_PRECONDITION` and unkeyable IDs

**Files:**
- `crates/components/camel-jms/src/consumer.rs` (modified)
- `bridges/jms/src/main/java/org/rustcamel/jms/JmsBridgeService.java` (modified)
- `bridges/jms/src/main/java/org/rustcamel/jms/JmsConsumer.java` (modified)

**Steps:**
1. Add `pub(crate) fn is_permanent_status(status: &tonic::Status) -> bool` in `consumer.rs` returning true for `tonic::Code::FailedPrecondition`. Apply it to the subscribe result and to the `stream.message()` Err arm before conversion.
2. Do not add a classifier over `&CamelError`; the gRPC code is lost after conversion.
3. In `JmsBridgeService.subscribe`, map `JournalPermanentException` to `Status.FAILED_PRECONDITION` on the observer.
4. On an unkeyable `JMSMessageID`, terminate the subscription without acknowledging, preserve the message for redelivery, and never manufacture an ID.

**Tests:**
- `destination_failure_is_route_only`: setup a subscribe returning `Status::failed_precondition`; act; assert that route fails and the slot stays `Ready`.
- `unkeyable_id_preserves_message`: setup an empty message ID; act; assert no ack, no manufactured ID, subscription terminated.
- `permanent_status_classifier`: setup `Status::failed_precondition` and `Status::unavailable`; act `is_permanent_status`; assert true then false.
- Command: `cargo test -p camel-component-jms --lib` (covers all three tests).

**Acceptance:**
- All three tests pass; no `&CamelError` classifier exists.

- [x] 4.4

### Task 4.5: All-task supervision

**Files:**
- `crates/components/camel-jms/src/consumer.rs` (modified)

**Steps:**
1. Change `consumer_loop` to return `Result<(), CamelError>` and wrap each spawned task so its result is observable.
2. Any task exit other than a requested cancellation (permanent failure, panic, or reconnect exhaustion) cancels sibling tasks through a shared token and returns `Err` through the supervised handle.
3. A requested cancellation is normal and does not return `Err`.

**Tests:**
- `any_task_failure_cancels_siblings_and_returns_err`: setup two tasks, one failing permanently; act; assert siblings cancelled and the supervised handle returns `Err`.
- `panic_is_surfaced`: setup a panicking task; act; assert the handle returns `Err`.
- `reconnect_exhaustion_is_surfaced`: setup exhausted reconnect; act; assert `Err`.
- `requested_cancellation_is_normal`: setup a cancel; act; assert siblings stop and the handle returns `Ok`.
- Command: `cargo test -p camel-component-jms --lib` (covers all four tests).

**Acceptance:**
- All four tests pass.

- [x] 4.5

### Task 4.6: Restart no hot-loop and transient reconnect

**Files:**
- `crates/components/camel-jms/src/consumer.rs` (modified)
- `crates/components/camel-jms/src/component.rs` (modified)

**Steps:**
1. A supervision restart against a still-broken journal fails through the pre-flight probe or a subscribe-time `FAILED_PRECONDITION` without an unbounded loop.
2. Keep transient transport reconnect behavior unchanged.

**Tests:**
- `restart_broken_journal_no_hot_loop`: setup a broken journal; act two restart attempts; assert each fails and attempts stay bounded.
- `transient_error_still_reconnects`: setup a transient stream error; act; assert the reconnect path runs.
- Command: `cargo test -p camel-component-jms --lib` (covers both tests).

**Acceptance:**
- Both tests pass; no busy loop in the restart path.

- [x] 4.6

### Task 4.7: Operator recovery strings

**Files:**
- `crates/components/camel-jms/src/component.rs` (modified)
- `crates/components/camel-jms/src/consumer.rs` (modified)

**Steps:**
1. Every permanent error names the operator action: stop the route, repair or remove the corrupt journal or the unkeyable source, then restart the context.
2. Keep the message free of payload and header bytes.

**Tests:**
- `permanent_error_names_operator_action`: setup a parked slot error; act format; assert the message names the repair and restart steps.
- Command: `cargo test -p camel-component-jms --lib` (covers the test).

**Acceptance:**
- The test passes; no payload or header value appears in the message.

- [x] 4.7

## Phase 5: Real-broker crash proof and redelivery disposition

### Task 5.1: Crash-probe binary

**Files:**
- `crates/camel-test/src/bin/jms_crash_probe.rs` (new)
- `crates/camel-test/Cargo.toml` (modified — `[[bin]]` with `required-features = ["integration-tests"]`)

**Steps:**
1. Add a binary that starts a consumer route with a first step that writes a handoff-marker file path from argv, then blocks.
2. The probe uses `CAMEL_JMS_BRIDGE_BINARY_PATH` and `CAMEL_JMS_BROKER_URL` and exits non-zero on setup failure.

**Tests:**
- `jms_crash_probe_writes_marker`: setup the binary built with the integration feature; act start against a Classic container; assert the marker file appears; command `cargo test -p camel-test --features integration-tests --test jms_test jms_crash`; expected green after the binary exists.
- Command: `cargo build -p camel-test --features integration-tests --bin jms_crash_probe`.

**Acceptance:**
- The binary builds and writes the marker.

- [x] 5.1

### Task 5.2a: Transport-close cancel-to-release (rc-icvzl)

Root cause (proven by thread dump, rc-icvzl): when the Rust client dies,
the TCP connection drops without RST_STREAM. Vert.x gRPC never calls
`onCancel`, so `JmsConsumer.awaitDecision` (JmsConsumer.java:652) waits
forever and the journal lock stays held by the orphan. Fix scope is
exactly this path; no other production behaviour changes. Version stays
0.7.1. `use-separate-server` is NOT changed. No deadline is added to
`awaitDecision`.

**Files:**
- `bridges/jms/src/main/java/org/rustcamel/jms/TransportCloseFilter.java` (new)
- `bridges/jms/src/main/java/org/rustcamel/jms/JmsBridgeService.java` (modified, near line 161)
- `bridges/jms/src/test/java/org/rustcamel/jms/JmsBridgeServiceTest.java` (modified)
- `bridges/jms/src/test/java/org/rustcamel/jms/TransportCloseProdModeTest.java` (new)

**Steps:**
1. Spike first (5.2a.1): a route-ordered filter registers a `closeHandler` on
   the response for `application/grpc` requests and completes a
   `CompletableFuture` stored in the Vert.x duplicated-context local. Verify
   the local reaches the `@Blocking` worker. If it does not, use the
   fallback: set an `x-rc-stream-key` header (overwriting any client value)
   and keep a key-to-future registry removed on close and on stream end.
2. Wire the future next to the `onCancel` handler: on completion call the
   existing `cleanupSubscription(consumer, subId, finished)`. Single entry;
   reuse the existing CAS on `finished` (JmsBridgeService.java:260-263).
3. Rebuild the native bridge at 0.7.1 after the Java change: run
   `cargo xtask build-jms-bridge --version 0.7.1`; record
   `sha256sum bridges/jms/build/native/jms-bridge` in bd rc-icvzl notes.

**Tests:**
- `transportCloseRunsCleanupOnce`: arrange a mocked consumer and a **pending**
  transport future; act complete the transport future, then fire `onCancel`;
  assert `stop()` and destroy run exactly once.
- `transportCloseProdModeDetectsSocketCut`: `@QuarkusTest` profile with
  `use-separate-server=false`; arrange a TCP proxy in front of the server
  and a subscribe through it; act cut the proxy sockets without RST; assert
  cleanup runs within 2 s.
- `normalStreamDoesNotCleanupEarly`: arrange a subscribe whose consumer emits
  and completes normally; act complete the stream without cancel or socket
  cut; assert the transport future is not completed and `stop()` runs once,
  only from the stream-end path.
- Command: Docker Gradle `test` (shared-context command).

**Acceptance:**
- The transport-close future completes on a socket cut with no RST_STREAM.
- Cleanup enters through the single path; the CAS prevents double release.
- The three tests pass; the native binary is rebuilt at 0.7.1 with its SHA256
  recorded.

- [x] 5.2a

### Task 5.2: Classic crash redelivery test

**Files:**
- `crates/camel-test/tests/support/jms_crash_probe_cases.rs` (modified: remove `#[ignore]` at line 320 and the `# Blocked (rc-icvzl)` doc block, only after the test passes)
- `crates/camel-test/tests/support/jms_crash_probe.rs` (modified if needed)

**Steps:**
1. Run `jms_bridge_crash_before_completion_redelivers_activemq`: spawn an isolated probe against a dedicated Classic container, wait for the marker, `SIGKILL` the probe, then assert a surviving consumer receives the same message.
2. Assert the survivor acquires the destination journal and observes the reservation count.
3. Reap the orphan bridge after the assertions; leave no process holding the lock.
4. Pin one stable explicit journal domain across the probe and the survivor: set `XDG_STATE_HOME` (the only Rust-side channel; there is no operator override) to a fixed test root for both processes and keep the broker name, type, and URL identical so both derive the same broker key. The per-spawn `TempDir` used by the ID-uniqueness and coexistence tests is unsuitable here — a fresh root would lose the recorded count and the survivor could not acquire the same journal.

**Tests:**
- `jms_bridge_crash_before_completion_redelivers_activemq`: setup a dedicated Classic broker and probe; act kill and redeliver; assert unchanged `JMSMessageID`, body, and headers, increased count, no `COMMIT`, survivor acquires the journal, orphan reaped.
- Command: `cargo test -p camel-test --features integration-tests --test jms_test jms_bridge_crash_before_completion_redelivers_activemq`.
- Prerequisite: Docker + the native 0.7.1 bridge rebuilt in 5.2a (SHA256 matches the bd record) via `CAMEL_JMS_BRIDGE_BINARY_PATH`.

**Acceptance:**
- The orphan bridge is still alive when the survivor subscribes, and the survivor acquires the destination journal within its `JMS_ATTEMPT_JOURNAL_LOCK_WAIT_MS` (default 30000 ms).
- The test passes or reports an apparatus gap. A broker-semantics gap is reported, not passed.
- Remove the `#[ignore]` at `jms_crash_probe_cases.rs:320` and the `# Blocked (rc-icvzl)` doc block only after this test passes. 5.2a must not touch them.

- [x] 5.2

### Task 5.3: Artemis crash redelivery test

**Files:**
- `crates/camel-test/tests/jms_test.rs` (modified)

**Steps:**
1. Add `jms_bridge_crash_before_completion_redelivers_artemis` mirroring task 5.2 against Artemis.

**Tests:**
- `jms_bridge_crash_before_completion_redelivers_artemis`: same contract as 5.2 on Artemis; command `cargo test -p camel-test --features integration-tests --test jms_test jms_bridge_crash_before_completion_redelivers_artemis`.

**Acceptance:**
- The test passes or reports an apparatus gap.

- [x] 5.3

### Task 5.4: Threshold and redelivery disposition recording

**Files:**
- `crates/camel-test/tests/jms_test.rs` (modified)
- `docs/src/components/jms.md` (modified — disposition table placeholder replaced with observed values)

**Steps:**
1. Add `jms_bridge_threshold_reject_then_log_activemq`: a route that always errors, assert six reservations then one ack and one `error!` log with no payload.
2. Add `jms_bridge_redelivery_disposition_activemq` and `..._artemis`: record whether the broker-native limit or the bridge cap fired first and where the message went.
3. Report a mismatch with the documented precedence as a gap; do not force a pass.

**Tests:**
- `jms_bridge_threshold_reject_then_log_activemq`, `jms_bridge_redelivery_disposition_activemq`, `jms_bridge_redelivery_disposition_artemis`; commands `cargo test -p camel-test --features integration-tests --test jms_test <name>`.

**Acceptance:**
- Observed disposition is recorded. A test-design gap is filed when the broker exposes no stable disposition signal.

- [x] 5.4

### Task 5.5: Credential broker-property negative test

**Files:**
- `crates/camel-test/tests/jms_test.rs` (modified)

**Steps:**
1. Add `jms_credentials_never_on_broker_properties_activemq`: a route sends `password`, `Authorization`, `api-key`, and `traceId` with an allowlist naming all.
2. Browse the broker properties and assert `traceId` present, no credential name or value present.

**Tests:**
- `jms_credentials_never_on_broker_properties_activemq`; command `cargo test -p camel-test --features integration-tests --test jms_test jms_credentials_never_on_broker_properties_activemq`.

**Acceptance:**
- Positive and negative assertions pass.

- [x] 5.5

## Phase 6: Native, version 0.8.0, docs

### Task 6.1: Native reflection registration

**Files:**
- `bridges/jms/src/main/resources/META-INF/native-image/reflect-config.json` (modified)
- `bridges/jms/src/main/resources/META-INF/native-image/org.rustcamel/jms-bridge/reflect-config.json` (new)
- `bridges/jms/src/test/java/org/rustcamel/jms/NativeReflectionConfigTest.java` (modified)

**Steps:**
1. Move only `{"name":"org.apache.activemq.ActiveMQConnectionFactory","allPublicMethods":true}` from the flat source resource into the namespaced resource. Quarkus shadows the flat path with generated reflection metadata. Leave unrelated legacy entries unchanged.
2. Add no package-level entry, broad transport registration, or transport option outside `jms.*`.
3. Update `NativeReflectionConfigTest` to read the namespaced JSON. Assert exactly one factory entry, `allPublicMethods == true`, and no package-level ActiveMQ entry. Assert the factory entry is absent from the flat source resource. Check actual runner-JAR delivery in Task 6.4, not through a JVM classpath assertion alone.

**Tests:**
- `native_reflection_registers_activemq_factory`: arrange both source resources; parse them in `NativeReflectionConfigTest`; assert one invocation-capable factory entry only at the namespaced path, no flat-path factory entry, and no package-level ActiveMQ entry; command Docker Gradle `test`.

**Acceptance:**
- The Java assertion passes. Task 6.4 verifies that the generated runner JAR retains the namespaced registration. Task 6.5 still requires actual native connection and broker observation.

- [x] 6.1

### Task 6.2: `JmsClientFactory` prefix precedence

**Files:**
- `bridges/jms/src/main/java/org/rustcamel/jms/JmsClientFactory.java` (modified)
- `bridges/jms/src/test/java/org/rustcamel/jms/JmsClientFactoryPrefixTest.java` (new)

**Steps:**
1. After the unchanged `new ActiveMQConnectionFactory(url)`, read the explicit connection-prefix option with ActiveMQ's `URISupport.parseParameters`. Pinned 5.19.10 has no `getConnectionIDPrefix()`; its String setter and null default make this value the effective base. Do not parse a plain no-query URI as a composite URI. Do not strip options manually.
2. When it is non-null and non-empty after construction (the ActiveMQ default is null), keep it as the base and append a per-process random token; otherwise set a random token alone.
3. Apply the same rule to `cf.getClientIDPrefix()`, which is available. Generate the process token lazily at runtime, not as a build-time-frozen final UUID.

**Tests:**
- `explicit_prefix_survives_with_process_token`: setup a URL with `jms.connectionIDPrefix=<sentinel>`; act factory build; assert the effective prefix contains `<sentinel>` and a unique token and is non-null.
- `no_explicit_prefix_uses_random_token`: setup a plain URL; act; assert the effective prefix is a random token and is non-null.
- `plain_ip_literal_url_without_query_is_accepted`: arrange `tcp://127.0.0.1:61616`; build the factory without connecting; assert construction succeeds and both prefixes contain the process token. Preserve the existing explicit, whitespace, and composite-prefix regression assertions.
- Command: Docker Gradle `test`.

**Acceptance:**
- Both tests pass; existing unique-ID regressions stay green.

- [x] 6.2

### Task 6.3: Bump bridge version to 0.8.0 in both languages

**Files:**
- `bridges/jms/build.gradle.kts` (modified)
- `crates/components/camel-jms/src/lib.rs` (modified)

**Steps:**
1. Change the Gradle default version fallback from `0.7.1` to `0.8.0`.
2. Change `pub const BRIDGE_VERSION` from `"0.7.1"` to `"0.8.0"`.
3. Preserve 0.7.1 until this task; both changes land together.

**Tests:**
- `bridge_version_pin_is_0_8_0`: setup the source; act read; assert `BRIDGE_VERSION == "0.8.0"`; command `cargo test -p camel-component-jms --lib bridge_version`.
- `gradle_default_is_0_8_0`: assert the `build.gradle.kts` fallback string; command Docker Gradle `test`.

**Acceptance:**
- Both constants are 0.8.0; no other version string changed.

- [x] 6.3

### Task 6.4: Native build acceptance

**Files:**
- `bridges/jms/build/native/jms-bridge` (build artifact, not committed)

**Steps:**
1. Run `cargo xtask build-jms-bridge --version 0.8.0` (Docker GraalVM musl path).
2. Confirm the binary exists and is statically linked. Inspect the generated `jms-bridge-0.8.0-runner.jar`: its namespaced reflection resource must contain the factory entry with `allPublicMethods == true`. Record the resource hash, native hash, and source commit. A source-classpath test does not prove delivery.

**Tests:**
- `native_build_succeeds`: build from the corrected parser and namespaced registration; assert the binary exists, `readelf -l` shows no interpreter, and the runner JAR retains the invocation-capable factory resource; command `cargo xtask build-jms-bridge --version 0.8.0` plus runner-JAR resource inspection.
- Prerequisite: Docker + image. This is time-heavy; rc-odr5c may be deferred at plan-bless.

**Acceptance:**
- Build exits 0, the binary is static, and packaged registration matches the source resource. Actual option application remains the Task 6.5 gate.

- [x] 6.4

### Task 6.5: Native live-broker sentinel proof

**Files:**
- `crates/camel-test/tests/jms_test.rs` (modified)
- `crates/camel-test/tests/support/jms_native_uri_cases.rs` (new, focused test module)
- `crates/camel-test/tests/support/activemq.rs` (modified, broker-browsed message identity accessor)

**Steps:**
1. Keep the existing test name `jms_bridge_native_jms_uri_option_applies_activemq` and move the test into the focused module. Start an isolated `ActiveMQBroker` with a dedicated consumer-free queue. Keep the existing `client_ids()` helper and its uniqueness tests unchanged.
2. The former `ClientId` oracle is invalid. `ConnectionView.getClientId()` delegates to `TransportConnection.getConnectionId`, which prefers `ConnectionInfo.clientId` when set. This behavior holds for brokers 5.18.3 and 5.19.10. Do not use it as the connection-prefix oracle.
3. Spawn two native 0.8.0 bridges against that broker with `BRIDGE_BROKER_URL` carrying `?jms.connectionIDPrefix=native-uri-sentinel-prefix`. Set `CAMEL_JMS_BRIDGE_BINARY_PATH` explicitly to the fresh native binary.
4. For each bridge, call `await_broker_connected` and `send_one` once to the dedicated queue. Wait for the broker enqueue count to reach two, then call the existing `browse_messages(queue)` Queue MBean helper. No consumer may remove the evidence before the browse.
5. Add `BrowsedMessage::jms_message_id() -> Option<&str>` to read the actual `JMSMessageID` broker field without formatting other message fields. Add `fn connection_id_from_broker_message_id(message_id: &str) -> Result<String, String>`: validate the ActiveMQ message identity's three numeric suffix components, then recover the preceding non-empty producer connection identifier. Strip the standard leading `ID:` marker only when present. Return fixed, value-free errors for malformed identities. Derive no identity from a send response, factory getter, input URL, or MBean object-name key. The bridge and test must not set `JMSMessageID`.
6. Require exactly two broker-browsed message IDs. Each recovered connection identifier must contain the literal sentinel `native-uri-sentinel-prefix`. The identifiers must differ. Reuse `process_token_segment` and add `fn validate_distinct_process_tokens(connection_ids: &[String]) -> Result<(), String>`: reject missing or invalid UUID tokens and require two distinct token segments. This assertion detects image-build token freezing. Keep the query as `jms.connectionIDPrefix`; do not add or substitute a client-prefix sentinel.
7. Run `jms_two_bridge_processes_same_binary_coexist` and `jms_message_ids_unique_across_bridge_restart` against the same fresh artifact. If native connection, broker browse, identity validation, or token assertions fail, leave 6.5 unchecked and report the gap. A metadata or JVM-only result never passes this gate.

**Tests:**
- `jms_bridge_native_jms_uri_option_applies_activemq`: arrange the isolated queue and two native bridges with the connection-prefix sentinel; connect and send once per bridge, then browse both messages at the broker; assert two valid message-derived connection IDs containing the sentinel and different process UUID tokens; command `cargo test -p camel-test --features integration-tests --test jms_test jms_bridge_native_jms_uri_option_applies_activemq` with explicit fresh `CAMEL_JMS_BRIDGE_BINARY_PATH`.
- `broker_message_identity_rejects_invalid_suffixes`: arrange valid ActiveMQ broker message IDs, missing components, an empty connection identifier, and non-numeric suffixes; act `connection_id_from_broker_message_id`; assert only valid producer-derived identities are accepted and errors omit the input value; command `cargo test -p camel-test --features integration-tests --test jms_test broker_message_identity_rejects_invalid_suffixes`. Expected: missing parser fails compilation before implementation, then the cases pass.
- `broker_identity_tokens_detect_frozen_process_token`: arrange two valid broker message IDs with different sequence suffixes but one shared process UUID; parse their connection identifiers and call `validate_distinct_process_tokens`; assert rejection. Repeat with two different valid process UUIDs and assert acceptance. Missing or invalid UUIDs must fail. Whole-ID inequality alone must not pass; command `cargo test -p camel-test --features integration-tests --test jms_test broker_identity_tokens_detect_frozen_process_token`. Expected: missing validator fails compilation before implementation, then all cases pass.

**Acceptance:**
- The fresh native test observes the sentinel in two producer connection identities derived from actual broker-browsed messages. Their process tokens differ, and both existing cross-process uniqueness regressions pass.
- The observer uses no manufactured message ID, client-prefix substitute, factory echo, or JVM-only result. If the contract cannot be observed, the task stays unchecked and a test-design gap remains open.

- [x] 6.5

### Task 6.6: Documentation updates

**Files:**
- `docs/src/components/jms.md` (modified)
- `bridges/jms/README.md` (modified)
- `crates/components/camel-jms/CONTEXT.md` (modified)
- `docs/adr/0067-jms-message-type-forwarding-policy.md` (modified)

**Steps:**
1. Document settlement, lazy journal ownership, the cap and precedence, the terminal reject-then-log discard as the explicit exception to at-least-once, the non-absolute cap with retention, and topic non-durability.
2. Document operator recovery for a parked slot.
3. Document `jmsHeaderAllowlist` and the breaking empty default: user headers are no longer forwarded unless they are allowlisted.
4. Update the ADR-0067 per-type table with the terminal-discard exception.

**Tests:**
- `docs_mention_terminal_discard_and_precedence`: setup the docs; act a text search; assert each file states the terminal discard and names native precedence; command `cargo xtask lint-context-citations` and a `rg` check in the task.
- Command: `cargo xtask lint-log-redaction` and `cargo xtask lint-secrets` exit 0.

**Acceptance:**
- Every doc states the terminal-discard exception, the operator recovery action, and the `jmsHeaderAllowlist` empty default.

- [x] 6.6

### Task 6.7: Final quality gates

**Files:**
- none (verification only)

**Steps:**
1. Run the Rust gates.
2. Run the Java Docker Gradle test command.
3. Run `openspec validate jms-bridge-080 --strict`.

**Tests:**
- `cargo fmt --check --all`
- `cargo clippy -p camel-component-jms -p camel-bridge --all-targets -- -D warnings`
- `cargo test -p camel-component-jms --lib`
- Docker Gradle `test`
- `openspec validate jms-bridge-080 --strict`

**Acceptance:**
- All commands exit 0.

- [x] 6.7

## Scenario coverage table

| Scenario | Owning task |
|---|---|
| Successful route commits exactly once | 3.2 |
| Propagated route error rolls back | 3.2 |
| ChannelClosed never commits | 3.5 |
| Handled error and Stop commit | 3.5 |
| Cancel before the decision prevents acknowledgement | 3.2 |
| Cancel after the decision does not undo it | 3.2 |
| Lost commit response still stands | 3.2 |
| Conversion failure recovers before the next receive | 3.3 |
| Bridge-managed settlement ignores endpoint options | 3.3 |
| A slow healthy route stays pending | 3.2 |
| Consumers of one bridge process share one budget | 3.4/3.7 |
| Cross-process duplicates are bounded | 3.7 |
| Unset env defers the failure to queue subscribe | 3.4/3.6 |
| Invalid env is a startup permanent failure | 3.4/3.6 |
| Sequential owners inherit counts | 2.2 |
| Rust owns the key and the default directory | 3.6 |
| Broker and destination namespaces stay separate | 3.7 |
| No raw identifiers in paths | 3.7 |
| Producer and topic paths never lock | 3.4/3.7 |
| Topic scope is explicit | 3.7/6.6 |
| Lock is acquired at first subscribe | 3.4 |
| Lock is released when the last subscription ends | 3.4 |
| Busy lock fails after a bounded wait | 3.4 |
| Readiness validates only the directory | 3.4 |
| Same journal domain forces one owner | 2.2 |
| Kernel releases the lock at process exit | 5.2 |
| Orphan bridge releases on stream end | 3.4 |
| Torn tail keeps confirmed counts | 2.1 |
| Snapshot replacement is atomic and durable | 2.4 |
| Space is reserved and the ceiling includes snapshots | 2.3 |
| Stale entries retire at compaction | 2.5 |
| The current reservation is never retired | 2.5 |
| No live eviction | 2.5 |
| Compaction runs at open and before the ceiling decision | 2.4 |
| Out-of-range configuration is a startup permanent failure | 2.6 |
| The cap is not absolute forever | 2.5/6.6 |
| Six reservations then terminal | 3.8 |
| Terminal is acknowledge plus one error log | 3.8/5.4 |
| Bridge cap precedence is explicit | 5.4/6.6 |
| Redelivery destination is not guaranteed | 5.4 |
| Behavior is verified with both brokers | 5.4 |
| Startup permanent failure parks the slot | 4.2 |
| Waiter on a Parked slot fails fast | 4.3 |
| Destination permanent failure fails only the route | 4.4 |
| Any consumer task failure cancels siblings | 4.5 |
| Requested cancellation is normal | 4.5 |
| Restart against a broken journal does not hot-loop | 4.6 |
| Unkeyable ID preserves the message | 4.4 |
| Transient transport failures still reconnect | 4.6 |
| Operator recovery is documented | 4.7/6.6 |
| Kill at the handoff barrier redelivers the message | 5.2 |
| Survivor acquires the journal and sees the count | 5.2 |
| The harness reaps the orphan bridge | 5.2 |
| Classic and Artemis coverage | 5.3 |
| Empty allowlist forwards no user headers | 1.1/1.2 |
| A listed business header is forwarded | 1.1/1.2 |
| Reserved and framework-internal names are denied even when listed | 1.2 |
| Credential variants are denied | 1.2 |
| Credentials never reach the wire | 1.2/1.4/5.5 |
| Java applies the same rules | 1.3/1.4 |
| Invocation-capable class-level registration | 6.1 |
| Explicit prefix survives with uniqueness | 6.2 |
| Live-broker connection observes the sentinel | 6.5 |
| The native build still succeeds | 6.4 |
| ObjectMessage forwarded empty and never deserialized | 3.3 |
| MapMessage forwarded empty without map access | 3.3 |
| StreamMessage forwarded empty without stream reads | 3.3 |
| Policy is documented | 6.6 |
