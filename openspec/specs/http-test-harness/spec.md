# http-test-harness Specification

## Purpose
Readiness helpers for HTTP consumer tests. Readiness is polled from
`ServerRegistry::bound_addr` under a bounded budget and retry backoff, and
setup serializes registry mutations with `REGISTRY_TEST_MUTEX`.
## Requirements
### Requirement: Registry-poll readiness with bounded budget

The consumer-test readiness helper SHALL determine readiness by polling
`ServerRegistry::bound_addr` for the staged port — never by TCP probes —
with a fail-loud wall-clock deadline of 10 seconds and a retry backoff that
doubles from 1 ms, capped at 64 ms. A deadline miss SHALL panic with a
message naming the port and a cause hint naming the two known classes
(concurrent registry reset, scheduler starvation).

#### Scenario: staged-consumer-becomes-ready

- **GIVEN** a listener bound to `127.0.0.1:0` is staged and a consumer
  `start()` is spawned
- **WHEN** the readiness helper polls the registry
- **THEN** the poll observes the bound entry and returns before the
  deadline, without sending any TCP probe

#### Scenario: deadline-miss-is-loud-and-diagnosable

- **GIVEN** no registry entry ever appears for the staged port
- **WHEN** the poll loop runs past the 10-second deadline
- **THEN** the helper panics with a message that names the port and hints
  "registry entry absent (concurrent reset or starvation)"

### Requirement: Registry-mutation serialization during setup

The consumer-test readiness helper SHALL hold `REGISTRY_TEST_MUTEX` from
`stage_listener` until readiness-complete (including the tail-yield loop),
so a test-concurrent `ServerRegistry::reset()` that itself follows the
mutex law cannot clear the staged listener or the spawned entry inside the
setup window. The guard SHALL be released before the helper returns.

#### Scenario: concurrent-legal-reset-survival

- **GIVEN** a hammer thread loops legal resets — try-lock the
  `REGISTRY_TEST_MUTEX` (counting a contention when it blocks), then
  `ServerRegistry::reset()` — under a stop flag
- **WHEN** the readiness helper performs setups on fresh ephemeral ports,
  always at least 25, continuing past 25 only until one contended reset is
  observed, with a hard cap of 50 total setups
- **THEN** the test asserts at least one contended reset occurred, every
  setup that ran reports ready within its deadline with no readiness
  panic, and the hammer thread is stopped and joined by a Drop guard even
  if the test panics

<!-- Amendment 2026-09-14 (conductor, r_glm task-1.3-review minor): the
deadline scenario's quoted hint string was reconciled to the parenthetical
form actually implemented and specified verbatim in blessed tasks.md
("registry entry absent (concurrent reset or starvation)"). Quote-only
reconciliation; requirement semantics unchanged. -->

### Requirement: Shared HTTP/1.1 test harness with mandated connection close

The `camel_component_api::test_support::http` module (gated behind the existing
`test-support` feature) SHALL provide a shared test-harness listener that speaks
HTTP/1.1 correctly: it parses the request line and headers, reads the request body
according to `Content-Length`, and drains the full request before writing the
response. Every terminal response SHALL carry `Connection: close` unless the test
explicitly opted into keep-alive. The harness SHALL NOT introduce hyper or any new
workspace dependency into `camel-component-api`.

#### Scenario: single-request roundtrip closes the connection

- **GIVEN** a harness server started with a fixed `(200 OK, body)` response
- **WHEN** a client sends one well-formed GET and reads the response to EOF
- **THEN** the response is well-formed with a correct `Content-Length`, carries
  `Connection: close`, and the server closes the connection after the response

#### Scenario: request is drained before reply

- **GIVEN** a harness server and a client that sends a POST with a `Content-Length`
  body and immediately waits for the response
- **WHEN** the server answers
- **THEN** the server read the full request body before writing the response bytes,
  so the client never observes a TCP reset from unread inbound data

#### Scenario: segmented body is drained before reply

- **GIVEN** a client that writes the request head first and the body in a second
  write after an observable readiness signal (no sleeps)
- **WHEN** the server answers
- **THEN** the response arrives only after the second write, and the body the server
  recorded is the full concatenated body

#### Scenario: truncated body gets a deterministic 400

- **GIVEN** a client that declares `Content-Length: 10`, sends 3 bytes, then shuts
  down its write half while retaining the read half
- **WHEN** the exchange completes
- **THEN** the harness answers 400 with `Connection: close` and closes the socket,
  with no hang path; if a peer has fully disconnected, error-response delivery is
  best-effort and the harness still closes without hanging

#### Scenario: malformed head gets a deterministic 400

- **GIVEN** a client that sends bytes that do not parse as a request line and header
  block
- **WHEN** the exchange completes
- **THEN** the harness answers 400 with `Connection: close` and closes the socket

#### Scenario: unsupported transfer framing gets a deterministic 400

- **GIVEN** a client that sends a head declaring a `Transfer-Encoding` the harness
  does not implement
- **WHEN** the exchange completes
- **THEN** the harness answers 400 with `Connection: close` and closes the socket

#### Scenario: keep-alive option and close are explicit

- **GIVEN** a harness server configured with `.keep_alive(reason)`
- **WHEN** the client sends two requests on one connection
- **THEN** both are answered in order on that connection, and the reason string is
  visible at the configuration site in source

### Requirement: Harness teardown cancels its tasks

Dropping the harness handle SHALL cancel the listener task and all active connection
tasks WITHOUT blocking the calling thread (Drop-detach: abort via a cancellation
token; a blocking join cannot make progress on a current-thread executor). For tests
that must confirm reclamation, the handle SHALL expose an explicit bounded async
`shutdown` that aborts an incomplete request or an idle keep-alive connection rather
than awaiting it. Teardown SHALL behave identically on current-thread and
multi-thread runtimes.

#### Scenario: drop with an incomplete request in flight

- **GIVEN** a client that connected and sent half a request head, then stalls
- **WHEN** the test drops the harness handle without awaiting shutdown
- **THEN** the drop returns promptly (non-blocking), the listener socket is released
  once the aborted tasks are polled, and no task continues serving

#### Scenario: async shutdown confirms reclamation

- **GIVEN** a server with an idle keep-alive connection
- **WHEN** the test awaits the bounded `shutdown`
- **THEN** the connection task is cancelled, the join completes within the bounded
  budget, and the listener socket is released

#### Scenario: teardown on a current-thread runtime

- **GIVEN** the harness server and its test driving a `#[tokio::test]`
  current-thread runtime
- **WHEN** the test drops the handle (and awaits `shutdown` where used)
- **THEN** teardown completes without deadlock, because no teardown step blocks the
  executor

### Requirement: Harness staged and chunked response modes

Beyond `Content-Length`-framed responses, the harness SHALL support a chunked mode
emitting legal `Transfer-Encoding: chunked` framing (no Content-Length) and a staged
mode that writes the header block immediately followed by ordered chunk
continuations signaled through channels — never sleeps — preserving the
immediate-headers-then-delayed-chunks behavior that migrated sites rely on.

#### Scenario: staged headers then delayed chunks

- **GIVEN** a staged response configured to send headers immediately and two chunks
  whose continuations are signaled by a channel
- **WHEN** the client reads the response incrementally under its existing timeout
  behavior
- **THEN** the header block arrives before the first chunk signal fires, the chunks
  arrive in order as each signal fires, and the final chunk terminates the message

#### Scenario: chunked framing is legal

- **GIVEN** a chunked-mode response
- **WHEN** the client reads the response to EOF
- **THEN** the body assembles from valid chunk-size lines and the message carries
  `Transfer-Encoding: chunked` with no `Content-Length`

### Requirement: Harness request capture and routing

The harness SHALL record each request it accepts and expose BOTH layers: parsed
fields (method, header names/values, body) AND the exact wire head — the raw request
line and head bytes with the unnormalized request-target including query bytes — so
existing raw-bytes assertions survive migration unchanged. The harness SHALL support
both a fixed responder and a path-routed responder, and tests SHALL be able to
assert on recorded requests after the exchange.

#### Scenario: path routing answers differently per path

- **GIVEN** a harness server with a path-routed responder where `/fail` maps to 500
  and other paths map to 200
- **WHEN** the client requests `/fail` and then `/ok`
- **THEN** the responses are 500 then 200, and both requests appear in the recording

#### Scenario: recorded request head is assertable

- **GIVEN** a harness server with request recording enabled
- **WHEN** a client sends a request with specific headers and body
- **THEN** the recorded entry exposes the exact method, path, header names/values,
  and body bytes for assertion

#### Scenario: wire head and unnormalized target are preserved

- **GIVEN** a client that sends a request line with an unnormalized target
  containing query bytes (for example `/x?a=1&b=2`)
- **WHEN** the exchange completes
- **THEN** the recorded entry exposes the exact wire request-line/head bytes and the
  unnormalized target string, byte-identical to what the client sent

### Requirement: Harness TLS path via serve_connection

The harness SHALL expose a public `serve_connection` function generic over
`AsyncRead + AsyncWrite` so a TLS test can accept its own TCP connection, wrap it in
its own TLS acceptor, and hand the resulting stream to the harness — without the
harness crate depending on rustls.

#### Scenario: TLS-wrapped stream served by the harness

- **GIVEN** a test that owns a TLS acceptor and calls `serve_connection` with the
  accepted TLS stream
- **WHEN** a TLS client sends a request
- **THEN** the harness parses the request and answers through the TLS stream with
  the same close-mandated response framing as plain TCP

