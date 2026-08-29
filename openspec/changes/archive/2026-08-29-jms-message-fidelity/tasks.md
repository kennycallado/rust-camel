# Tasks: jms-message-fidelity

## Phase 1: content-type

### Task 1.1: Java consumer preserves ContentType property

- **ID:** jms-ct-java
- **Files:**
  - `bridges/jms/src/main/java/org/rustcamel/jms/JmsConsumer.java` (modified)
  - `bridges/jms/src/test/java/org/rustcamel/jms/JmsConsumerContentTypeTest.java` (new)
- **Steps:**
  1. In `convertMessage` (`JmsConsumer.java:216`), after `b.setDestination(destination)` and before the `instanceof` dispatch, add: `String contentTypeProp = null; try { contentTypeProp = msg.getStringProperty("ContentType"); } catch (JMSException ignored) { }` (defensive: providers must return null for absent, but a misbehaving provider must not kill forwarding).
  2. In the `TextMessage` branch, replace `b.setContentType("text/plain");` with: use `contentTypeProp` when it is non-null and `!contentTypeProp.isEmpty()`; otherwise `"text/plain"` (whitespace-only values are preserved — the blessed spec only special-cases empty). Keep the property-enumeration loop untouched (the property stays in headers).
  3. Do NOT touch the `BytesMessage` branch — it sets no content type.
- **Tests** (`JmsConsumerContentTypeTest.java`, Mockito harness mirroring `JmsConsumerMessageTypesTest` setup: mocked `factory`/`connection`/`session`/`messageConsumer`, `RecordingObserver` from `JmsConsumerBodyCapTest`):
  - `contentTypePropertyPreservedOnTextMessage`
    - setup: `TextMessage` mock; `when(tm.getStringProperty("ContentType")).thenReturn("application/xml")`; `when(tm.getText()).thenReturn("<a/>")`; `when(tm.getPropertyNames()).thenReturn(Collections.enumeration(List.of("ContentType")))`; `when(tm.getObjectProperty("ContentType")).thenReturn("application/xml")`; `stubCommonAttributes` equivalent for id/correlation/timestamp.
    - action: `consumer.subscribe(DESTINATION, SUB_ID, obs, new AtomicBoolean(false))`; await `obs.next`; `consumer.stop()`.
    - assert: `obs.last.get().getContentType()` equals `"application/xml"`; `getBody().toStringUtf8()` equals `"<a/>"`; `getHeadersMap().get("ContentType")` equals `"application/xml"` (spec: property stays in headers).
    - command: `./gradlew test --tests 'org.rustcamel.jms.JmsConsumerContentTypeTest'` (cwd `bridges/jms`)
    - expected: fails before step 2 (returns `text/plain`), passes after.
  - `absentContentTypeFallsBackToTextPlain`
    - setup: same but `getStringProperty("ContentType")` returns `null`.
    - assert: content type `"text/plain"`.
  - `emptyContentTypeFallsBackToTextPlain`
    - setup: `getStringProperty("ContentType")` returns `""`.
    - assert: content type `"text/plain"`.
  - `bytesMessageContentTypeStaysEmpty`
    - setup: `BytesMessage` mock with `getBodyLength()` returning 3, `readBytes` filling 3 bytes; `getStringProperty("ContentType")` returns `"application/xml"`; `getPropertyNames()` → enumeration of `"ContentType"`; `getObjectProperty("ContentType")` returns `"application/xml"`.
    - assert: `getContentType()` is `""`; body has the 3 bytes; `getHeadersMap().get("ContentType")` equals `"application/xml"`.
- **Acceptance:**
  - `./gradlew test --tests 'org.rustcamel.jms.JmsConsumerContentTypeTest'` exits 0.
  - `rg -n 'setContentType\("text/plain"\)' bridges/jms/src/main/java/org/rustcamel/jms/JmsConsumer.java` shows the literal only inside the fallback expression (no unconditional hardcode).
- `[x] jms-ct-java`

### Task 1.2: Rust bridge contract round-trips content type

- **ID:** jms-ct-rust
- **Files:**
  - `crates/components/camel-jms/src/bridge_client_test.rs` (modified)
- **Steps:**
  1. Add test `content_type_round_trips_through_stream`: build `JmsMessage { content_type: "application/xml".to_string(), body: b"<a/>".to_vec(), ..Default::default() }` reusing the struct-literal pattern of `near_cap_body_decodes_end_to_end` (`bridge_client_test.rs:90`).
  2. Spawn `spawn_mock_bridge(message)`, connect, `client.subscribe(SubscribeRequest { destination: "queue.ct.test", subscription_id: "ct-round-trip".into() })`.
  3. Assert `delivered.content_type == "application/xml"` and `delivered.body == b"<a/>"` byte-intact — pins the proto field as part of the bridge contract the Java side must fill (Task 1.1) and the Rust `consumer.rs:57` text/json branching depends on.
- **Tests:** as above; command `cargo test -p camel-component-jms --lib bridge_client_test` (worktree); expected: passes once Task 1.2 written (contract pin — the field already flows; this guards regressions).
- **Acceptance:**
  - `cargo test -p camel-component-jms --lib` exits 0.
  - `cargo fmt --check` and `cargo clippy -p camel-component-jms --all-targets -- -D warnings` exit 0.
- `[x] jms-ct-rust`

## Phase 2: subscription-guard

### Task 2.1: Java service rejects duplicate subscription IDs

- **ID:** jms-sub-java
- **Files:**
  - `bridges/jms/src/main/java/org/rustcamel/jms/JmsBridgeService.java` (modified)
  - `bridges/jms/src/test/java/org/rustcamel/jms/JmsBridgeServiceTest.java` (new)
- **Steps:**
  1. In `subscribe` (`JmsBridgeService.java:59`): replace `activeConsumers.put(subId, consumer)` with `JmsConsumer existing = activeConsumers.putIfAbsent(subId, consumer);`.
  2. When `existing != null`: call `consumerFactory.destroy(consumer)` (return the fresh consumer — no leak), then `responseObserver.onError(Status.ALREADY_EXISTS.withDescription("subscription_id already active: " + subId).asException());` and `return;` — all BEFORE registering `setOnCancelHandler` or calling `consumer.subscribe`.
  3. In `cleanupSubscription` (`JmsBridgeService.java:156`): replace `activeConsumers.remove(subId)` with `activeConsumers.remove(subId, consumer)` (two-arg owner-checked remove).
- **Tests** (`JmsBridgeServiceTest.java`, same package `org.rustcamel.jms` — package-private field access is legal; `@Mock jakarta.enterprise.inject.Instance<JmsConsumer> instance` stubbed `get()` to return mock consumers A then B, assigned directly to `service.consumerFactory` (the package-private `Instance` field, `JmsBridgeService.java:27`); `StreamObserver<JmsMessage>` mocks for responses; a small reflection helper `activeConsumersOf(service)` reads the `private final ConcurrentHashMap` field for ownership assertions):
  - `duplicateSubscriptionIdRejectedAlreadyExists`
    - setup: consumer A's `subscribe` stubbed with a NON-BLOCKING `doAnswer` that captures its inner `StreamObserver<JmsMessage>` argument and returns null (stream 1 stays "active" because nothing triggers its cleanup — no latch); first `service.subscribe(req(s1), obsA)` invoked.
    - action: second `service.subscribe(req(s1), obsB)` with consumer B from `instance.get()`.
    - assert: `obsB` receives `onError` whose `Status.fromThrowable(error).getCode() == Status.Code.ALREADY_EXISTS` (house style `asException()` produces a `StatusException`, not `StatusRuntimeException` — capture `Throwable`); `verify(instance).destroy(consumerB)` (rejected consumer returned); `activeConsumersOf(service).get("s1")` is consumer A.
    - ALSO spec clause "first stream continues delivering": invoke the captured inner-A observer's `onNext(jmsMessage)` and assert `obsA.onNext` received it (rejection did not disturb stream 1).
  - `cancelledFirstStreamDoesNotEvictSecondOwner` (same-key reuse; the live protector on THIS path is the `finished` CAS — the two-arg remove is defense-in-depth)
    - setup: subscribe `"s1"` (consumer A); drive stream 1 to completion via the response-observer `onCompleted` path (`JmsBridgeService.java:98-99`) so `cleanupSubscription` removes the entry; subscribe `"s1"` again (consumer B).
    - action: invoke the captured inner-A `StreamObserver`'s `onError(...)` (NOT `obsA` — the response mock cannot execute service cleanup) — the finished stream's error path runs `cleanupSubscription` again, `finished.compareAndSet` returns false, cleanup is a no-op.
    - assert: `activeConsumersOf(service).get("s1")` is still consumer B; `verify(consumerB, never()).stop()`.
  - `differentlyKeyedCancellationLeavesOtherIntact` (spec scenario s1/s2)
    - setup: subscribe `"s1"` (A) and `"s2"` (B), both with captured inner observers.
    - action: complete stream `"s1"` via its `onCompleted` path.
    - assert: `activeConsumersOf(service)` contains `"s2"` only; `verify(consumerB, never()).stop()`; `verify(instance, never()).destroy(consumerB)`; ALSO invoke captured inner-B `onNext(jmsMessage)` and verify `obsB.onNext(jmsMessage)` received it (s2 still delivers).
  - `rejectedStreamRegistersNoCleanup`
    - setup: `ServerCallStreamObserver<JmsMessage> serverObsB = mock(ServerCallStreamObserver.class)` as observer 2; duplicate subscribe rejected as in the first test.
    - assert: `verify(serverObsB, never()).setOnCancelHandler(any())` (no getter exists in the gRPC API — absence-of-registration is the check).
  - command: `./gradlew test --tests 'org.rustcamel.jms.JmsBridgeServiceTest'` (cwd `bridges/jms`); expected RED before implementation: tests 1 AND 4 (`duplicateSubscriptionIdRejectedAlreadyExists` — no rejection, `put` overwrites; `rejectedStreamRegistersNoCleanup` — old code registers the cancel handler unconditionally). Tests 2-3 pin-pass (CAS + key isolation already hold).
- **Acceptance:**
  - `./gradlew test --tests 'org.rustcamel.jms.JmsBridgeServiceTest'` exits 0.
  - `rg -n 'activeConsumers.put\(' bridges/jms/src/main/java/org/rustcamel/jms/JmsBridgeService.java` returns no hits (put replaced by putIfAbsent).
  - `rg -n 'activeConsumers.remove\(subId, consumer\)' bridges/jms/src/main/java/org/rustcamel/jms/JmsBridgeService.java` returns 1 hit.
- `[x] jms-sub-java`

### Task 2.2: Rust client surfaces ALREADY_EXISTS

- **ID:** jms-sub-rust
- **Files:**
  - `crates/components/camel-jms/src/bridge_client_test.rs` (modified)
- **Steps:**
  1. Extend the mock bridge (`bridge_client_test.rs:44` region): track `active_ids: Arc<Mutex<HashSet<String>>>` in the service struct; in `subscribe`, if the request's `subscription_id` is already in the set, return `Err(Status::already_exists("subscription_id already active"))`; otherwise insert and stream.
  2. Add test `duplicate_subscription_id_surfaces_already_exists`: spawn the extended mock, subscribe twice with `subscription_id: "dup-test"` (sequential awaits on two connected clients — the first await completes insertion before the second subscribe runs; the set has no removal path).
  3. Assert the second `client.subscribe(...).await` returns `Err` with `tonic::Code::AlreadyExists`.
- **Tests:** as above; command `cargo test -p camel-component-jms --lib bridge_client_test`; expected: fails before step 1 (mock accepts duplicates), passes after.
- **Acceptance:**
  - `cargo test -p camel-component-jms --lib` exits 0.
  - `cargo fmt --check` and `cargo clippy -p camel-component-jms --all-targets -- -D warnings` exit 0.
- `[x] jms-sub-rust`

## Phase 3: forwarding-policy

### Task 3.1: ADR-0067 records the forwarding decision

- **ID:** jms-adr
- **Files:**
  - `docs/adr/0067-jms-message-type-forwarding-policy.md` (new)
  - `CONTEXT-MAP.md` (modified — one line in the ADR index list, matching existing entry style)
- **Steps:**
  1. Write ADR-0067 with sections: Context (bridge forwards broker messages to a bytes-only proto; Object/Map/Stream have no branch today), Decision inputs (three, exactly as blessed in `design.md` Phase 3: current empty-body behavior with `JmsConsumer.java:132` AUTO_ACKNOWLEDGE; Java-serialization gadget risk under ADR-0032 exchange-data trust; MapMessage flattening rejected — numeric type preservation, `byte[]` representation, null semantics, canonical media type + versioning), Decision (forward Object/Map/Stream with empty body, headers preserved, receipt acknowledges, body accessors never invoked), Consequences (no deserialization attack surface; content-producers must use Bytes/Text; flattening revisitable via new ADR).
  2. Add the index line to `CONTEXT-MAP.md` ADR list: `- [0067](./docs/adr/0067-jms-message-type-forwarding-policy.md) — JMS message-type forwarding policy: Object/Map/Stream forward empty-bodied with headers preserved; body accessors never invoked (deserialization stays out of the bridge)`.
- **Tests:** none (document). Verify: `rg -n "0067" CONTEXT-MAP.md docs/adr/0067-jms-message-type-forwarding-policy.md` returns both files; ADR mentions all three types and the word "flattening".
- **Acceptance:**
  - `cargo xtask lint-context-citations` exits 0 (citation lint).
  - ADR file exists, is in the index, English prose.
- `[x] jms-adr`

### Task 3.2: README policy table

- **ID:** jms-readme
- **Files:**
  - `bridges/jms/README.md` (modified)
- **Steps:**
  1. Add section `## Message-type forwarding policy (ADR-0067)` with a 5-row markdown table: header row `| JMS type | Body forwarded | Representation | Rationale |`; rows: BytesMessage (yes, raw bytes, byte-intact), TextMessage (yes, UTF-8 text with `ContentType` property as `content_type`, fallback `text/plain`), ObjectMessage (empty, none, never deserialized — gadget risk), MapMessage (empty, none, no canonical wire representation chosen yet — see ADR), StreamMessage (empty, none, sequential accessor reads would imply parsing the stream — never invoked).
  2. Link `../../docs/adr/0067-jms-message-type-forwarding-policy.md` from the section (repo-relative path from `bridges/jms/`).
- **Tests:** none (document). Verify table renders: `rg -c "^\|" bridges/jms/README.md` ≥ 6.
- **Acceptance:**
  - README contains the section, 5-row table, ADR link; English prose.
- `[x] jms-readme`

### Task 3.3: Java test matrix pins Map and Stream behavior

- **ID:** jms-matrix-java
- **Files:**
  - `bridges/jms/src/test/java/org/rustcamel/jms/JmsConsumerMessageTypesTest.java` (modified)
- **Steps:**
  1. Add `mapMessageForwardedEmptyAndAcked`: `MapMessage` mock, `stubCommonAttributes` equivalent PLUS a real property — `getPropertyNames()` → enumeration of `"X-Trace"`, `getObjectProperty("X-Trace")` returns `"m1"` (note: `stubCommonAttributes` stubs EMPTY enumeration, so headers-preserved needs its own stubbing). `receive` returns it then null. Assert body size 0, no error outcome, one delivery, `getHeadersMap().get("X-Trace")` equals `"m1"`; `verify(connection).createSession(false, Session.AUTO_ACKNOWLEDGE)`; `verify(mapMessage, never()).getMapNames()`; `verify(mapMessage, never()).getObject(anyString())`; `verify(mapMessage, never()).acknowledge()`.
  2. Add `streamMessageForwardedEmptyAndAcked`: same shape (own `X-Trace` property stubbing + headers-preserved assert) with `StreamMessage` mock; `verify(streamMessage, never()).readInt()`; `verify(streamMessage, never()).readString()`; `verify(streamMessage, never()).readBytes(any())`; `verify(streamMessage, never()).acknowledge()`.
  3. Extend the EXISTING `unsupportedMessageTypeForwardedEmptyAndAcked`: add `verify(objectMessage, never()).acknowledge()` (aligns all three cases to the blessed ack assertion) AND add an `X-Trace` property stubbing + `getHeadersMap().get("X-Trace")` assert (spec: headers preserved).
  4. Update the class javadoc: policy now ADR-0067 (replace "policy rc-41h3" wording), all three types covered.
- **Tests:** the two new tests above; command `./gradlew test --tests 'org.rustcamel.jms.JmsConsumerMessageTypesTest'` (cwd `bridges/jms`); expected: both pass against CURRENT code (behavior already empty-body — these pin it; if either fails, the fall-through was broken and the worker must STOP and report).
- **Acceptance:**
  - `./gradlew test --tests 'org.rustcamel.jms.JmsConsumerMessageTypesTest'` exits 0 with 3 tests.
  - `rg -c "acknowledge\(\)" bridges/jms/src/test/java/org/rustcamel/jms/JmsConsumerMessageTypesTest.java` ≥ 3.
- `[x] jms-matrix-java`
