## ADDED Requirements

### Requirement: Consumer streams are registered per profile

The bridge SHALL hold at most one consumer stream per security profile name. A consumer SHALL announce its profile when it opens `OpenConsumerStream`, as the gRPC call metadata value `x-cxf-profile`. The bridge SHALL close an open whose profile is missing or not configured with status `INVALID_ARGUMENT`. Opening a stream for one profile SHALL NOT replace, close or otherwise affect the stream or the pending requests of another profile.

#### Scenario: Two profiles keep two streams

- **GIVEN** consumer streams open for profiles `v11` and `v12`
- **WHEN** the second stream opens
- **THEN** the stream for `v11` remains registered and connected

#### Scenario: Open without a profile is rejected

- **GIVEN** a configured bridge with a stream registered for `v11`
- **WHEN** a consumer opens `OpenConsumerStream` without `x-cxf-profile`, with an empty value, with `V11` (profile names match exactly, case-sensitive), or with an unconfigured profile
- **THEN** the call ends with `INVALID_ARGUMENT`, no new stream is registered and the `v11` stream is untouched

#### Scenario: Consumer does not retry a rejected profile

- **GIVEN** a bridge that answers `OpenConsumerStream` with `INVALID_ARGUMENT`
- **WHEN** the Rust consumer opens its stream
- **THEN** it logs one error naming the profile, performs no reconnect attempt and its task returns `Ok(())`

#### Scenario: Rejected profile does not restart the route

- **GIVEN** a route supervised with restarts enabled and a bridge that rejects its profile with `INVALID_ARGUMENT`
- **WHEN** the consumer starts
- **THEN** the runtime records no consumer crash and performs no route restart

#### Scenario: Metadata reaches the handler over real gRPC

- **GIVEN** a `@QuarkusTest` bridge served by a real gRPC server (precedent: `OtelSpanFlowTest`, `ManagedChannel` plus `MetadataUtils`) and a client sending `x-cxf-profile: v11`
- **WHEN** the client opens `OpenConsumerStream`
- **THEN** the stream is registered under `v11`

### Requirement: Inbound requests dispatch to the addressed profile

The bridge SHALL deliver an inbound SOAP request only to the consumer stream registered for the profile named in the request path. It SHALL NOT fall back to the stream of another profile. When no stream is registered for that profile, the request SHALL fail as consumer-stream unavailable.

#### Scenario: Request goes to its own route

- **GIVEN** consumers connected for `v11` and `v12`
- **WHEN** a SOAP request arrives at `/cxf/v11`
- **THEN** only the `v11` stream receives it, and the `v12` stream receives nothing

#### Scenario: No cross-profile fallback

- **GIVEN** a consumer connected only for `v12`
- **WHEN** a SOAP request arrives at `/cxf/v11`
- **THEN** the request is not delivered to `v12` and the listener answers HTTP 503

### Requirement: Stream replacement and close are scoped to one profile

Replacing the stream of a profile SHALL leave the pending requests of the replaced stream owned by it, as before. Closing a stream SHALL clear the registration only if that stream still holds it, and SHALL fail only the pending requests that stream owns. Pending requests of other profiles SHALL be unaffected.

#### Scenario: Closing one profile does not fail another

- **GIVEN** pending requests on `v11` and on `v12`
- **WHEN** the `v11` stream closes
- **THEN** only the `v11` requests fail and the `v12` requests stay pending

#### Scenario: Reconnect of one profile keeps new requests

- **GIVEN** the `v11` stream was replaced by a new `v11` stream, with requests pending on the old one
- **WHEN** the old stream closes
- **THEN** `hasConsumerObserver("v11")` stays true, a request sent after the replacement goes to the new stream and stays pending, and the old stream's pending requests fail with `StreamUnavailableException`

### Requirement: A second consumer on one profile fails at route start

The Rust component SHALL refuse to start a second consumer route for a profile that already has an active consumer in the same pool. The error SHALL name the profile. The first consumer SHALL keep running.

#### Scenario: Duplicate profile

- **GIVEN** a running `cxf://` consumer route on profile `v11`
- **WHEN** a second consumer route on `v11` starts
- **THEN** it fails with a configuration error naming `v11`, and the first route keeps receiving requests

#### Scenario: Stopped route can start again

- **GIVEN** a consumer route on `v11` that was started and then stopped (or whose start failed, or whose task was aborted)
- **WHEN** a consumer route on `v11` starts
- **THEN** it starts successfully

#### Scenario: Immediate restart after stop does not race the old task

- **GIVEN** a running consumer route on `v11` whose task is aborted by the runtime without being awaited, followed by `stop()`
- **WHEN** a new consumer route on `v11` starts immediately afterwards
- **THEN** it starts successfully, and a late exit of the old task does not release the new registration
