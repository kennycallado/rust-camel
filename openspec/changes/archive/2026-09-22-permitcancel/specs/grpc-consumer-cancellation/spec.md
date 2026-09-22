## ADDED Requirements

### Requirement: permit wait observes consumer cancellation

While the dispatcher semaphore is saturated, the gRPC consumer SHALL
race permit acquisition against `ConsumerContext` cancellation, and
SHALL stop accepting new work when cancellation wins.

#### Scenario: graceful stop during saturated permit wait

- **GIVEN** a gRPC consumer with `consumerConcurrency=1` and one
  long-lived stream holding the only permit
- **WHEN** a second request is dequeued and waits for a permit, and the
  consumer's cancellation token is cancelled
- **THEN** the consumer future completes without external abort within
  the test timeout, and the dispatch table no longer contains the path

#### Scenario: waiting client receives UNAVAILABLE on shutdown

- **GIVEN** a request envelope dequeued by the consumer and blocked on
  permit acquisition
- **WHEN** cancellation is observed before a permit is granted
- **THEN** unary and client-streaming reply channels receive gRPC
  UNAVAILABLE when their receivers remain open, and server-streaming
  and bidi reply channels are sent a best-effort UNAVAILABLE item
  without delaying shutdown if their channels are full or closed

#### Scenario: buffered envelopes retain channel-closure behavior

- **GIVEN** cancellation wins while additional envelopes remain
  buffered in `env_rx`
- **WHEN** the consumer receiver is dropped
- **THEN** those non-dequeued envelopes follow existing reply-channel
  closure behavior and are not guaranteed an explicit UNAVAILABLE
  reply

### Requirement: dispatch registration cleanup on every exit path

The gRPC consumer's dispatch-table registration SHALL be removed on
every exit path from `start_inner` after the table insert when a
runtime is alive to execute the cleanup. When no runtime remains, the
stale entry SHALL be replaceable: a registration whose envelope sender
is closed SHALL be atomically replaced by a new registration for the
same path instead of failing as a duplicate.

#### Scenario: forced abort mid permit wait

- **GIVEN** a consumer blocked on permit acquisition with its path
  present in the dispatch table
- **WHEN** the consumer task is aborted via `JoinHandle::abort()`
- **THEN** after abort completion, the stale registration is removed
  asynchronously or treated as replaceable because its sender is
  closed; same-path registration succeeds, and delayed cleanup cannot
  remove the replacement entry

#### Scenario: restart replaces a stale closed registration

- **GIVEN** an aborted consumer left a stale dispatch entry whose
  envelope sender is closed
- **WHEN** a new consumer registers the same path
- **THEN** the insert atomically replaces the stale entry, startup
  succeeds without a duplicate-path error, and the new consumer
  serves requests

#### Scenario: restart after saturated stop re-registers the path

- **GIVEN** a consumer that stopped while a second request waited on a
  saturated semaphore
- **WHEN** a new consumer starts on the same host, port, and path
- **THEN** startup succeeds without a duplicate-path error and the new
  consumer serves requests
