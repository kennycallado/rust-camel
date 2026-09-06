# Delta: integration-tier

## ADDED Requirements

### Requirement: Wire-fidelity lane key and mismatch diagnostics

Partner arrival lanes MUST key on the strict wire `path_and_query` bytes of each recorded request; receive paths derive from the declared partner endpoint URI. The key MUST stay strict (never canonicalized) so producer-side byte drift remains detectable. When matching fails, the harness MUST report the wire evidence: receive-timeout errors list the wire paths that arrived in the lane group, and partner-count mismatches list the recorded paths. Harness HTTP targets with an empty or absent path MUST fail as apparatus errors, never silently defaulting to `/`.

#### Scenario: Query-bearing receive matches after wire fidelity

- **Given** a scenario declaring an HTTP partner `from: http://0.0.0.0:PORT/api?flag=a&x=1` and a route sending to a matching target with the same authored query bytes
- **When** the partner `receive` action runs
- **Then** the request is recorded into the matching lane and the receive asserts against it (arrival key equals the declared endpoint's wire `path_and_query`)

#### Scenario: Receive-timeout lists arrived wire paths

- **Given** a partner `receive` that matches no arrival lane
- **When** the receive times out
- **Then** the error message lists every wire path (with query) that arrived in the lane group, so byte divergence is diagnosed without external packet capture

#### Scenario: Count mismatch lists recorded paths

- **Given** a partner `receive` whose count assertion fails (expected N, recorded M ≠ N)
- **When** the mismatch is reported
- **Then** the failure detail prints the recorded request paths, not only the counts

#### Scenario: Empty harness path is an apparatus error

- **Given** a harness HTTP target declaration whose path is empty or absent
- **When** the harness parses the target
- **Then** parsing fails with an apparatus-class error naming the declaration — no silent `/` fallback lane

#### Scenario: Diagnostics redact sensitive query values

- **Given** a receive-timeout or count-mismatch diagnostic whose recorded wire path contains a query value classified sensitive under ADR-0051
- **When** the harness renders the diagnostic message
- **Then** the sensitive value is redacted in the printed path — wire-path visibility never becomes credential disclosure
