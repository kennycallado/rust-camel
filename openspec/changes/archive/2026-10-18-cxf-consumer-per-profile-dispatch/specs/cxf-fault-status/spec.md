## MODIFIED Requirements

### Requirement: Consumer-stream availability is gated before security and body processing

The listener SHALL check consumer-stream availability for the addressed profile (via
`CxfServerManager.hasConsumerObserver(profile)`) BEFORE any WS-Security processing and BEFORE parsing
or classifying the SOAP body. When no consumer stream is connected for that profile, the listener
SHALL reject the request with HTTP 503 and a `Retry-After` header and a plaintext
body, and SHALL NOT emit a SOAP fault envelope, so this response does not enter
the fault-class matrix above. This gate SHALL be unconditional and
version-independent: a request on a `Signature`-required profile whose signature
is missing or invalid SHALL still take the pre-stream 503 rather than a security
fault, and the configured body byte/transport limits SHALL remain enforced. Once
the profile's consumer stream is connected, requests SHALL proceed through the normal
security/body path. The listener SHALL NOT defer this to a late
`CxfServerManager` check or map the case through a generic exception path to
HTTP 500. A stream that becomes unavailable after the availability check SHALL
yield the same HTTP 503 transport-availability response.

#### Scenario: Request before the consumer stream returns 503 without a fault

- **GIVEN** a configured listener whose addressed profile has no consumer stream connected
- **WHEN** an actual SOAP POST arrives
- **THEN** the response is HTTP 503 with `Retry-After` and no SOAP fault
  envelope, not HTTP 500

#### Scenario: Availability is checked before WS-Security

- **GIVEN** a `Signature`-required profile with no consumer stream connected and
  a request whose signature is missing or invalid
- **WHEN** an actual SOAP POST arrives
- **THEN** the response is HTTP 503 with `Retry-After` and a plaintext body, NOT
  a 400/500 security fault, and the configured body byte/transport limits were
  applied

#### Scenario: Late stream loss yields the same 503

- **GIVEN** a request that passed the availability check whose consumer stream
  becomes unavailable before dispatch
- **WHEN** the listener responds
- **THEN** the response is the same HTTP 503 transport-availability response
  with `Retry-After`, not a generic 500
