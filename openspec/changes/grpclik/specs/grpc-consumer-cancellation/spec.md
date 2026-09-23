# grpc-consumer-cancellation delta — grpclik

## ADDED Requirements

### Requirement: bidi response stream terminates on consumer shutdown

The gRPC consumer's dispatch registration SHALL carry a cancellation
token derived from `ConsumerContext::cancel_token()` (child token, no
fresh production root), the registration guard SHALL cancel it on
every teardown path, and the bidi forward task SHALL observe it: when
the token fires, the forward task emits a best-effort UNAVAILABLE
error item (non-blocking) and exits, so an accepted bidi response
stream terminates in bounded time even while the client request stream
stays open.

#### Scenario: graceful cancellation closes an accepted bidi stream

- **GIVEN** a gRPC bidi consumer with one accepted bidi call whose
  client keeps its request stream open (no further messages sent)
- **WHEN** the consumer's `ConsumerContext` cancellation token is
  cancelled
- **THEN** the consumer future completes, and the client's response
  stream reaches a terminal item (`Err` with code UNAVAILABLE, or
  stream end) within bounded time WITHOUT the client closing its
  request side, and all in-flight claims drain to zero

#### Scenario: forced abort closes an accepted bidi stream

- **GIVEN** a gRPC bidi consumer with one accepted bidi call whose
  client keeps its request stream open
- **WHEN** the consumer task is aborted via `JoinHandle::abort()`
- **THEN** the registration guard's drop cancels the entry token, and
  the client's response stream reaches a terminal item (any outcome)
  within bounded time without the client closing its request side

#### Scenario: shutdown closure is best-effort and non-blocking

- **GIVEN** a bidi response stream whose reply channel receiver is
  gone or full when the entry token fires
- **WHEN** the forward task handles the cancellation
- **THEN** it drops its senders without blocking on the channel (the
  UNAVAILABLE item is `try_send`-ed and ignored on failure), so
  consumer teardown is never delayed by stream closure

#### Scenario: closure is per-consumer and spares replacements

- **GIVEN** a consumer torn down with open bidi streams and a
  replacement consumer registered on the same path
- **WHEN** the predecessor's entry token is cancelled by its guard
- **THEN** only the predecessor's streams close; the replacement's
  bidi streams are governed solely by the replacement's own token
  (pinned by the existing rc-orr73 restart suites, which serve new
  bidi calls after same-path restart)
