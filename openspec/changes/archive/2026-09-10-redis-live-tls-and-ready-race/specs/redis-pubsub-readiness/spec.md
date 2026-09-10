## ADDED Requirements

### Requirement: PubSub consumer readiness follows server-side SUBSCRIBE registration

The camel-redis PubSub consumer SHALL signal consumer readiness
(`mark_ready`) only after the pubsub session has completed its initial
connect and the server has acknowledged every SUBSCRIBE and PSUBSCRIBE
command for the configured channels and patterns. Readiness SHALL
remain pending while any acknowledgement is outstanding. Reconnect
re-subscriptions SHALL NOT re-signal readiness.

#### Scenario: Readiness stays pending until all subscribe acks complete

- **GIVEN** a pubsub consumer session driven against a fake I/O seam
  whose subscribe-ack replies are blocked (never delivered)
- **WHEN** the session has connected and sent its SUBSCRIBE commands
- **THEN** the ready signal has not fired, and it fires only after the
  blocked acks are released and every channel and pattern
  acknowledgement completes

#### Scenario: Ready after subscribe-ack delivers immediate publishes

- **GIVEN** a Redis broker and a route with a `redis://` SUBSCRIBE
  consumer
- **WHEN** a producer PUBLISHes to the channel immediately after
  `harness.start()` returns, with no readiness barrier and no retry
- **THEN** the consumer receives the message

#### Scenario: Immediate-publish delivery holds under a 20-trial stress battery

- **GIVEN** the camel-test redis pubsub-consumer integration test with
  no readiness barrier, no sleep, and no retry in the test body
- **WHEN** the test is executed as 20 independent invocations, each on
  a freshly named channel, via
  `cargo test -p camel-test --test redis_test --features integration-tests redis_consumer_pubsub_mode -- --exact`
  run 20 times
- **THEN** every invocation passes (20/20 green)

#### Scenario: Readiness ordering is deterministic without a broker

- **GIVEN** a pubsub consumer session driven against a fake I/O seam
  that records the ordering of subscribe and ready events
- **WHEN** the session runs to first message delivery
- **THEN** every subscribe acknowledgement is observed before the
  ready signal, and the ready signal is observed exactly once across a
  simulated reconnect with re-subscribe

#### Scenario: Unreachable Redis does not hang startup

- **GIVEN** a route with a `redis://` SUBSCRIBE consumer pointed at an
  unreachable broker, configured with a short reconnect policy, and a
  test-side outer timeout shorter than the consumer startup budget
- **WHEN** `harness.start()` is called (which reports failure by
  panicking through its `expect`, harness.rs:271-273)
- **THEN** startup fails within the outer timeout — the panic is
  observed, not a hang — and, asserted separately at unit level, the
  reconnect-exhaustion error classifies as a retryable network failure
  per the component's retry taxonomy (ADR-0007), not as a terminal
  `Config` error

### Requirement: PubSub end-to-end delivery proof in camel-test

The camel-test Redis integration suite SHALL prove PUBLISH→SUBSCRIBE
delivery end-to-end: the pubsub-consumer test SHALL NOT gate its
publish on a server-side `PUBSUB CHANNELS` readiness barrier (the
rc-8kha test-side mask), and the pubsub-producer test SHALL assert the
published message is received by a real subscriber.

#### Scenario: Pubsub consumer test publishes without a barrier

- **GIVEN** the camel-test redis pubsub-consumer integration test with
  the component-side readiness fix in place
- **WHEN** the test publishes immediately after `harness.start()`
- **THEN** no `PUBSUB CHANNELS` poll barrier exists in the test and the
  message is delivered

#### Scenario: Pubsub producer test asserts subscriber receipt

- **GIVEN** the camel-test redis pubsub-producer integration test
- **WHEN** a route publishes a message through the PUBLISH producer
- **THEN** an actual SUBSCRIBE subscriber on the channel receives the
  published payload (not merely route completion)
