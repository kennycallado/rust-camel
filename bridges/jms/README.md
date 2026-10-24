# JMS Bridge

A Quarkus-based gRPC bridge that exposes JMS messaging to the Rust runtime. The bridge connects to the configured broker (ActiveMQ Classic or Artemis) and relays messages over mTLS gRPC.

## Environment Variables

| Variable                        | Default                 | Description                                                                                                          |
| ------------------------------- | ----------------------- | -------------------------------------------------------------------------------------------------------------------- |
| `JMS_MAX_BODY_BYTES`            | `16777216` (16 MiB)     | Message body cap in bytes. Covers both `BytesMessage` and `TextMessage` bodies.                       |
| `JMS_HEALTH_BROKER_PROBE_MODE`  | `reuse`                 | Broker diagnostic strategy for health calls. Valid values: `reuse`, `skip`. Unsupported values abort startup. |
| `BRIDGE_BROKER_URL`             | `tcp://localhost:61616` | Broker connection URL.                                                                                                |
| `BRIDGE_BROKER_TYPE`            | `activemq`              | Broker adapter type. Valid values: `activemq`, `artemis`.                                                             |
| `BRIDGE_USERNAME`               | _(none)_                | Broker username.                                                                                                      |
| `BRIDGE_PASSWORD`               | _(none)_                | Broker password.                                                                                                      |
| `BRIDGE_BROKER_KEYSTORE_PATH`   | _(none)_                | PKCS12 keystore for secure broker schemes (legacy, Artemis-only). Operator-provided.                                  |
| `BRIDGE_BROKER_TRUSTSTORE_PATH` | _(none)_                | PKCS12 truststore for secure broker schemes (legacy, Artemis-only). Operator-provided.                                |
| `BRIDGE_BROKER_KEYSTORE_PASSWORD` | _(none)_              | Keystore password (legacy, Artemis-only).                                                                             |
| `QUARKUS_OTEL_SDK_DISABLED`        | `true`                | Set `false` to enable the bridge's OTel SDK (spans continue the Rust-side trace). Fail-closed pair with the endpoint var. |
| `QUARKUS_OTEL_EXPORTER_OTLP_TRACES_ENDPOINT` | _(none)_    | Trusted OTLP collector, e.g. `http://collector:4317`. Required when the SDK is enabled: startup aborts without it (no dial-out defaults). |

### Body cap

A malformed, non-positive, or above-ceiling `JMS_MAX_BODY_BYTES` value aborts startup. The cap applies to both body types: `BytesMessage` bodies are checked against the pre-read body length, and `TextMessage` bodies are materialized and UTF-8 encoded before enforcement — the cap bounds the forwarded body size, not the peak sidecar allocation (a transient ~2-3x allocation of oversized text precedes rejection). The ceiling is 19 MiB. Operators must respect the ordering constraint: the cap stays at or below 19 MiB, and the Rust IPC decode limit is 20 MiB. The headroom absorbs IPC framing overhead (destination, headers, content type), so a message accepted by the bridge is always decodable on the Rust side.

## Broker status and health probes

The health RPC returns a cached broker status snapshot. It does not wait for broker connect, start, inspection, or close.

- `JMS_HEALTH_BROKER_PROBE_MODE=reuse` retains one provider diagnostic connection across refreshes. The bridge does not open a fresh broker connection for each health check.
- `JMS_HEALTH_BROKER_PROBE_MODE=skip` performs no broker I/O and opens no broker connection. Connectivity stays unverified.
- Unsupported values abort startup.

The snapshot has a 10 s TTL (`HEALTH_TTL_MS`). A background refresh starts after 5 s. One owned worker performs at most one refresh at a time, and there is no refresh queue. A snapshot or in-flight refresh older than the TTL reports stale status with its age. A not-yet-checked broker reports unknown status. Skipped status is explicit. Unknown, skipped, and stale status never claim verified broker connectivity.

The broker-latency isolation guarantee requires this updated Java health handler. No protobuf change is needed. An older blocking bridge cannot provide the guarantee.

### Advertised capabilities

The health RPC's `capabilities` list reports what this bridge build supports:

- `broker-tls-v1` — Classic broker TLS setup through the Rust `tls` table (see Broker TLS (Classic) below).
- `message-selector-v1` — consumer-side broker filtering support: `SubscribeRequest` accepts a `message_selector` and the broker applies it during delivery.

## Message-type forwarding policy (ADR-0067)

The bridge forwards broker messages to a bytes-only proto. Only `BytesMessage` and `TextMessage` carry a body. The other JMS types arrive empty-bodied with headers preserved. See [ADR-0067](../../docs/adr/0067-jms-message-type-forwarding-policy.md).

| JMS type | Body forwarded | Representation | Rationale |
| -------- | -------------- | -------------- | --------- |
| `BytesMessage` | yes | raw bytes | byte-intact |
| `TextMessage` | yes | UTF-8 text with `ContentType` property as `content_type`, fallback `text/plain` | |
| `ObjectMessage` | empty | none | never deserialized — gadget risk |
| `MapMessage` | empty | none | no canonical wire representation chosen yet — see ADR |
| `StreamMessage` | empty | none | sequential accessor reads would imply parsing the stream — never invoked |

### Terminal discard

The types above settle through the completion path. Object, Map, and Stream messages still forward an empty body and never invoke a body accessor. The terminal reject-then-log discard is the single exception to at-least-once: when a message reaches `JMS_MAX_DELIVERY_ATTEMPTS`, the bridge acknowledges it once and logs the message ID and the count only. The discard applies to every message type, not only the empty-bodied types.

## Broker URL Schemes

The broker URL scheme selects plaintext or TLS connection setup:

- Plaintext schemes: `tcp://`, `ws://`. No TLS material is required.
- Secure schemes: `ssl://`, `wss://`. Classic brokers configure TLS through the `tls` table (see below); Artemis uses the legacy keystore env contract (Artemis-only).

### Broker TLS (Classic)

Classic (`activemq`) brokers with an `ssl://` URL configure TLS through the Rust `tls` table, which reaches the bridge as the `BRIDGE_BROKER_TLS_*` paths/SNI environment: `ca_cert_path` pins a CA (it replaces platform trust), no CA means platform trust, and a `client_cert_path`/`client_key_path` pair adds mTLS. Startup aborts only when configured material is unusable (placeholder, missing, or unreadable) or the scheme has no registered transport.

### TLS material contract (legacy Artemis keystore env, Artemis-only)

When the broker URL scheme is `ssl://` or `wss://`, startup aborts unless all of the following hold:

1. `BRIDGE_BROKER_KEYSTORE_PATH` and `BRIDGE_BROKER_TRUSTSTORE_PATH` point to existing PKCS12 files.
2. `BRIDGE_BROKER_KEYSTORE_PASSWORD` is set.
3. `BRIDGE_BROKER_TYPE` is `artemis`. This keystore env contract is Artemis-only: Classic configures TLS through the `tls` table (see above).

Paths containing the `placeholder-` marker are rejected even when the file exists. A secure scheme never falls back to a plaintext connection.

### Example — secure Artemis broker

```bash
BRIDGE_BROKER_URL=ssl://broker.example.com:61617
BRIDGE_BROKER_TYPE=artemis
BRIDGE_BROKER_KEYSTORE_PATH=/etc/camel/broker-keystore.p12
BRIDGE_BROKER_TRUSTSTORE_PATH=/etc/camel/broker-truststore.p12
BRIDGE_BROKER_KEYSTORE_PASSWORD=changeit
```

## Settlement

The consumer runs with `CLIENT_ACKNOWLEDGE` and holds one unacknowledged delivery per session. The Rust consumer submits with `send_and_wait`. Route success sends `COMMIT` and acknowledges once. A handled error and a `Stop` commit. A propagated error and a closed channel send `ROLLBACK` and recover the session. A cancel before the decision prevents acknowledgement. A lost `COMMIT` response stands and is not retried. A healthy slow route has no completion deadline.

## Attempt journal

The bridge records each delivery as a durable reservation before handoff. It caps each `JMSMessageID` at `JMS_MAX_DELIVERY_ATTEMPTS` reservations (default 6). The seventh receipt is the terminal reject-then-log discard. The bridge never forwards to a dead-letter destination. This discard is the explicit exception to at-least-once.

Rust provides `JMS_ATTEMPT_JOURNAL_DIR` and `JMS_BRIDGE_BROKER_KEY` to the bridge. They are not operator inputs. Rust resolves the default directory from `$XDG_STATE_HOME` else `$HOME/.local/state`. When either variable is unset, readiness still succeeds and only a queue subscribe fails with `FAILED_PRECONDITION`.

The journal domain is the tuple of directory, broker key, and destination key. One live owner holds it at a time. The bridge acquires it at the first subscribe and releases it when the last subscription ends. Producers and topic subscribers never lock. A busy lock waits a bounded time, then the subscribe fails with `FAILED_PRECONDITION`. A new owner inherits the recorded counts through lock handoff. The cap is not absolute: at compaction the bridge retires entries untouched longer than the retention window (default 24 hours). Retirement never applies to the current reservation, and the bridge never evicts a live reservation. Duplicates across distinct domains are bounded at N times the cap, where N is the number of domains within one retention window.

Topic subscribers use a per-subscription volatile counter. The topic cap is not crash-stable, and the counter resets on disconnect. Durable topic subscribers are unsupported.

## Broker-native precedence

The bridge terminal cap applies only when it fires before a provider or broker native redelivery or expiry limit. Where the native limit fires first, it takes precedence, and the bridge does not claim its terminal policy ran. A redelivery is not guaranteed to land on the source queue. The bridge asserts only that the message was not acknowledged and was made available again.

Observed with default ActiveMQ Classic configuration: the bridge cap fired first, the source queue was empty, and `ActiveMQ.DLQ` `EnqueueCount` was 0. Artemis destinations were not observed. That test-design gap is `rc-5c6yv`. Not every native-first ordering or dead-letter query is verified.

## Operator recovery

On a permanent failure, follow a strict sequence.

1. Stop the route and its context.
2. Confirm that bridge and journal ownership has ended before you edit any file. The last subscription for the affected domain must be released, so its reference count reaches 0. Wait for the bridge process and its file lock to be released.
3. Only then repair or remove the corrupt journal or the unkeyable source for the affected domain.
4. Restart the context.

Do not edit a live journal file while a route or bridge holds the lock. An unkeyable broker message has no usable `JMSMessageID`. The bridge preserves it for redelivery and does not acknowledge it.

## Header forwarding

The Rust producer forwards an outgoing exchange header only when the `jmsHeaderAllowlist` endpoint option names it. The allowlist is empty by default. This is a breaking change: user headers are not forwarded unless allowlisted. Credentials are denied before the allowlist check, together with reserved JMS names and the `jms` and `camel` framework namespaces.

Use valid JMS property names, such as `traceId` and `orderId`. The producer does not sanitize property names. Hyphenated names can be dropped by Artemis (`rc-ooczk`). This release does not fix that gap.

## Native image

The ActiveMQ Classic factory needs invocation-capable reflection in the native image. Only `org.apache.activemq.ActiveMQConnectionFactory` is registered, at class level, with `allPublicMethods`. The entry lives in the namespaced resource `META-INF/native-image/org.rustcamel/jms-bridge/reflect-config.json`.

Quarkus generates its own flat `META-INF/native-image/reflect-config.json`. The namespaced resource survives that generated collision. Protect this one entry during a tracing-agent regeneration. Legacy flat entries are shadowed. The bridge does not claim they are delivered. Do not add a package-level `TransportFactory` registration.

## Connection identity

`JmsClientFactory` keeps an explicit `jms.connectionIDPrefix` as the base and appends a per-process token. When no explicit prefix is set, it uses a random per-process token alone. The process token is generated at runtime, not frozen at build time. The pinned ActiveMQ client has no connection-prefix getter, so the factory reads the option with `URISupport.parseParameters` after the unchanged constructor.

The native proof derives each producer connection identity from the broker-browsed `JMSMessageID`, using its right three numeric suffix components. It does not use the `ClientId` attribute. For ActiveMQ 5.18.3 and 5.19.10, that attribute prefers `ConnectionInfo.clientId`, which the JMS client ID drives, not the connection prefix.
