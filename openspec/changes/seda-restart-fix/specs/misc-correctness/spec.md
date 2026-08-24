# Delta Specification: seda-restart-fix

## MODIFIED Requirements

### Requirement: SEDA concurrent consumer delivery

The SEDA component SHALL spawn one forwarder task per `concurrentConsumers`
value when the mode is Single, using a shared
`tokio::sync::Mutex<Option<Receiver>>` so that forwarders process envelopes in
parallel. A forwarder that observes `None` in the shared receiver (the consumer
stopped and the stop path took the receiver back) SHALL exit successfully.

#### Scenario: concurrentConsumers=4 spawns four forwarders

- **GIVEN** a SEDA endpoint configured with `concurrentConsumers=4`
- **WHEN** the consumer is started
- **THEN** four forwarder tasks are spawned
- **AND** four JoinHandles are stored in `forwarder_handles`
- **AND** `concurrency_model()` reports `Concurrent { max: Some(4) }`

#### Scenario: InOut exchanges process in parallel

- **GIVEN** a SEDA endpoint with `concurrentConsumers=2` and an InOut consumer that sleeps 100ms per envelope
- **WHEN** two envelopes are enqueued simultaneously
- **THEN** both envelopes complete within 200ms (parallel), not 400ms (serial)

#### Scenario: concurrentConsumers=1 preserves single-forwarder behavior

- **GIVEN** a SEDA endpoint configured with `concurrentConsumers=1`
- **WHEN** the consumer is started
- **THEN** exactly one forwarder task is spawned

#### Scenario: Lock is not held during processing

- **GIVEN** a SEDA endpoint with `concurrentConsumers=2`
- **WHEN** two envelopes are enqueued and the consumer blocks on the first
- **THEN** the second forwarder acquires the receiver lock and processes the second envelope concurrently
