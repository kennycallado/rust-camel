# Design: jms-bridge-tls-durable-topics

## Diagnosis (evidence, main@7c0f35e9e)

1. **Classic `ssl://` — landed** (`dff018c7e`). Historical refusal point: no `ssl`
   transport factory in the native-image workarounds, so `verifySchemesResolvable`
   (`bridges/jms/src/main/java/org/rustcamel/jms/JmsClientFactory.java:676-690`) aborted
   with `UNSUPPORTED_TRANSPORT`. Today `registerTransportFactories()`
   (`JmsClientFactory.java:656-661`) registers `CamelSslTransportFactory`;
   `buildActiveMqFactory` (`:241-290`) selects `BridgeActiveMqConnectionFactory` when
   `urlHasSslMember`; material comes from `BRIDGE_BROKER_TLS_*`
   (`crates/services/camel-bridge/src/process.rs:100-112,434-443`; Rust validation
   `crates/components/camel-jms/src/config.rs:773,987-1092`).
   `ServerNamePreservingSSLSocket.java:47-57` forces `endpointIdentificationAlgorithm=HTTPS`
   — decision **D1** (verification always on; `server_name` overrides only the expected
   name) is implemented on Classic. Evidence: JVM `TlsTransportMatrixTest` and native
   `crates/camel-test/tests/jms_tls_test.rs` (CI-owned, `#[ignore]`, real containerized
   Classic broker).
2. **Durable topic subscribers — absent.** `JmsConsumer.runOwner`
   (`JmsConsumer.java:336-342`) creates only a plain consumer; proto `SubscribeRequest`
   (`bridges/jms/src/main/proto/jms_bridge.proto:25-29`) carries no client id /
   subscription name; `JmsEndpointConfig` (`config.rs:315`) has no params; README
   line 103: "Durable topic subscribers are unsupported."
3. **Artemis verifyHost unpinned** — `transportConfig` (`JmsClientFactory.java:419-514`)
   never sets `verifyHost`; Artemis 2.36 default unconfirmed (rc-ewfom, open, P2).

## Approach

**Durable subscribers** (mirror the existing capability-gate architecture):

- Proto: `SubscribeRequest` gains `string client_id = 4; string subscription_name = 5;`.
  All three copies stay byte-identical: `bridges/jms`, `crates/components/camel-jms`,
  `crates/camel-fake-bridge`.
- Rust: known params `clientId`/`client_id` + `subscriptionName`/`subscription_name`
  (`KNOWN_PARAM_NAMES`, `KnownParam` enum, `JmsEndpointConfig` fields, `from_uri`),
  threaded into the `SubscribeRequest` construction (`consumer.rs:576-580`).
  Validation (fail-loud, ADR-0032 posture — operator config is trusted input, still
  validated): both-or-neither; non-blank; only valid on `topic:` destinations; a
  queue destination with either param is a permanent config error.
- Java: when both fields are set and the destination is a topic,
  `JmsConsumer.runOwner` calls `connection.setClientID(clientId)` **before**
  `connection.start()` and `session.createDurableConsumer(topic, subscriptionName,
  selector-or-null, noLocal=false)`. Either field alone, or either on a queue, is a
  permanent error (INVALID_ARGUMENT). Collision classification is NEW work: today
  `BrokerFailureClassifier` maps only `javax.jms.IllegalStateException` to
  `CLIENT_ID_IN_USE` (`BrokerFailureClassifier.java:112-115`); the durable work adds
  `javax.jms.InvalidClientIDException` (a `JMSException` subclass the broker throws
  on colliding client ids) to the same class, credential-safe.
- Reconnect identity: the Rust consumer loop re-subscribes with the identical
  durable identity (`client_id` + `subscription_name` stable from route config;
  each replacement stream keeps its existing fresh `subscription_id`), so the same
  broker subscription re-attaches and messages published while disconnected are
  delivered after reconnect. Durable endpoints require `concurrentConsumers` of
  exactly 1 (rejected at URI validation otherwise): one client id serves exactly
  one dedicated connection.
- Capability gate: `JmsBridgeService` advertises `durable-subscribers-v1`
  (alongside `broker-tls-v1`, `message-selector-v1`, `JmsBridgeService.java:32-35,274-275`);
  the Rust side refuses a durable route against a bridge that does not advertise it
  (same permanent-refusal pattern as `ensure_tls_capability`), closing the
  silent-downgrade hazard (proto3 ignores unknown fields).
- Artemis D1 alignment: pin `verifyHost=true` explicitly in `transportConfig` for
  secure schemes (constant name verified against the pinned 2.36 jar at
  implementation time). rc-ewfom stays open; this closes its D1-alignment half.

**Decisions**: D1 (inherited) verification always on. D2: `noLocal=false`, fixed —
not configurable in this change. D3: durable subscriptions persist across route
cancel and bridge restart (that is their contract); no unsubscribe RPC; cleanup is
broker-side, documented. D4: the explicit client id must win over the Classic
`clientIDPrefix` autogeneration ("used if the JMS client does not explicitly
specify one"); pinned by an executable test asserting `getClientID()` equals the
configured id on a started dedicated connection. D5: client id uniqueness is the
operator's duty (`concurrentConsumers` must be 1 on a durable endpoint); colliding
subscribers fail loud via the broker, and this change adds the
`InvalidClientIDException` → `CLIENT_ID_IN_USE` classifier mapping so that failure
classifies without echoing credential or remote text.

## Affected crates

- `bridges/jms` (Java): proto, `JmsConsumer` durable construction, `JmsBridgeService`
  capability, `transportConfig` verifyHost, README; version bump `bridges/versions.toml`.
- `crates/components/camel-jms`: params, validation, proto, capability gate.
- `crates/camel-fake-bridge`: proto sync only.
- `crates/camel-test`: durable-reconnect row in the CI-owned harness.
- `CHANGELOG.md`: one entry.

## Architecture boundaries

Services boundary intact: the bridge owns JMS semantics; Rust validates config and
gates capabilities (ADR-0041 metadata/capability posture). No journal/framing change;
the record path is untouched. Settlement stays bridge-managed `CLIENT_ACKNOWLEDGE`.
ADR-0086 health semantics unchanged; ADR-0054 governs the new `#[ignore]` row.

## Alternatives considered

- Unsubscribe-on-cancel parameter: rejected — new surface, contradicts the durable
  contract, non-goal.
- Client id from env: rejected — the mission forbids ad-hoc env vars; the route URI
  is the existing bridge surface.
- `checkSubscriptionName`-style JMS 2.0 shared durables: rejected — not portable to
  the Classic client's `createDurableConsumer(Topic, String, String, boolean)` shape.
