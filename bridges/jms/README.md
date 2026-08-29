# JMS Bridge

A Quarkus-based gRPC bridge that exposes JMS messaging to the Rust runtime. The bridge connects to the configured broker (ActiveMQ Classic or Artemis) and relays messages over mTLS gRPC.

## Environment Variables

| Variable                        | Default                 | Description                                                                                                          |
| ------------------------------- | ----------------------- | -------------------------------------------------------------------------------------------------------------------- |
| `JMS_MAX_BODY_BYTES`            | `16777216` (16 MiB)     | Message body cap in bytes. Covers both `BytesMessage` and `TextMessage` bodies.                       |
| `BRIDGE_BROKER_URL`             | `tcp://localhost:61616` | Broker connection URL.                                                                                                |
| `BRIDGE_BROKER_TYPE`            | `activemq`              | Broker adapter type. Valid values: `activemq`, `artemis`.                                                             |
| `BRIDGE_USERNAME`               | _(none)_                | Broker username.                                                                                                      |
| `BRIDGE_PASSWORD`               | _(none)_                | Broker password.                                                                                                      |
| `BRIDGE_BROKER_KEYSTORE_PATH`   | _(none)_                | PKCS12 keystore for secure broker schemes. Operator-provided.                                                         |
| `BRIDGE_BROKER_TRUSTSTORE_PATH` | _(none)_                | PKCS12 truststore for secure broker schemes. Operator-provided.                                                       |
| `BRIDGE_BROKER_KEYSTORE_PASSWORD` | _(none)_              | Keystore password.                                                                                                    |

### Body cap

A malformed, non-positive, or above-ceiling `JMS_MAX_BODY_BYTES` value aborts startup. The cap applies to both body types: `BytesMessage` bodies are checked against the pre-read body length, and `TextMessage` bodies are checked against the materialized text length (`TextMessage` exposes no pre-read size). The ceiling is 19 MiB. Operators must respect the ordering constraint: the cap stays at or below 19 MiB, and the Rust IPC decode limit is 20 MiB. The headroom absorbs IPC framing overhead (destination, headers, content type), so a message accepted by the bridge is always decodable on the Rust side.

## Message-type forwarding policy (ADR-0067)

The bridge forwards broker messages to a bytes-only proto. Only `BytesMessage` and `TextMessage` carry a body. The other JMS types arrive empty-bodied with headers preserved. See [ADR-0067](../../docs/adr/0067-jms-message-type-forwarding-policy.md).

| JMS type | Body forwarded | Representation | Rationale |
| -------- | -------------- | -------------- | --------- |
| `BytesMessage` | yes | raw bytes | byte-intact |
| `TextMessage` | yes | UTF-8 text with `ContentType` property as `content_type`, fallback `text/plain` | |
| `ObjectMessage` | empty | none | never deserialized — gadget risk |
| `MapMessage` | empty | none | no canonical wire representation chosen yet — see ADR |
| `StreamMessage` | empty | none | sequential accessor reads would imply parsing the stream — never invoked |

## Broker URL Schemes

The broker URL scheme selects plaintext or TLS connection setup:

- Plaintext schemes: `tcp://`, `nio://`, `ws://`. No TLS material is required.
- Secure schemes: `ssl://`, `wss://`. The full TLS material contract below applies.

### TLS material contract (secure schemes)

When the broker URL scheme is `ssl://` or `wss://`, startup aborts unless all of the following hold:

1. `BRIDGE_BROKER_KEYSTORE_PATH` and `BRIDGE_BROKER_TRUSTSTORE_PATH` point to existing PKCS12 files.
2. `BRIDGE_BROKER_KEYSTORE_PASSWORD` is set.
3. `BRIDGE_BROKER_TYPE` is `artemis`. Any other broker type with a secure scheme aborts startup, because the Classic connection path does not implement this TLS contract and would silently produce a plaintext connection.

Paths containing the `placeholder-` marker are rejected even when the file exists. A secure scheme never falls back to a plaintext connection.

### Example — secure Artemis broker

```bash
BRIDGE_BROKER_URL=ssl://broker.example.com:61617
BRIDGE_BROKER_TYPE=artemis
BRIDGE_BROKER_KEYSTORE_PATH=/etc/camel/broker-keystore.p12
BRIDGE_BROKER_TRUSTSTORE_PATH=/etc/camel/broker-truststore.p12
BRIDGE_BROKER_KEYSTORE_PASSWORD=changeit
```
