# Tasks: 389-jms-selector

## camel-component-jms (Rust)

### Task 1.1: Add `message_selector` to SubscribeRequest and plumb from Rust consumer

**Files:**
- `bridges/jms/src/main/proto/jms_bridge.proto` (modified)
- `crates/components/camel-jms/proto/jms_bridge.proto` (modified)
- `crates/components/camel-jms/src/consumer.rs` (modified)
- `crates/components/camel-jms/src/config.rs` (modified)
- `crates/components/camel-jms/src/bridge_client_test.rs` (modified — struct literals gain the new field)
- `crates/components/camel-jms/src/consumer.rs` test module `SubscribeRequest` literals (modified, same file)

**Steps:**
1. In BOTH proto copies, add `string message_selector = 3;` to `message SubscribeRequest`
   (fields 1 `destination`, 2 `subscription_id` unchanged). Verify copies byte-identical:
   `diff bridges/jms/src/main/proto/jms_bridge.proto crates/components/camel-jms/proto/jms_bridge.proto`
   exits 0. Do NOT touch `SendRequest`.
2. In `crates/components/camel-jms/src/consumer.rs` at the subscribe construction site
   (~line 546), build the request with
   `message_selector: endpoint_config.message_selector.clone().unwrap_or_default()` (prost
   maps proto3 `string` to `String`; absent selector = empty `String`). Remove the
   `TODO(JMS-010)` marker and the `let _selector = selector;` discard at lines 432-434.
3. Update every existing `SubscribeRequest { .. }` struct literal in test code
   (`bridge_client_test.rs`, `consumer.rs` tests) with `message_selector: String::new()` or
   `..Default::default()` so the crate compiles.
4. In `config.rs` `from_uri` parsing (the `"messageSelector" | "message_selector"` arm, line
   ~432): percent-DECODE the value with the crate's existing decode utility (the same helper
   other URI options use; locate it in `config.rs`/`camel-endpoint`) and store the DECODED
   string in `message_selector` — an encoded selector reaching the broker would filter on the
   wrong expression. A decoded value that is non-empty but whitespace-only after `trim()` is a
   validation error whose message names `messageSelector` (follow the existing URI-validation
   error type used by `from_uri`; do not invent a new error type). Empty value keeps the
   existing behavior: `None`. The existing `parse_message_selector` test currently pins the
   RAW encoded value `"priority%20%3E%205"` (config.rs:2004-2009) — update its expectation to
   the decoded `"priority > 5"`; this is a deliberate, justified behavior fix (decode boundary
   moves to parse time), not a silent test change.
5. Run `cargo build -p camel-component-jms` (proto codegen via build.rs picks up the new field).

**Tests:** (executable spec)
- `selector_reaches_subscribe_request`: setup — in-process bridge mock that records incoming
  `SubscribeRequest` (existing pattern in `consumer.rs` tests, e.g. the mock at line ~1796);
  route URI `jms:queue:orders?message_selector=priority%20%3E%205`; act — start consumer,
  await subscription setup; assert — captured request has `message_selector == "priority > 5"`
  and `destination == "queue:orders"`. Command: `cargo test -p camel-component-jms --lib
  selector_reaches_subscribe_request`. Expected before fix: compile error (no field) — the
  field's existence IS the failing-first state.
- `no_selector_request_unchanged`: setup — same mock, route `jms:queue:orders` (no selector
  option); act — subscribe; assert — captured `message_selector.is_empty()` and
  destination/subscription_id equal the values captured by the pre-change assertions already
  present in the test module. Command: `cargo test -p camel-component-jms --lib
  no_selector_request_unchanged`.
- `parse_whitespace_only_message_selector_rejected`: setup — none; act —
  `JmsEndpointConfig::from_uri("jms:queue:orders?message_selector=%20%20")`; assert —
  `is_err()` and the error text contains `messageSelector`. Command: `cargo test
  -p camel-component-jms --lib parse_whitespace_only_message_selector_rejected`. Fails
  (returns Ok) before the fix.
- Existing `parse_message_selector` (config.rs ~2004) is UPDATED in step 4 to expect the
  decoded `Some("priority > 5".to_string())` (was raw `"priority%20%3E%205"`), and
  `parse_empty_message_selector_is_none` (~2013) must still pass unchanged — together they pin
  decode-at-parse and absent/empty → `None`.

**Acceptance:**
- `diff` of the two proto files exits 0
- `cargo test -p camel-component-jms --lib` passes
- `cargo fmt --check` and `cargo clippy -p camel-component-jms -- -D warnings` exit 0
- No `SendRequest` change (`git diff` shows no `SendRequest` hunk)

- [x] 1.1

### Task 1.2: Fail-closed `message-selector-v1` capability gate (Rust)

**Files:**
- `crates/components/camel-jms/src/diagnostics.rs` (modified)
- `crates/components/camel-jms/src/component.rs` (modified — two gate sites, mirroring the
  `broker-tls-v1` gate)

**Steps:**
1. In `diagnostics.rs`, next to `MISSING_BROKER_TLS_CAPABILITY` (line ~21) and
   `ensure_tls_capability` (line ~51), add:
   `pub(crate) const MISSING_MESSAGE_SELECTOR_CAPABILITY: &str = "lacks message-selector-v1;
   use a jms bridge >= 0.11.0";` and
   `pub(crate) fn ensure_selector_capability(capabilities: &[String], selector_configured: bool)
   -> Result<(), ...>` returning the same error shape as `ensure_tls_capability`; errors only
   when `selector_configured && !capabilities.iter().any(|c| c == "message-selector-v1")`.
   Reuse the existing error type/shape of `ensure_tls_capability` exactly.
2. Dynamic requirement registration (unlike `tls_configured`, which is static broker config
   captured once at `component.rs:1032-1047`, the selector requirement is per-endpoint and can
   attach late): the slot state gains `selector_required: Arc<AtomicBool>` (next to the
   existing slot fields, `component.rs:388-407`). The consumer-attach path sets it `true`
   BEFORE any gate evaluation whenever the endpoint configures a non-blank selector. The
   startup gate and the runtime Health monitor READ this shared flag on every evaluation —
   no closure capture — so a selector attached after slot start is covered by the runtime
   gate too. No capabilities are STORED on the slot (none exist today; do not add a
   capability cache — see step 5).
3. Startup gate: in `component.rs` at the startup capability-gate site (the
   `seen_capabilities` collection + `ensure_tls_capability` startup check, lines ~1500-1560),
   add the selector check on the same collected evidence, armed by the shared
   `selector_required` flag: when required and the startup Health evidence lacks the
   capability, park the slot permanently BEFORE the first Subscribe. Error text must contain,
   all three: `message-selector-v1`, `0.11.0`, and the pinned bridge version
   (`self.bridge_version`, the `crate::BRIDGE_VERSION`-backed field at line ~496 — same
   substitution as the TLS text at line ~1551). Follow the same flow as the TLS gate
   (permanent park reason).
4. Runtime gate: in `component.rs` at the per-Health-response gate site (lines ~1087-1108,
   the `ensure_tls_capability` call inside the Health monitor), add the matching
   `ensure_selector_capability` call reading the SHARED `selector_required` flag (fresh read
   per response, not captured), so capability loss parks permanently and stops resubscription
   with the same error text — including for consumers that attached after monitor start.
5. Consumer-attach gate (covers "startup is bypassed" for late routes; `BridgeSlot` stores no
   capability evidence and none is retained from runtime responses, so the attach path uses a
   BOUNDED FRESH HEALTH CHECK instead of stored state): when a selector-configured consumer
   attaches (at slot start or late), after setting `selector_required`, issue one fresh
   Health RPC via the existing health client with its existing timeout; gate on THAT
   response's capabilities. Missing capability, or RPC error/timeout, parks that consumer
   permanently BEFORE its first Subscribe — with the three-fragment error for a response
   lacking the capability, or a "could not verify message-selector support" message (naming
   the capability and required version) for an RPC failure. No silent start in either arm.

**Tests:** (executable spec)
- `ensure_selector_capability_unit`: setup — none; act/assert —
  `ensure_selector_capability(&[], false)` is `Ok`; `ensure_selector_capability(&["message-selector-v1".into()],
  true)` is `Ok`; `ensure_selector_capability(&["broker-tls-v1".into()], true)` is `Err` and
  the error text contains `message-selector-v1` and `0.11.0`. Command: `cargo test
  -p camel-component-jms --lib ensure_selector_capability_unit`. Fails to compile before step 1.
- `startup_parks_when_selector_configured_and_capability_missing`: setup — scripted bridge
  mock (`spawn_scripted_health`, consumer.rs test module) whose HealthResponse has
  `capabilities: vec![]`, route with `message_selector`; act — start the slot; assert —
  slot parks with permanent error whose text contains `message-selector-v1`, `0.11.0`, and
  the pinned bridge version string, and the mock recorded ZERO Subscribe calls. Command:
  `cargo test -p camel-component-jms --lib
  startup_parks_when_selector_configured_and_capability_missing`.
- `startup_subscribes_when_selector_capability_present`: setup — mock HealthResponse with
  `capabilities: vec!["broker-tls-v1".into(), "message-selector-v1".into()]`, selector
  route; act — start slot; assert — a Subscribe call arrives and carries the selector.
  Command: `cargo test -p camel-component-jms --lib
  startup_subscribes_when_selector_capability_present`.
- `no_selector_old_bridge_unaffected`: setup — mock HealthResponse with empty capabilities,
  route WITHOUT selector; act — start slot; assert — subscription proceeds (gate trivially
  passes). Command: `cargo test -p camel-component-jms --lib no_selector_old_bridge_unaffected`.
- `runtime_capability_loss_parks_permanently`: setup — mock whose first Health response
  includes `message-selector-v1` and later responses omit it (scripted sequence), selector
  route; act — run until the second response is observed; assert — permanent park with the
  missing-capability error and no further Subscribe calls. Command: `cargo test
  -p camel-component-jms --lib runtime_capability_loss_parks_permanently`. This test is also
  the owning evidence for the spec scenario "replacement bridge cannot inherit previous
  evidence": the later responses model the replacement instance and their capability list
  governs.
- `startup_gate_ignores_broker_health_status`: setup — mock HealthResponse with
  `status: STALE` AND `capabilities: vec!["message-selector-v1".into()]`, selector route;
  act — start slot; assert — gate passes on the capability alone and Subscribe proceeds
  (broker health status neither bypasses nor weakens the capability check). Command:
  `cargo test -p camel-component-jms --lib startup_gate_ignores_broker_health_status`.
- `replacement_bridge_startup_parks_without_inherited_evidence`: setup — two sequential slot
  cycles over the same mock server handle: cycle 1's Health evidence includes
  `message-selector-v1`; simulate restart by starting a fresh slot cycle whose evidence
  collection starts empty (the existing TLS gate-window re-collection mechanism) against a
  mock (or later scripted responses) that reports NO capabilities; act — let the replacement
  cycle evaluate its startup gate; assert — the replacement parks permanently at startup
  with the missing-capability error and ZERO Subscribe calls are sent to the replacement
  (cycle 1's evidence did not carry over). Command: `cargo test -p camel-component-jms --lib
  replacement_bridge_startup_parks_without_inherited_evidence`.
- `selector_on_already_running_incapable_slot_parks_before_subscribe`: setup — slot running
  against an incapable bridge (fresh Health response with empty capabilities) with a
  NON-selector consumer active; act — attach a second consumer whose route configures a
  selector (startup gate was already bypassed before the selector route existed); assert —
  the attach-path fresh Health check observes the missing capability and the consumer is
  parked permanently with the three-fragment error BEFORE any Subscribe is sent for it (the
  incapable slot's Subscribe-call count does not increase). Command: `cargo test
  -p camel-component-jms --lib selector_on_already_running_incapable_slot_parks_before_subscribe`.
- `late_selector_on_capable_slot_then_capability_loss_parks`: setup — slot running against a
  capable bridge with a NON-selector consumer; act — attach a selector consumer (fresh Health
  check passes, Subscribe proceeds), then the scripted bridge's Health responses stop
  advertising `message-selector-v1` (replacement by an older build); assert — the shared
  `selector_required` flag makes the runtime gate park permanently on the next Health
  response with the three-fragment error and no further Subscribe calls (registration
  semantics cover the late consumer). Command: `cargo test -p camel-component-jms --lib
  late_selector_on_capable_slot_then_capability_loss_parks`.

**Acceptance:**
- `cargo test -p camel-component-jms --lib` passes
- `cargo test -p camel-component-jms` (full crate) passes
- `cargo fmt --check` and `cargo clippy -p camel-component-jms -- -D warnings` exit 0
- Startup park text asserted to contain all three fragments (capability, 0.11.0, pinned version)
- No new capability cache or version channel introduced (gate reads existing Health evidence)

- [x] 1.2

## bridges/jms (Java)

### Task 2.1: Java selector overload + capability advertisement

**Files:**
- `bridges/jms/src/main/java/org/rustcamel/jms/JmsBridgeService.java` (modified)
- `bridges/jms/src/main/java/org/rustcamel/jms/JmsConsumer.java` (modified)
- `bridges/jms/src/test/java/org/rustcamel/jms/JmsBridgeServiceHealthTest.java` (modified)
- `bridges/jms/src/test/java/org/rustcamel/jms/JmsConsumerSelectorTest.java` (new)

**Steps:**
1. In `JmsBridgeService.java`, next to `CAP_BROKER_TLS_V1` (line 32) add
   `static final String CAP_MESSAGE_SELECTOR_V1 = "message-selector-v1";`. In
   `healthResponse(...)` (line ~266) add `.addCapabilities(CAP_MESSAGE_SELECTOR_V1)` after
   the existing `.addCapabilities(CAP_BROKER_TLS_V1)`.
2. In `JmsBridgeService.subscribe` (line ~92), thread the selector from the request into the
   `JmsConsumer` instance obtained from `consumerFactory.get()` using the same mechanism the
   request's other fields reach the consumer (read the class: fields are assigned on the
   injected instance before the consume loop starts; add a `messageSelector` field the same
   way `destination`/`subscriptionId` are set, default `null`).
3. In `JmsConsumer.java` consume loop at `session.createConsumer(dest)` (line ~320):
   when `messageSelector != null && !messageSelector.isBlank()` call
   `session.createConsumer(dest, messageSelector)`, otherwise keep the one-argument call.
   `JmsProducer.parseDestination` unchanged. Producer path untouched.
4. Do not add durable-consumer paths (`createDurableConsumer` stays out — M2/M3 scope).

**Tests:** (executable spec)
- `healthResponseAdvertisesSelectorCapability` (in `JmsBridgeServiceHealthTest`): setup —
  build a `HealthSnapshot` as the existing test does; act — call the static
  `healthResponse(snapshot, failureClass)`; assert — the response's capabilities list contains
  BOTH `broker-tls-v1` and `message-selector-v1`. Command (from `bridges/jms`):
  `./gradlew test --tests 'org.rustcamel.jms.JmsBridgeServiceHealthTest'`. Fails before step 1.
- `consumerUsesSelectorOverload` (new `JmsConsumerSelectorTest`): setup — a JMS
  ConnectionFactory mock/stub (follow the stubbing style of the existing
  `JmsConsumerCompletionTest`/`JmsConsumerMessageTypesTest` for session/consumer mocks);
  a subscribe request carrying `messageSelector = "priority > 5"`; act — run the consumer
  setup path; assert — `session.createConsumer(dest, "priority > 5")` was invoked (capture
  the two-arg call), one-arg `createConsumer(dest)` was NOT invoked. Command: from
  `bridges/jms`, `./gradlew test --tests 'org.rustcamel.jms.JmsConsumerSelectorTest'`.
- `consumerWithoutSelectorUsesPlainOverload` (same new test class): request with null/blank
  selector; assert — exactly the one-arg `createConsumer(dest)` call. Same command as above.
- `invalidSelectorSurfacesThroughFailurePath` (same new test class): setup — session stub
  whose `createConsumer(dest, selector)` throws `jakarta.jms.InvalidSelectorException` (or
  the provider equivalent the existing failure tests use) for selector `"NONE SUCH >"`; act —
  run the consumer setup path; assert — the error reaches the existing subscription failure
  path (the same `safeOnError`/observer error notification the other failure tests assert),
  with NO local parsing or rewriting of the selector string before the call. Command: from
  `bridges/jms`, `./gradlew test --tests 'org.rustcamel.jms.JmsConsumerSelectorTest'`.
  This is the executable evidence for the spec scenario "invalid selector syntax is a
  broker-owned failure" at the JVM plumbing level; end-to-end semantics stay with the
  live-broker suite.

**Acceptance:**
- `./gradlew test` (all bridge JVM tests) passes from `bridges/jms`
- No `createDurableConsumer` occurrence added
- Producer files (`JmsProducer.java`, `SendRequest` handling) show no diff

- [x] 2.1

## camel-test + docs

### Task 3.1: Strict-walk recognition + docs + live-broker selector test

**Files:**
- `crates/components/camel-jms/src/metadata.rs` (modified only if the strict walk does not
  already recognize `messageSelector` — verify first)
- `crates/components/camel-jms/README.md` (modified)
- `docs/src/components/jms.md` (modified)
- `bridges/jms/README.md` (modified)
- `crates/camel-test/tests/jms_test.rs` (modified)

**Steps:**
1. Strict-walk check: locate the strict unknown-parameter walk for JMS endpoint URIs (grep
   `strict`/`unknown` in `crates/components/camel-jms/src/` and its URI-parsing entry points;
   the rc-d8svj policy). If `messageSelector` is already recognized (via the
   `#[uri_param(name = "messageSelector")]` registration at `metadata.rs:24` or the walk's own
   allowlist), no code change — record that in the task report. ONLY if it is NOT recognized,
   add exactly that recognition (one allowlist entry / metadata field alignment) and nothing
   else.
2. `crates/components/camel-jms/README.md` endpoint option table: add/extend the
   `messageSelector` row: applied at the broker (non-matching messages are withheld before
   delivery), provider-specific SQL-like syntax (not parsed or validated by rust-camel beyond
   non-blank), requires jms bridge >= 0.11.0 advertising `message-selector-v1`, and the
   startup error shape: the slot parks with
   `bridge <version> lacks message-selector-v1; use a jms bridge >= 0.11.0`.
3. `docs/src/components/jms.md`: update the `messageSelector` row (line ~81) and the
   consumer section text (line ~182): same facts as the README row, plus explicitly: JVM and
   Rust unit tests prove plumbing only; selector semantics are verified by the CI-owned
   live-broker suite. State empty option (`message_selector=`) means no selector, absent
   option means no selector, whitespace-only is a URI validation error.
4. `bridges/jms/README.md`: in the capabilities list introduced by the TLS work, add
   `message-selector-v1` with one sentence (consumer-side broker filtering support).
5. `crates/camel-test/tests/jms_test.rs`: add live-broker tests using the shared ActiveMQ
   container (`support/activemq.rs`, `#[tokio::test(flavor = "multi_thread")]`, same marking
   as the rest of the file — Docker-requiring, CI-owned):
   - queue: start a route with `message_selector=(priority > 5)` style property filter
     (properties are set through the existing producer test helpers), publish one matching
     and one non-matching message, assert the matching message is received within the
     bounded wait the file already uses and the non-matching one is NOT received within it;
   - topic: same publish/receive pattern against a topic destination.
   These tests require Docker; they are exercised by CI (do not run them locally without
   Docker; they must compile: `cargo test -p camel-test --test jms_test --features
   integration-tests --no-run`).

**Tests:** (executable spec)
- `jms_consumer_selector_filters_activemq_queue`: setup — shared ActiveMQ container + route
  with selector; act — publish matching + non-matching; assert — matching received,
  non-matching withheld within the bounded wait. Command: `cargo test -p camel-test --test
  jms_test --features integration-tests jms_consumer_selector_filters_activemq_queue` —
  the whole file is feature-gated (`jms_test.rs:10`); CI enables the feature and builds the
  in-tree bridge (`ci.yml:102-107`); requires Docker, CI-owned; compile-check locally with
  `cargo test -p camel-test --test jms_test --features integration-tests --no-run`.
- `jms_consumer_selector_filters_activemq_topic`: same shape on a topic. Same command with
  the topic test name.
- If any camel-test integration code constructs `SubscribeRequest` literals, update them for
  the new proto field (compile gate: the `--features integration-tests --no-run` command).
- Strict-walk recognition test: if step 1 changed code — a test asserting the walk
  recognizes `messageSelector` (follow the naming/assertion pattern of the existing
  unknown-parameter tests in camel-jms); if no code change — cite the existing passing test
  that already covers recognition.
- `cd docs && mdbook test` passes (docs gate).

**Acceptance:**
- `cargo test -p camel-test --test jms_test --features integration-tests --no-run` exits 0
  (compiles without Docker)
- `cd docs && mdbook test` exits 0
- `cargo test -p camel-component-jms` (full) and `cargo test -p camel-bridge` pass
  (mission evidence bar: camel-bridge at `crates/services/camel-bridge` is untouched by this
  change — run as regression verification)
- README + guide state: broker-side filtering, 0.11.0 requirement, error shape, plumbing-only
  scope of unit tests
- No selector parser introduced (`git diff` shows no parser code)

- [ ] 3.1
