# ws-component Specification

## Purpose
The `ws:` component. Its optional client-consumer mode connects outbound,
reconnects, applies backpressure and resource limits, exposes readiness,
health, and observability, and preserves the default server-mode behavior.
## Requirements
### Requirement: Client-consumer mode

A `ws://` or `wss://` endpoint configured with `consumeAsClient=true` and
used as a route's `from:` endpoint SHALL establish an outbound WebSocket
client connection to the endpoint's host/port/path and SHALL create one new
Exchange for each text or binary frame the remote server pushes.

#### Scenario: pushed frames become exchanges

- **GIVEN** a route whose `from:` endpoint is `ws://localhost:PORT/feeds?consumeAsClient=true` and a remote WebSocket server accepting the connection
- **WHEN** the remote server sends three text frames
- **THEN** the route processor receives three exchanges whose input bodies equal the frame payloads and whose `CamelWsMessageType` header is `text`

#### Scenario: binary frame mapping

- **GIVEN** an active client-consumer connection
- **WHEN** the remote server sends one binary frame
- **THEN** the route receives one exchange with a bytes body and `CamelWsMessageType` header `binary`

#### Scenario: control frames are transparent

- **GIVEN** an active client-consumer connection
- **WHEN** the remote server sends ping and pong frames between text frames
- **THEN** no exchanges are created for the ping/pong frames and the text frames are still delivered

#### Scenario: wss client connection

- **GIVEN** a local `wss://` server whose certificate chains to a test CA trusted by the client TLS configuration and a route whose `from:` endpoint is `wss://localhost:PORT/feed?consumeAsClient=true`
- **WHEN** the consumer starts and the server pushes one text frame
- **THEN** the TLS connection is established and the frame reaches the route as an exchange

#### Scenario: subprotocol negotiation on connect

- **GIVEN** a route whose `from:` endpoint includes `consumeAsClient=true&subprotocols=vt1` and a remote server that selects subprotocol `vt1`
- **WHEN** the consumer connects
- **THEN** the upgrade request carries `Sec-WebSocket-Protocol: vt1` and the connection is established

### Requirement: Client-consumer reconnection

The client-consumer SHALL reconnect after connection loss using the
endpoint's reconnect policy (`NetworkRetryPolicy`) and SHALL resume frame
delivery on the re-established connection without route restart. Each
disconnect SHALL start one fresh bounded reconnect sequence.

#### Scenario: disconnect then reconnect resumes delivery

- **GIVEN** an active client-consumer connection with reconnect enabled
- **WHEN** the remote server drops the connection and later accepts a new one and pushes a further frame
- **THEN** the consumer reconnects within the policy bounds and the further frame reaches the route

#### Scenario: policy exhaustion surfaces failure

- **GIVEN** a client-consumer whose remote is unreachable and whose reconnect policy is exhausted
- **WHEN** the reconnect attempts run out
- **THEN** the consumer task reports failure instead of retrying silently forever

### Requirement: Client-consumer startup readiness

The client-consumer SHALL follow `ConsumerStartupMode::Explicit`: it SHALL
signal readiness only after the first outbound connection is established,
and SHALL fail startup when the initial connection cannot be established
within the reconnect policy.

#### Scenario: unreachable remote fails route start

- **GIVEN** a route whose `from:` endpoint is `ws://localhost:PORT/x?consumeAsClient=true` where nothing listens on PORT and the reconnect policy has a small bounded attempt count
- **WHEN** the route starts
- **THEN** consumer startup returns an error and the route does not silently run as a no-op

#### Scenario: reachable remote becomes ready

- **GIVEN** a reachable remote server
- **WHEN** the client-consumer starts and the connection is established
- **THEN** readiness is signalled before any frame delivery

### Requirement: Client-consumer backpressure

The client-consumer SHALL deliver exchanges through the bounded
consumer-to-pipeline channel and SHALL pause reading the WebSocket stream
while the channel is full instead of buffering frames without bound.

#### Scenario: slow route pauses reads

- **GIVEN** an active client-consumer whose route processes exchanges slower than the remote pushes frames
- **WHEN** the delivery channel is full
- **THEN** the consumer stops reading further frames until the channel drains, and no unbounded frame queue grows

### Requirement: Client-consumer lifecycle

The client-consumer SHALL reject double-start, SHALL stop idempotently, and
its receive/reconnect task SHALL observe the consumer cancellation token and
exit promptly (best-effort WebSocket Close) when shutdown is requested,
including while reading, while backpressured on the delivery channel, and
while sleeping between reconnect attempts.

#### Scenario: double-start rejected

- **GIVEN** a started client-consumer
- **WHEN** `start()` is called a second time
- **THEN** the second call returns an error and the running task is unaffected

#### Scenario: shutdown while receiving

- **GIVEN** an active client-consumer connection delivering frames
- **WHEN** shutdown is requested
- **THEN** the task exits promptly without panic, sends a best-effort Close frame, and no further exchanges are submitted

#### Scenario: shutdown during reconnect backoff

- **GIVEN** a client-consumer in a reconnect sequence with a long backoff delay
- **WHEN** shutdown is requested during the delay
- **THEN** the task exits promptly instead of waiting for the delay and remaining attempts

#### Scenario: shutdown while backpressured

- **GIVEN** a client-consumer paused on a full delivery channel
- **WHEN** shutdown is requested
- **THEN** the task exits promptly instead of blocking on the channel send

#### Scenario: stop is idempotent

- **GIVEN** a stopped client-consumer
- **WHEN** `stop()` is called again
- **THEN** the second call returns success without side effects

### Requirement: Client-consumer resource limits

The client-consumer SHALL enforce `maxMessageSize` on inbound frames by
dropping the oversized frame, recording the drop, and keeping the connection
and subsequent frame delivery intact.

#### Scenario: oversized frame dropped, flow continues

- **GIVEN** an active client-consumer configured with `maxMessageSize=1024`
- **WHEN** the remote sends one frame larger than 1024 bytes followed by one small text frame
- **THEN** no exchange is created for the oversized frame, the drop is recorded (log and error metric), and the small frame reaches the route

### Requirement: Client-consumer health

The client-consumer SHALL register a passive health check backed by the
shared connection state (`Connecting | Connected | Reconnecting |
Exhausted`) that opens no probe connections to the remote, healthy only in
the `Connected` state. The server-mode TCP-listener health check SHALL
remain unchanged.

#### Scenario: health reflects connection state

- **GIVEN** a client-consumer whose connection state transitions Connecting then Connected
- **WHEN** the health check runs in each state
- **THEN** it reports unhealthy with `Connecting` and healthy once `Connected`, without opening any TCP connection to the remote

#### Scenario: server health check unchanged

- **GIVEN** a server-mode `ws://` consumer without `consumeAsClient`
- **WHEN** its health check runs
- **THEN** it is the same TCP-listener probe as before the change

### Requirement: Client-consumer observability

Connect observability SHALL be owned solely by the retry helper (per-attempt
retry counters and a single exhaustion error per exhausted sequence); frame
outcomes SHALL be emitted through the lever-gated component-operations
facade, and the call site SHALL NOT add its own metrics for the connect
operation (ADR-0066 D6/D13).

#### Scenario: exhaustion error recorded once

- **GIVEN** a client-consumer whose reconnect policy allows three attempts against an unreachable remote
- **WHEN** the sequence exhausts
- **THEN** three retry attempts are recorded and exactly one connect-exhaustion error is recorded by the retry machinery, with no additional call-site connect metric

#### Scenario: frame outcomes via the facade

- **GIVEN** an active client-consumer delivering two frames
- **WHEN** both frames are submitted to the route
- **THEN** two `ws`/`frame` component-operation observations are recorded through the component-metrics facade, with the success series flowing only when the components metrics lever is enabled

### Requirement: Producer TLS enablement

Enabling the crate-wide tokio-tungstenite TLS feature SHALL make the
unchanged `wss://` producer able to connect over TLS, a side effect of the
client-consumer TLS path; `ws://` producer behavior SHALL be unaffected.

#### Scenario: producer wss connect

- **GIVEN** a local `wss://` server whose certificate chains to a test CA trusted by the test TLS configuration and a producer endpoint at that URL
- **WHEN** the producer sends a message
- **THEN** the TLS connection is established and the exchange completes with the server's response

### Requirement: Server mode default preserved

Without `consumeAsClient`, the `ws://`/`wss://` consumer SHALL behave
identically to the pre-change server-mode consumer.

#### Scenario: absent option keeps server behavior

- **GIVEN** a route whose `from:` endpoint is `ws://localhost:PORT/echo` with no `consumeAsClient` option
- **WHEN** the consumer starts
- **THEN** it binds a listening server and delivers exchanges from inbound connections exactly as before the change

### Requirement: Client-consumer option metadata

The `consumeAsClient` option SHALL be declared in the component's
`ComponentMetadata` URI options (derived from the `#[uri_param]` macro), so
URI linting accepts it without manual catalog edits, and its parsing SHALL
reject invalid values and default to `false` when absent.

#### Scenario: lint accepts the option

- **GIVEN** the production lint catalog built from component metadata
- **WHEN** a route URI uses `consumeAsClient=true`
- **THEN** URI-known linting raises no unknown-option finding

#### Scenario: parsing true, false, and default

- **GIVEN** URIs with `consumeAsClient=true`, `consumeAsClient=false`, and no `consumeAsClient` parameter
- **WHEN** the endpoint config is parsed from each URI
- **THEN** the parsed values are true, false, and false respectively, and `uri_options()` metadata lists the option

#### Scenario: invalid value rejected

- **GIVEN** a URI with `consumeAsClient=yes`
- **WHEN** the endpoint config is parsed
- **THEN** parsing returns an error instead of silently coercing

### Requirement: Consumer stop settle bound follows the configured drain budget

The system SHALL bound the `WsConsumer` stop settle wait by the configured
drain budget (`camel-config` `drain_timeout_ms`) when the stop is driven by a
graceful context shutdown, and SHALL preserve the existing 5 s bound for
reload and suspend stops. On settle expiry the consumer SHALL fail open
(warn + proceed with teardown) with the existing warn fields.

#### Scenario: Graceful stop honors a budget smaller than the old bound

- **GIVEN** a started `WsConsumer` with an in-flight exchange outstanding and
  a propagated drain budget of 500 ms
- **WHEN** `stop()` runs under a paused tokio clock and the budget expires
  before the exchange completes
- **THEN** the settle wait expires at ~the configured 500 ms bound (not 5 s),
  the consumer logs the fail-open warning and proceeds with teardown, and
  `stop()` returns successfully

#### Scenario: Graceful stop honors a budget larger than the old bound

- **GIVEN** a started `WsConsumer` with an in-flight exchange that completes
  6 s into the settle wait (simulated on a paused tokio clock) and a
  propagated drain budget of 10 s
- **WHEN** `stop()` runs
- **THEN** the settle wait does NOT expire at the old 5 s bound, the exchange
  completion is observed before expiry, and `stop()` returns successfully
  without the fail-open warning

#### Scenario: Reload stop keeps the default shutdown bound

- **GIVEN** a route stopped through the registry/reload stop path
- **WHEN** the `WsConsumer` settle wait runs
- **THEN** the settle bound is `DEFAULT_SHUTDOWN_TIMEOUT` (5 s), matching the
  pre-change behavior

#### Scenario: Suspend stop falls back when no budget was propagated

- **GIVEN** a consumer incarnation whose stop-budget slot was never written
- **WHEN** `stop()` runs
- **THEN** the settle bound is the documented fallback constant (5 s)

#### Scenario: Real-IO echo survives under a configured budget

- **GIVEN** a connected WebSocket client with an echo exchange in flight and
  a small configured drain budget propagated to the consumer
- **WHEN** stop is initiated while the echo exchange is still completing
  inside the budget
- **THEN** the echo response is delivered on the connection before teardown
  and the consumer stop completes cleanly

