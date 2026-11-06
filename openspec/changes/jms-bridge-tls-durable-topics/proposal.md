# Proposal: jms-bridge-tls-durable-topics

## Why

bd rc-c4fbo (downstream camel-112): the JMS bridge must reach production parity with a
plain ActiveMQ Classic client on two capabilities:

1. `ssl://` transport with the profile TLS options (`ca_cert`, `server_name`).
2. Durable topic subscribers (`clientId` + subscription name) surviving reconnect.

Diagnosis against `main@7c0f35e9e`:

1. **Classic `ssl://` is already landed** (commit `dff018c7e`, capability
   `broker-tls-v1`, README "Broker TLS (Classic)"). The historical refusal point — the
   `ssl` scheme had no registered transport factory in the native-image workarounds
   (`JmsClientFactory.initNativeImageWorkarounds`), so
   `verifySchemesResolvable` (`JmsClientFactory.java:676`) aborted with
   `UNSUPPORTED_TRANSPORT` — is gone: `registerTransportFactories()`
   (`JmsClientFactory.java:656`) registers `CamelSslTransportFactory`, the Rust
   `tls` table validates and exports `BRIDGE_BROKER_TLS_*`
   (`camel-bridge/src/process.rs:100-112,434-443`), and
   `ServerNamePreservingSSLSocket` pins `endpointIdentificationAlgorithm=HTTPS`
   (verification always on, `server_name` overrides the expected name only — decision
   D1). Regression evidence exists: JVM `TlsTransportMatrixTest` and the CI-owned
   native `crates/camel-test/tests/jms_tls_test.rs` against a real containerized
   Classic broker.
2. **Durable topic subscribers are unsupported** (README line 103 says so verbatim).
   `JmsConsumer.runOwner` (`JmsConsumer.java:336-342`) only ever creates a plain
   `createConsumer`; the proto `SubscribeRequest` (`jms_bridge.proto:25-29`) carries no
   client id or subscription name; the Rust `JmsEndpointConfig` has no such params.
3. **Artemis hostname verification is unpinned**: `transportConfig`
   (`JmsClientFactory.java:419-514`) never sets `verifyHost`, so behavior follows the
   Artemis 2.36 client default, which is unconfirmed (rc-ewfom). This change pins it
   per decision D1; rc-ewfom stays open for its remaining scope.

## What Changes

- Durable topic subscriptions end-to-end: new known URI params `clientId` +
  `subscriptionName` (camelCase and snake_case aliases) on `JmsEndpointConfig`, new
  `SubscribeRequest` proto fields (`client_id`, `subscription_name`), Java durable
  consumer construction (`setClientID` before `start()`,
  `createDurableConsumer(topic, name, selector, noLocal=false)`), stable identity
  across reconnects (the Rust consumer loop re-issues the same `client_id` and
  `subscription_name`, with a fresh per-stream `subscription_id`).
- New bridge capability `durable-subscribers-v1` (HealthResponse), gated on the Rust
  side like `broker-tls-v1`/`message-selector-v1` so an old bridge binary can never
  silently downgrade a durable route to a volatile subscription.
- Artemis `verifyHost=true` pinned explicitly for secure schemes (D1 alignment).
- Broker-backed testcontainers regression rows (CI-owned `#[ignore]`, extending the
  `jms_tls_test.rs` harness): `ssl://` round-trip is already there; add a durable
  subscriber row that proves survival across disconnect/reconnect, plus JVM unit
  tests for durable construction and Rust param/validation tests.
- Docs: bridge README durable-subscriber section (replacing the "unsupported" line)
  with a working example; CHANGELOG entry; bridge version bump (`bridges/versions.toml`).

Explicitly excluded: Artemis-specific behavior beyond the D1 `verifyHost` pin, any
change to the journal/framing format or the bridge record path, an unsubscribe RPC
(durable subscriptions persist by design; cleanup is broker-side, documented), and
ad-hoc environment variables for the new options.

## Acceptance criteria

- `ssl://` to ActiveMQ Classic works from the bridge with the `tls` table options,
  with hostname verification always on (existing evidence re-recorded, not regressed).
- A durable topic subscriber survives broker disconnect/reconnect: messages published
  while disconnected are received after re-subscribe with the same
  `clientId`/`subscriptionName` (broker-backed test).
- Durable routes against a bridge without `durable-subscribers-v1` fail permanently,
  never silently non-durable.
- JVM tests for durable consumer construction and Artemis `verifyHost` pinning; Rust
  tests for param parsing and validation; docs updated for both features.

## Risk budget

- Client-id identity across reconnects comes from route config (stable), not runtime
  state. Collision hazard (two consumers, one client id) is fail-loud via the broker;
  this change adds the `InvalidClientIDException` → `CLIENT_ID_IN_USE` classifier
  mapping (the current mapping covers only `IllegalStateException`).
- One client id serves exactly one consumer: durable endpoints require
  `concurrentConsumers` unset or `1`; larger values are a permanent URI error.
- The ActiveMQ Classic clientIDPrefix autogeneration must not defeat the explicit
  client id; pinned by an executable test (D4 in design.md).
- Out of bounds: journal/framing, record path, new env vars, Artemis feature work.

Affected crates/bridges: `bridges/jms` (Java), `crates/components/camel-jms`,
`crates/camel-fake-bridge` (proto sync), `crates/camel-test` (integration rows),
`crates/services/camel-bridge` (only if env-plumbing tests need the new none).
bd: rc-c4fbo.
