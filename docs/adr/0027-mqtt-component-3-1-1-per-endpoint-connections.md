# ADR-0027 — MQTT Component: MQTT 3.1.1, per-endpoint connections, TLS basic

**Date:** 2026-06-26  
**Status:** Accepted

## Context

Implementing an MQTT component (`camel-mqtt`) for rust-camel using `rumqttc`.

## Decisions

### MQTT 3.1.1 only (v1)

MQTT 3.1.1 is widely supported (Mosquitto, EMQX, HiveMQ, AWS IoT, Azure IoT Hub).
`rumqttc` supports both 3.1.1 (`rumqttc::AsyncClient`) and 5.0 (`rumqttc::v5::AsyncClient`)
via parallel APIs. v1 uses 3.1.1; 5.0 is deferred to v2 via a separate ADR.

### One connection per Consumer/Producer (v1)

`route_id` — required for stable `client_id` generation — is not available at
`create_endpoint()` time. It arrives in `ConsumerContext::route_id()` (at `start()`)
and `ProducerContext::route_id()` (at `create_producer()`). Therefore, connections are
created lazily, not in `MqttEndpoint`.

This mirrors the Redis and Kafka patterns (no shared pool in v1). The JMS shared-pool
pattern (actor with command channel) is documented for v2 as `connectionMode=sharedSession`.

### client_id generation

MQTT 3.1.1 specifies a portable maximum of 23 bytes for `ClientIdentifier`.
Format: `{prefix}-{route_id}-{uri_hash_6}`, SHA-256 hashed and truncated to 23 bytes
if longer. Explicit `clientId` URI param overrides this.

The hash input for the producer is the **full endpoint URI** (not just the broker name)
so that two producers on the same broker but different publish topics receive distinct
`client_id`s. The consumer uses `{broker_name}-{subscriptions}` as the hash key.

### TLS basic via rustls (v1); mTLS deferred (v2)

`mqtts://` scheme activates TLS via `rumqttc`'s `use-rustls` feature. Custom CA via
`tls_ca_cert` path. mTLS (client certificate) requires `rumqttc::TlsConfiguration`
`client_auth` field — deferred to v2.

### manual ack requires cleanSession=false

With `ackMode=manual` and QoS 1/2, broker redelivery on reconnect depends on session
persistence. `cleanSession=true` discards session state on reconnect, making the
at-least-once guarantee best-effort only. Config validation rejects this combination.

The ack decision is computed from the **received packet QoS** (`publish.qos`), not the
statically configured subscription QoS. This matters because a subscription at QoS 1 can
still deliver QoS 0 messages, which must never be manually acked.

### Reconnect backoff via NetworkRetryPolicy

Connection retries use the shared `camel_component_api::NetworkRetryPolicy`
(exponential backoff with jitter, capped by `max_delay`) rather than hardcoded sleeps,
consistent with the CXF consumer (ADR-0013). An endpoint-level override
(`MqttEndpointConfig.reconnect`) takes precedence over the component-level default.

Every backoff sleep is cancellation-aware: the `tokio::time::sleep` is wrapped in a
`select!` against the consumer/producer `CancellationToken` so shutdown is not delayed.

### Crate migration: rumqttc 0.24 → rumqttc-v4-next 0.33.2

Post-implementation, the `camel-component-mqtt` crate was migrated from `rumqttc 0.24` to
`rumqttc-v4-next 0.33.2` (a community fork) to clear 4 active RUSTSEC advisories in
`rustls-webpki 0.102.8`, an EOL line that the original `rumqttc 0.24`/`0.25` pins via
`rustls 0.22`. The fork tracks `rustls 0.23` / `rustls-webpki 0.103`, which receives
patches. The dependency is aliased via `package = "rumqttc-v4-next"` in `Cargo.toml`,
preserving all `rumqttc::` paths in source code.

Key API changes applied (see commit for full diff):
- `MqttOptions::new(id, (host, port))` — broker is a tuple, not two separate args.
- `set_keep_alive(u16)` — arg is seconds as `u16`, not `Duration`.
- `set_credentials(.., password: Into<Bytes>)` — password is byte-oriented.
- `AsyncClient::builder(opts).capacity(N).build()` — builder pattern replaces
  `AsyncClient::new(opts, N)`.
- `Publish.topic` → `Bytes` (was `String`).

MSRV was bumped from 1.85 to 1.89 (the fork's minimum).

## Consequences

- v1: one TCP connection per route Consumer and one per Producer endpoint.
- Broker connection limits are a user concern in v1; document as known limitation.
- v2: shared session actor pattern avoids connection growth.
- `error!` calls classified `outside-contract` follow ADR-0012 and carry a replacement
  signal (`rt.metrics().increment_errors(...)`); the producer's driver loop logs retried
  connection errors at `warn!` level (no `error!`) since it has no runtime handle.
