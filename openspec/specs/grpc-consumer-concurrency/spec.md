# grpc-consumer-concurrency Specification

## Purpose
TBD - created by archiving change concpanic. Update Purpose after archive.
## Requirements
### Requirement: gRPC consumer concurrency upper bound

The system SHALL reject `consumerConcurrency` values above
`tokio::sync::Semaphore::MAX_PERMITS` with a typed configuration error
that names the parameter, the configured value, and the limit. For
`create_consumer` and consumer startup the rejection SHALL occur
before shared-server registry mutation, consumer envelope-channel
construction, or dispatcher-semaphore construction; for `start()` it
SHALL additionally occur before listener binding and readiness
signaling (`start_with_listener()` receives an already-bound listener
from its caller, so listener binding precedes any validation it can
perform). The system SHALL NOT panic during route startup for any
representable `consumerConcurrency` value. The value `0` SHALL
continue to normalize to `1`.

#### Scenario: oversized value rejected at URI parse

- **GIVEN** a gRPC consumer URI with `consumerConcurrency` greater than `Semaphore::MAX_PERMITS`
- **WHEN** the URI is parsed
- **THEN** parsing fails with a typed configuration error naming `consumerConcurrency`, the configured value, and the limit

#### Scenario: oversized value rejected at consumer creation even when constructed directly

- **GIVEN** a `GrpcEndpoint` constructed directly (not via URI parse) whose config carries `consumerConcurrency` greater than `Semaphore::MAX_PERMITS`
- **WHEN** `create_consumer` is invoked
- **THEN** it returns a typed configuration error naming the parameter, the configured value, and the limit, and no panic occurs

#### Scenario: oversized value rejected at consumer startup before side effects

- **GIVEN** a `GrpcConsumer` constructed directly (not via `create_consumer`) with `consumer_concurrency` greater than `Semaphore::MAX_PERMITS`
- **WHEN** either startup entry point (`start` or `start_with_listener`) is invoked
- **THEN** it returns a typed configuration error before any shared-server registry mutation, envelope-channel construction, or semaphore construction, and no panic occurs; for `start` the error also precedes listener binding and readiness signaling

#### Scenario: boundary values

- **GIVEN** the limit `L = tokio::sync::Semaphore::MAX_PERMITS`
- **WHEN** concurrency validation runs with `L-1`, `L`, and `L+1`
- **THEN** `L-1` and `L` are accepted unchanged and `L+1` is rejected with a typed configuration error

#### Scenario: zero normalization retained

- **GIVEN** a gRPC consumer URI with `consumerConcurrency=0`
- **WHEN** the URI is parsed
- **THEN** the stored consumer concurrency is `1` (rc-ey6v behavior unchanged)

#### Scenario: accepted boundary is constructible in Tokio primitives

- **GIVEN** the limit `L = tokio::sync::Semaphore::MAX_PERMITS`
- **WHEN** a semaphore of `L` permits and a bounded channel of capacity `L` are constructed
- **THEN** both constructions succeed without panicking (the accepted upper bound is exactly the primitives' bound)

