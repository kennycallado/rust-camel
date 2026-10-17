# cxf-fault-status Specification

## Purpose
SOAP fault classification and HTTP status on the CXF listener. The listener
detects the SOAP version before WS-Security processing, maps fault class to a
fixed status matrix, emits version-matched fault envelopes, and applies
per-profile status overrides to security faults.

## Requirements
### Requirement: SOAP version is detected before WS-Security processing

The listener SHALL determine the request SOAP version, before any WS-Security
processing, from the envelope root namespace; if the root is unavailable, it
SHALL fall back to the content type: `application/soap+xml` selects SOAP 1.2,
while `text/xml` or an absent content type selects SOAP 1.1. Only when neither
the root nor the content type is usable SHALL the listener default to SOAP 1.1
for rendering the response. Version selection SHALL use only the envelope
namespace and the content-type fallback; there SHALL be no separate version
override.

#### Scenario: SOAP 1.2 root detected before WSS

- **GIVEN** a request whose envelope root uses the SOAP 1.2 namespace
- **WHEN** the listener processes it
- **THEN** the detected version is 1.2 before WSS runs

#### Scenario: SOAP 1.1 root detected before WSS

- **GIVEN** a request whose envelope root uses the SOAP 1.1 namespace
- **WHEN** the listener processes it
- **THEN** the detected version is 1.1 before WSS runs

#### Scenario: application/soap+xml without a readable root selects 1.2

- **GIVEN** a request with content type `application/soap+xml` whose root cannot
  be parsed
- **WHEN** the listener responds
- **THEN** the response envelope uses SOAP 1.2

#### Scenario: text/xml or no content type without a readable root defaults to 1.1

- **GIVEN** a malformed request with content type `text/xml` or no content type
- **WHEN** the listener responds
- **THEN** the response envelope uses SOAP 1.1

### Requirement: Fault class and HTTP status follow a fixed matrix

The listener SHALL classify every failure as `Sender` or `Receiver` and map it
to HTTP status as follows:

- SOAP 1.1: any fault → HTTP 500.
- SOAP 1.2: `Sender` → HTTP 400; `Receiver` → HTTP 500.
- Success → HTTP 200; a fault SHALL NOT return 200.

Classification SHALL be: security/WSS failure → `Sender`; malformed envelope →
`Sender`; a request with no SOAP `Body` → `Sender`; signer-descriptor validation
failure → `Receiver` (not a WSS fault); route processing error → `Receiver`;
timeout →
`Receiver`; a Rust response code `soap:Client` → `Sender`; a Rust response code
`soap:Server` → `Receiver`; any other SOAP 1.2 fault code → HTTP 500.

#### Scenario: Security failure is a Sender fault

- **GIVEN** a SOAP 1.2 request whose WSS verification fails
- **WHEN** the listener responds
- **THEN** the status is 400 and the fault code is a Sender code

#### Scenario: Malformed envelope is a Sender fault

- **GIVEN** a SOAP 1.2 request with a malformed envelope
- **WHEN** the listener responds
- **THEN** the status is 400

#### Scenario: Missing Body is a Sender fault

- **GIVEN** a SOAP 1.2 request with no `Body` element
- **WHEN** the listener responds
- **THEN** the status is 400

#### Scenario: Invalid signer descriptor is a Receiver fault

- **GIVEN** a SOAP 1.2 request whose exported signer descriptor fails validation
  on a profile that sets `fault_status`
- **WHEN** the listener responds
- **THEN** the status is 500 (the override does not apply, the descriptor failure
  is not a WSS fault), and the request was not dispatched to the route

#### Scenario: Route error and timeout are Receiver faults

- **GIVEN** a SOAP 1.2 request whose route processing failed or timed out
- **WHEN** the listener responds
- **THEN** the status is 500

#### Scenario: Rust soap:Client maps to Sender and soap:Server to Receiver

- **GIVEN** a Rust consumer response with `fault_code` `soap:Client` or
  `soap:Server`
- **WHEN** the listener renders the fault
- **THEN** `soap:Client` yields a Sender result and `soap:Server` a Receiver
  result

#### Scenario: SOAP 1.1 always returns 500

- **GIVEN** a SOAP 1.1 request of any fault class
- **WHEN** the listener responds
- **THEN** the status is 500

### Requirement: Fault envelopes are version-matched and namespace-correct

The fault builder SHALL emit an envelope whose namespace matches the detected
version, with QName prefixes bound to declared namespaces. A SOAP 1.1 fault
SHALL use `faultcode`/`faultstring` with a declared prefix (no undeclared
`soap:` prefix). A SOAP 1.2 fault SHALL use `Code`/`Value` and `Reason`/`Text`
with an `xml:lang` attribute.

#### Scenario: SOAP 1.1 fault prefix is declared

- **GIVEN** a SOAP 1.1 fault
- **WHEN** the fault XML is parsed
- **THEN** the SOAP prefix used by the fault code resolves to the SOAP 1.1
  namespace

#### Scenario: SOAP 1.2 fault uses the 1.2 structure

- **GIVEN** a SOAP 1.2 fault
- **WHEN** the fault XML is parsed
- **THEN** it contains `Code/Value` and `Reason/Text` with an `xml:lang`
  attribute and no `faultcode` element

### Requirement: Per-profile fault-status override applies to security faults only

`[profiles.security] fault_status` SHALL override the default HTTP status **for
security (WSS) faults only**, on **both SOAP versions**, on that profile,
preserving the ESAP security-fault requirement. It SHALL be an integer in the
range 400–599, validated at startup; values outside that range (including 1xx,
2xx, and 3xx) SHALL be rejected. All non-WSS fault classes SHALL keep the
default matrix. A signer-descriptor validation failure is a Receiver fault and
is NOT a WSS fault, so the override SHALL NOT apply to it; its status is 500.
The override SHALL NOT affect other profiles; there SHALL be no global
override.

#### Scenario: Override applies to its profile's WSS faults on SOAP 1.2

- **GIVEN** profile `esap` with `fault_status = 500` and profile `cyl` with
  defaults
- **WHEN** an equivalent SOAP 1.2 WSS fault occurs on each
- **THEN** `esap` returns 500 and `cyl` returns the default 400

#### Scenario: Override applies to its profile's WSS faults on SOAP 1.1

- **GIVEN** profile `esap` with `fault_status = 400`
- **WHEN** a SOAP 1.1 WSS fault occurs
- **THEN** the override applies and the status is 400

#### Scenario: Override does not affect non-security Receiver faults

- **GIVEN** a profile with `fault_status = 403` and a route (`Receiver`) fault
- **WHEN** the listener responds
- **THEN** the default matrix applies and the status is 500

#### Scenario: Override does not affect non-WSS Sender faults

- **GIVEN** a profile with `fault_status = 403` and a non-WSS SOAP 1.2 Sender
  fault (for example a malformed envelope)
- **WHEN** the listener responds
- **THEN** the default matrix applies and the status is 400

#### Scenario: Out-of-range override is rejected at startup

- **GIVEN** a profile with `fault_status = 200` (or `399`, or `600`)
- **WHEN** the pool config is validated at startup
- **THEN** startup fails with an actionable error

### Requirement: Consumer-stream availability is gated before security and body processing

The listener SHALL check consumer-stream availability (via the publisher's
`hasConsumerObserver` check) BEFORE any WS-Security processing and BEFORE parsing
or classifying the SOAP body. When no consumer stream is connected, the listener
SHALL reject the request with HTTP 503 and a `Retry-After` header and a plaintext
body, and SHALL NOT emit a SOAP fault envelope, so this response does not enter
the fault-class matrix above. This gate SHALL be unconditional and
version-independent: a request on a `Signature`-required profile whose signature
is missing or invalid SHALL still take the pre-stream 503 rather than a security
fault, and the configured body byte/transport limits SHALL remain enforced. Once
the consumer stream is connected, requests SHALL proceed through the normal
security/body path. The listener SHALL NOT defer this to a late
`CxfServerManager` check or map the case through a generic exception path to
HTTP 500. A stream that becomes unavailable after the availability check SHALL
yield the same HTTP 503 transport-availability response.

#### Scenario: Request before the consumer stream returns 503 without a fault

- **GIVEN** a configured listener whose consumer stream is not yet connected
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

