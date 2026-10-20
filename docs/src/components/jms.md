# JMS

The JMS component publishes to and consumes from JMS brokers through a Java bridge process. It supports ActiveMQ Classic and ActiveMQ Artemis. One crate covers both directions. The Consumer subscribes to a destination and submits one Exchange per message. The Producer publishes the Exchange body to a destination.

The component does not implement JMS in Rust. It delegates protocol work to a native Java bridge binary over gRPC. The bridge is downloaded once and cached.

## Schemes

Three schemes share one bridge pool:

| Scheme | Shorthand | Locks broker type |
| --- | --- | --- |
| `jms` | rejected (ambiguous) | no |
| `activemq` | `activemq:orders` → queue | yes |
| `artemis` | `artemis:orders` → queue | yes |

The `jms:` scheme requires an explicit destination type. `jms:orders` returns an error. Use `jms:queue:orders` or `jms:topic:orders`.

The `activemq:` and `artemis:` schemes set the broker type at the URI level. They override any `broker_type` declared in the broker config. Use `jms:` when you want the broker type to come from configuration.

## Example

The jms-example wires a timer-driven producer and a log consumer against a testcontainers broker:

```rust,ignore
{{#include ../../../examples/jms-example/src/main.rs:jms-producer-route}}
```

<details>
<summary>YAML equivalent</summary>

```yaml
routes:
  - id: jms-producer
    from: "timer:tick?period=3000"
    steps:
      - set_body: '{"event":"order","source":"rust-camel"}'
      - to: "activemq:queue:orders"
```

The Rust example starts an ActiveMQ Classic container through testcontainers. The broker URL is read from the container port. Substitute your real broker address in production.

</details>

```rust,ignore
{{#include ../../../examples/jms-example/src/main.rs:jms-consumer-route}}
```

<details>
<summary>YAML equivalent</summary>

```yaml
routes:
  - id: jms-consumer
    from: "activemq:orders"
    steps:
      - to: "log:info?showHeaders=true"
```

The consumer uses the `activemq:orders` shorthand. It is equivalent to `activemq:queue:orders`. The destination type defaults to queue for the broker-specific schemes.

</details>

## URI

```text
jms:queue:<name>[?param=value&...]
jms:topic:<name>[?param=value&...]
activemq:queue:<name>[?param=value&...]
activemq:topic:<name>[?param=value&...]
artemis:queue:<name>[?param=value&...]
artemis:topic:<name>[?param=value&...]
activemq:<name>      # shorthand, defaults to queue
artemis:<name>       # shorthand, defaults to queue
```

| Parameter | Default | Description |
| --- | --- | --- |
| `broker` | configured `default_broker` | Named broker from the `[components.jms.brokers]` table |
| `acknowledgementMode` | `Auto` | `Auto`, `Client`, `DupsOk`, or `Transacted` |
| `messageSelector` | — | SQL-92 selector expression for filtering inbound messages |
| `concurrentConsumers` | `1` | Number of parallel consumer tasks for this endpoint |
| `transactionMode` | `None` | `None` or `Session` (Session is not yet implemented) |
| `timeToLive` | — | Message time-to-live in milliseconds |
| `priority` | — | Message priority 0-9 (9 is highest) |
| `persistentDelivery` | `true` | PERSISTENT or NON_PERSISTENT delivery mode |
| `mapJmsHeaders` | `true` | Map JMS headers and properties to Exchange headers |
| `jmsHeaderAllowlist` | empty | Comma-separated allowlist of outgoing business header names |
| `exchangePattern` | `InOnly` | `InOnly` or `InOut` (InOut is not yet implemented) |

Credentials and the broker URL are not URI parameters. They live in the `[components.jms.brokers.<name>]` table.

## Broker configuration

Brokers are declared in `Camel.toml`. The component creates one Java bridge process per broker. The bridge pool admits at most `max_bridges` (default 8) bridges concurrently.

```toml
[default.components.jms]
default_broker = "main"

[default.components.jms.brokers.main]
broker_url  = "tcp://localhost:61616"
broker_type = "activemq"   # "activemq" | "artemis"
username    = "admin"      # optional
password    = "admin"      # optional
```

`failover://` URLs are accepted for Classic (`activemq`) brokers and rejected for `artemis` (use a single primary URL or multiple broker entries).

The bridge binary downloads on first use. It comes from a configured release URL and is cached at `~/.cache/rust-camel/jms-bridge/`. The download is SHA256-verified. Set `CAMEL_JMS_BRIDGE_BINARY_PATH` to point at a local build for development.

## Consumer

`activemq:queue:orders` subscribes to a destination. The Consumer submits one Exchange per inbound JMS message. The Exchange body carries the message payload. With `mapJmsHeaders=true` (the default), the headers carry `JMSMessageID`, `JMSCorrelationID`, `JMSTimestamp`, `JMSDestination`, and `JMSPriority`.

The body is typed from the JMS `content_type`. `text/*` becomes `Body::Text`. `application/json` becomes `Body::Json` when valid JSON. Binary content becomes `Body::Bytes`. A `TextMessage` carrying an explicit `ContentType` property (for example `application/xml` from a non-Camel producer, or from this component's own producer) now keeps that value, so such payloads surface as `Body::Bytes` instead of `Body::Text`. The consumer pre-flights the bridge slot before starting. A missing or degraded bridge fails fast with `JMS bridge not available`.

The `concurrentConsumers` parameter spawns N parallel consumer tasks on the same destination. Each task subscribes independently and submits Exchanges into the shared route pipeline. `messageSelector` filters messages at the broker with a SQL-92 expression.

## Producer

`activemq:queue:orders` sends the Exchange body to a destination. The content type is inferred from the body. `Body::Text` sends `text/plain`. `Body::Json` sends `application/json`. `Body::Xml` sends `text/xml`. An explicit `Content-Type` header wins over inference.

Message type selection uses that resolved content type. A value that starts with `text/` produces a `TextMessage`. Any other value produces a `BytesMessage`. The Exchange `ContentType` header does not select the type. `ContentType` is only the JMS wire property that the bridge sets on the sent message.

The Producer uses a semaphore for backpressure. The default concurrency limit is 128 in-flight sends. When the limit is reached, `poll_ready` still returns ready. The `call` future waits on the semaphore. ADR-0024 classifies a closed semaphore as `ConsumerStopping`.

A successful send returns the Exchange unchanged. The `JMSMessageID` header carries the broker-assigned message ID. A send failure on a gRPC transport error refreshes the channel. The original send is not retried. A retry on a non-idempotent write would cause duplicates. The caller decides whether to retry.

## Header forwarding

The Producer forwards an exchange header only when `jmsHeaderAllowlist` names it. The option takes a comma-separated list. The list is empty by default.

This default is a breaking change. The Producer no longer forwards user headers unless you allowlist them. Add each business header name to `jmsHeaderAllowlist` to keep it. Example: `jms:queue:orders?jmsHeaderAllowlist=traceId,orderId`.

Denial runs before the allowlist check. The Producer denies reserved JMS names, the `jms` and `camel` framework namespaces, and credential-shaped names even when the allowlist names them. It forwards only string values. A denied name such as `api-key` or `Authorization` never reaches the broker.

Use valid JMS property names, such as `traceId` and `orderId`. The Producer does not sanitize property names. Artemis can drop a hyphenated name (`rc-ooczk`). This release declares that inherited gap. It does not fix it.

`Content-Type` stays a dedicated protocol field. Inbound mapping (the `mapJmsHeaders` option) is unchanged.

## Settlement and durable reservations

The Consumer runs the bridge session with `CLIENT_ACKNOWLEDGE` and holds one unacknowledged delivery per session. `acknowledgementMode` and `transactionMode` do not change this bridge-managed settlement. `transactionMode=Session` logs a warning.

The Rust consumer submits the route with `send_and_wait`. Route success sends `COMMIT` and acknowledges once. A handled error and a `Stop` count as success and commit. A propagated `Err` and a closed pipeline channel (`ChannelClosed`) send `ROLLBACK` and recover the session.

A cancel, a stop, or a disconnect before the decision prevents any acknowledgement. The same event after the decision does not undo it. A lost `COMMIT` response stands. The consumer does not retry it. A healthy slow route has no completion deadline.

The bridge records each delivery as a durable reservation before handoff. It caps each `JMSMessageID` at `JMS_MAX_DELIVERY_ATTEMPTS` reservations (default 6). The seventh receipt takes the terminal reject-then-log policy: the bridge acknowledges once, logs at `error!` with the message ID and the count only, and never logs the body or headers. It does not forward to a dead-letter destination. This discard is the explicit terminal exception to at-least-once, applied to every message type.

Journal ownership is lazy and per destination. The domain is the tuple of directory, broker key, and destination key. The bridge acquires the lock at the first subscribe and releases it when the last subscription ends. Producers and topic subscribers never lock. A busy lock waits a bounded time, then the subscribe fails with `FAILED_PRECONDITION`. A new owner inherits the recorded counts through lock handoff.

Rust owns the default directory and the broker key. The default directory is `$XDG_STATE_HOME/rust-camel/jms-bridge`, else `$HOME/.local/state/rust-camel/jms-bridge`. Rust passes the directory as `JMS_ATTEMPT_JOURNAL_DIR` and the key as `JMS_BRIDGE_BROKER_KEY`. These are Rust outputs, not operator inputs. When either is unset, a queue subscribe fails with `FAILED_PRECONDITION`. Producers and topic subscribers keep working.

The cap is not absolute. At compaction the bridge retires entries untouched longer than the retention window (default 24 hours). Retirement never applies to the current in-flight reservation. The bridge never evicts a live reservation to admit another. Duplicates across separate journal domains are bounded at N times the cap, where N is the number of distinct journal domains that consume the queue within one retention window.

Topic subscribers use a per-subscription volatile counter. The topic cap is not crash-stable, and the counter resets on disconnect. Durable topic subscribers are unsupported.

### Operator recovery

A startup directory failure parks the bridge slot. The slot stays parked until an operator acts. A subscribe-time permanent failure fails only that route and leaves the slot ready.

Operator recovery is a strict sequence.

1. Stop the route and its context.
2. Confirm that bridge and journal ownership has ended before you edit any file. The last subscription for the affected domain must be released, so its reference count reaches 0. Wait for the bridge process and its file lock to be released.
3. Only then repair or remove the corrupt journal or the unkeyable source for the affected domain.
4. Restart the context.

Do not edit a live journal file while a route or bridge holds the lock. An unkeyable broker message has no usable `JMSMessageID`. The bridge preserves it for redelivery and does not acknowledge it.

## Java bridge

The component does not speak JMS. A native Java bridge process handles the JMS protocol and the TCP connection. The component talks to the bridge over gRPC. The bridge binary is a GraalVM native executable. The host does not need a Java runtime.

The bridge pool assigns one bridge per broker. The first send or consume starts the bridge. The bridge's ephemeral gRPC port comes from its stdout. A health monitor probes the bridge every `health_check_interval_ms` (default 5 s) with a `health_timeout_ms` deadline (default 3 s). Only consecutive failed probes restart a bridge. The slot moves to `Restarting` only after `health_failure_threshold` consecutive failures. Restarts use exponential backoff capped at 120 seconds. After 10 failed restart attempts the slot stays `Degraded`.

The Consumer observes bridge state through a watch channel. A pending bridge returns `Pending` from `poll_ready`. A degraded bridge returns `Err` with the reason. The producer waits for the bridge to become `Ready` before sending.

## Health and broker status

A completed health RPC proves that the bridge process is alive. This rule holds when the response reports a broker problem. A failed probe is an RPC error or an expired `health_timeout_ms`. Only `health_failure_threshold` consecutive failed probes restart a bridge. A successful RPC resets the failure count.

The health RPC returns a cached broker status snapshot and does not wait for broker connectivity. The snapshot has a 10 s TTL and a background refresh starts after 5 s. The readiness adapter maps the response as follows.

| Broker status | `healthy` | `brokerConnected` | Readiness |
| --- | --- | --- | --- |
| Verified up | true | true | Healthy (200) |
| Verified down | false | false | Unhealthy (503) |
| Not yet checked | true | false | Degraded (200) |
| Skipped | true | false | Degraded (200) |
| Stale | true | false | Degraded (200) |

A response that reports the broker verified down maps to `Unhealthy` 503. It is not merely a message attached to a healthy response. A healthy RPC that reports `healthy=false` maps to `Unhealthy` 503, and the adapter does not wait for an RPC failure. An RPC error or timeout also maps to `Unhealthy` 503.

A not yet checked broker reports unknown status. Unknown, skipped, and stale status map to `Degraded` 200, so readiness passes. The process is live and ready, and broker connectivity is not verified. Broker status fields never trigger a process restart.

`health_broker_probe_mode` selects the broker probe strategy. `reuse` (the default) keeps one provider diagnostic connection across refreshes. `skip` performs no broker I/O and opens no connection. Unsupported values are rejected at startup.

`health_check_interval_ms` sets the delay between probes. `health_timeout_ms` sets the deadline for one probe. A probe that exceeds its deadline counts as a failed probe, whatever the interval.

One channel refresh has a fixed 2 s budget. The budget covers process-lock acquisition and the selected connector future. Progress does not renew the budget. After a failed or expired refresh, the caller requests a liveness probe within 500 ms. It never restarts the process. Slow broker work alone cannot restart a live bridge.

The broker-latency isolation guarantee requires the updated Java health handler. The update needs no protobuf change. An older blocking bridge cannot provide the guarantee.

## Trust boundary

Per ADR-0032, incoming JMS headers, bodies, correlation IDs, and destinations enter `exchange.input` without validation. The route is responsible for validation when data crosses into a control action, a resource decision, or an executable sink.

Credentials cross the gRPC boundary in plain text for the username and through a `Redacted` wrapper for the password. `BrokerConfig` redacts the password in `Debug` output. `BridgeSlot` omits its `credentials` field. `redact_url` strips user information from URLs before logging. The audit command `rg '#\[derive.*Debug' crates/components/camel-jms/src/` checks that no type holding a password derives `Debug`.

## Error handling

ADR-0007 governs Consumer shutdown. `JmsConsumer::stop` cancels the requested-shutdown token. It waits up to 5 seconds for each owned consumer task, then aborts the stragglers. Abort authority stays component-owned, so no worker is detached. Cancellation races the in-flight `send_and_wait` and the completion RPC. A cancel before the terminal decision drops the pending wait and the subscription without a speculative acknowledgement, so the broker or provider redelivers the message. A decision already made stands. The Consumer does not restart itself. Route supervision owns restart.

ADR-0012 classifies log sites as outside-contract or system-broken. A consumer-side `ctx.send` failure increments the `b-prime:jms:consumer-send` metric and logs at `error!`. A bridge restart that exhausts the attempt cap logs at `error!` and leaves the slot in `Degraded`.

## Redelivery and terminal disposition

The Consumer settles each delivery through the bridge with `CLIENT_ACKNOWLEDGE` and one terminal decision per reservation. A route error (`Err`, including a closed pipeline channel) settles as `ROLLBACK`: the bridge recovers the session and the broker or provider makes the message available again. A completed route (`Ok`) settles as `COMMIT` and acknowledges once. A rolled-back message is not promised to return to the source queue; the bridge asserts only that it was not acknowledged and is made available again.

Durable reservation accounting caps each `JMSMessageID` at `JMS_MAX_DELIVERY_ATTEMPTS` reservations (validated, default 6). The receipt whose recorded count has already reached the cap applies the terminal **reject-then-log** policy: the bridge acknowledges the message once and logs at `error!` with the provider message ID and the recorded count only — never the body, headers, or credential values. It does not forward to a dead-letter destination. This discard is an explicit terminal exception to at-least-once, applied to every message type. Repeated crashes before handoff can exhaust the budget before any route runs.

The bridge cap is the documented terminal only where it fires before a provider or broker native redelivery or expiry limit. Where the native limit fires first, it takes precedence and the bridge does not claim its terminal policy ran. The limits were **observed with default broker and provider configuration only**; no URI option was assumed to change a broker limit. Lowering a native limit below the cap to exercise a native-first ordering was not exercised.

| Broker fixture (defaults) | Provider/broker native limit | Bridge terminal cap | Terminal that fired first | Observed destination |
| --- | --- | --- | --- | --- |
| ActiveMQ Classic 5.18.3 | provider redelivery policy default (6 redeliveries) | 6 reservations | bridge cap, on the 7th receipt | acknowledged and removed; `ActiveMQ.DLQ` `EnqueueCount` 0 |
| ActiveMQ Artemis 2.36.0 | broker `max-delivery-attempts` default (10) | 6 reservations | bridge cap, on the 7th receipt | **not observed** — the testcontainer fixture exposes no stable management query; filed as test-design gap `rc-5c6yv` |

Observed durable evidence for each run: six `RESERVE` frames, six `ROLLBACK` frames, and one terminal `COMMIT` frame for the same `JMSMessageID`; exactly six accepted route handoffs; and exactly one native `ERROR` record from `org.rustcamel.jms.JmsConsumer` carrying the provider message ID and count only. ActiveMQ Classic additionally reported numeric source `QueueSize` 0 and `ActiveMQ.DLQ` `EnqueueCount` 0.

A missing bridge terminal is not by itself read as the native limit. A native-first ordering is reported only with stable broker evidence (ActiveMQ Classic source queue empty and a non-zero `ActiveMQ.DLQ` `EnqueueCount`); otherwise the disposition is recorded as unknown. The Artemis broker-side destination is unobserved and tracked as `rc-5c6yv`. Only the observed runs above are verified. Other native-first orderings and dead-letter queries are not verified.

## Limitations

- Durable topic subscribers are not supported.
- IBM MQ is not supported.
- The `InOut` exchange pattern and `Session` transaction mode log a warning and fall back to the default.

**Reference**: [JMS crate CONTEXT](https://github.com/kennycallado/rust-camel/blob/main/crates/components/camel-jms/CONTEXT.md). Example source: [`examples/jms-example`](https://github.com/kennycallado/rust-camel/tree/main/examples/jms-example).
