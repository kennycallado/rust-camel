# Proposal: jms-message-fidelity

## Why

The JMS bridge (`bridges/jms`) passes bytes and text through faithfully, but three
fidelity gaps make round-trips through the bridge lossy or unsafe:

1. **ContentType is dropped on TextMessage** (`rc-kzti`). The bridge's own
   producer stamps `ContentType` as a JMS string property
   (`JmsProducer.java:43`), but `JmsConsumer.convertMessage` hardcodes
   `setContentType("text/plain")` (`JmsConsumer.java:262`). A producer sending
   `application/xml` as text gets it back as `text/plain` — downstream routes
   that branch on content type misclassify the body.
2. **Duplicate `subscription_id` silently evicts** (`rc-5e4l`).
   `JmsBridgeService.subscribe` does `activeConsumers.put(subId, consumer)`
   unconditionally (`JmsBridgeService.java:62`). A second stream with the same
   ID overwrites the first stream's bookkeeping; cancelling the first stream
   then runs `cleanupSubscription`, which `remove`s the entry now owned by the
   second stream (`JmsBridgeService.java:156`) — cross-stream teardown with no
   error anywhere. The Rust side's `Uuid::new_v4()` per request
   (`consumer.rs:213`) is the only accidental protection.
3. **Object/Map/StreamMessage forwarding is undefined policy** (`rc-41h3`).
   `convertMessage` has branches only for `BytesMessage`/`TextMessage`; the
   other JMS types fall through with an EMPTY body while AUTO_ACKNOWLEDGE
   receipt acknowledges them. The behavior exists but was never decided,
   documented, or fully tested (the test suite covers ObjectMessage only).
   ObjectMessage in particular carries Java-serialization gadget risk if ever
   deserialized — the policy must pin that `getObject()` is never invoked.

## What Changes

- **Phase 1 (kzti):** `convertMessage` TextMessage branch prefers the
  `ContentType` JMS property when present and non-empty, falling back to
  `text/plain`. Mockito test proves `application/xml` survives the round trip;
  a no-property message still yields `text/plain`.
- **Phase 2 (5e4l):** `subscribe` rejects a duplicate `subscription_id` with
  `ALREADY_EXISTS` BEFORE touching `activeConsumers`; `cleanupSubscription`
  removes the map entry only if it still owns it (owner-check). New
  `JmsBridgeServiceTest` proves: second stream rejected, first stream
  unaffected, cancel of first no longer tears down second.
- **Phase 3 (41h3):** ADR-0067 records the forwarding decision inputs
  (current empty-body behavior, Java-serialization gadget risk, MapMessage
  flattening alternative) and the ruling; `bridges/jms/README.md` gains a
  per-type policy table; `JmsConsumerMessageTypesTest` grows MapMessage and
  StreamMessage cases — every type asserting body emptiness, no
  deserialization call (`verify(never())`), exactly-once acknowledgement,
  headers preserved.

Rust contract: `crates/components/camel-jms` — the bridge_client harness gains
a ContentType round-trip assertion (Phase 1) and a duplicate-subscription
rejection test (Phase 2). No Rust production code changes.

## Acceptance criteria

- TextMessage with `ContentType=application/xml` property delivers
  `content_type="application/xml"`; absent/empty property delivers
  `text/plain`. Java + Rust tests green.
- Second `subscribe` with an in-use `subscription_id` is rejected with
  `ALREADY_EXISTS` and leaves the first stream delivering; cancelling the
  first stream does not stop the second.
- ADR-0067 + README policy table cover Object/Map/Stream;
  `JmsConsumerMessageTypesTest` covers all three with body-emptiness,
  never-deserialize, exactly-once-ack, headers-preserved assertions.
- Full gate battery green in the worktree (Rust) and `./gradlew test` green
  (Java, bridges/jms).

## Risk budget

- Java-only bridge changes, additive guards; no wire-format or proto change.
- Phase 3 ruling "keep empty-body forwarding" matches current behavior — no
  wire change, so broker compatibility risk is nil.
- Collision guard could reject previously-accepted (but broken) duplicate
  streams: that is the fix, not a regression; Rust `Uuid` callers unaffected.
