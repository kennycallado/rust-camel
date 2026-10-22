# cxf-consumer-dispatch Specification

## Purpose
TBD - created by archiving change cxf-consumer-per-profile-dispatch. Update Purpose after archive.
## Requirements
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

### Requirement: Allow-listed SOAP headers are exported to the route

The consumer SHALL export each child of the SOAP `Header` whose QName appears in the
consuming profile's `security.exposed_soap_headers` allow-list. For each exported
element the bridge SHALL send the namespace URI, the local name, the serialized element
XML, and a `signed` flag. A profile without inbound WS-Security SHALL export the same
fields with `signed = false`. An element not in the allow-list SHALL be ignored
silently and SHALL NOT fault the request. `wsse:Security` SHALL never be exported.

A header SHALL be `signed = true` only when a verified `SIGN` result references the
exported post-WSS DOM node itself. A reference without a protected element SHALL NOT
set the flag, even when a `wsu:Id` matches: node identity is the only accepted
evidence. A `SIGN` result that covers only the Body SHALL leave the header
`signed = false`. References from `ENCR` results SHALL NOT set the flag. A failed
signature verification SHALL remain a WSS failure and SHALL NOT be downgraded to an
unsigned dispatch.

#### Scenario: Allow-listed header is exported

- **GIVEN** a profile whose allow-list contains `{http://tempuri.org/commonSchema}requestHeaderType`
- **WHEN** a signed request carrying that element next to `wsse:Security` is accepted
- **THEN** exactly one exported header carries that namespace, local name, and serialized XML

#### Scenario: Signed flag is true for a covered header

- **GIVEN** a profile with inbound signature verification and a header element referenced by the verified `SIGN`
- **WHEN** the request is accepted
- **THEN** the exported header has `signed = true`

#### Scenario: Uncovered header on a signed request is false

- **GIVEN** a profile with inbound signature verification whose signature covers only the SOAP Body
- **WHEN** the request also carries an allow-listed header that the signature does not reference
- **THEN** the header is exported with `signed = false`

#### Scenario: Id-only reference does not set signed

- **GIVEN** a profile with inbound signature verification and a `SIGN` result whose data reference has no protected element but whose `wsu:Id` equals the allow-listed element's own `wsu:Id`
- **WHEN** the request is accepted
- **THEN** the header is exported with `signed = false`

#### Scenario: Encrypted-only reference does not set signed

- **GIVEN** a profile with inbound verification where an `ENCR` result references the allow-listed element and no `SIGN` result references it
- **WHEN** the request is accepted
- **THEN** the header is exported with `signed = false`

#### Scenario: Signed flag is false without WS-Security

- **GIVEN** a profile without inbound WS-Security whose allow-list contains a header QName
- **WHEN** a request carrying that header is accepted
- **THEN** the exported header has `signed = false`

#### Scenario: Unlisted header is ignored

- **GIVEN** a profile whose allow-list contains one QName
- **WHEN** the request carries the listed element and a second, unlisted element
- **THEN** only the listed element is exported and the request reaches the route without a fault

#### Scenario: Several QNames per profile

- **GIVEN** a profile whose allow-list contains both partner QNames
- **WHEN** a request carries both elements
- **THEN** both headers are exported in one request

#### Scenario: No cross-profile leakage

- **GIVEN** two profiles, one allowing both partner QNames and one allowing only the first
- **WHEN** the same two-header request is processed by each profile
- **THEN** the first profile exports both headers and the second exports only the listed one

#### Scenario: Security block is never exported

- **GIVEN** a profile build whose allow-list names `{WSSE namespace}Security`
- **WHEN** the profile is built
- **THEN** startup fails naming the reserved namespace and no request can export `wsse:Security`

### Requirement: The allow-list is validated fail-closed at startup

Each `security.exposed_soap_headers` entry SHALL be a `{namespace}local` QName. The
namespace SHALL be non-empty and SHALL NOT contain `{`, `}`, `,`, or whitespace. The
local name SHALL match `[A-Za-z_][A-Za-z0-9._-]*`. A profile SHALL be rejected at
startup when an entry violates that grammar, uses a WSSE 1.0, WSSE 1.1, WSU, DS,
XENC, or SOAP envelope namespace, when the list exceeds 16 entries, when two entries
share a QName, or when two local names collide under ASCII case-insensitive
comparison. An absent or empty list SHALL export no SOAP headers. The comma-joined
env carrier SHALL round-trip the validated list exactly.

#### Scenario: Reserved namespace is rejected

- **GIVEN** a profile whose allow-list contains an element from the WSSE, WSU, DS, XENC, or SOAP envelope namespace
- **WHEN** the profile is built
- **THEN** startup fails with an error naming the reserved namespace

#### Scenario: Malformed and oversized allow-lists are rejected

- **GIVEN** an allow-list with a missing namespace, a missing local name, a local name outside the ASCII grammar, a delimiter-bearing namespace, or 17 entries
- **WHEN** the profile is built
- **THEN** startup fails with an error naming the offending entry or the cap

#### Scenario: Duplicate and case-colliding local names are rejected

- **GIVEN** an allow-list with two QNames that share a local name, or whose local names differ only by ASCII case, in different namespaces
- **WHEN** the profile is built
- **THEN** startup fails because the exchange key uses the local name and header lookup is case-insensitive

#### Scenario: Configuration flows through the env carrier

- **GIVEN** a route profile with a non-empty allow-list
- **WHEN** the bridge process environment is generated
- **THEN** `CXF_PROFILE_<NAME>_EXPOSED_SOAP_HEADERS` carries the comma-joined QNames and the Java profile holds a list that equals the configured list

### Requirement: Export is bounded and rejects ambiguous or oversized requests before dispatch

The bridge SHALL reject an accepted request with a Sender fault, before any route
dispatch, when two elements with the same allow-listed QName appear, when one exported
header exceeds 16,384 UTF-8 bytes of serialized XML, or when the exported headers
exceed 65,536 UTF-8 bytes in total. The limits are inclusive: a value of exactly
16,384 or 65,536 bytes is accepted. The measured value is the UTF-8 byte length of the
serialized element, not a character count. The fault SHALL NOT be classified as a
WS-Security fault and SHALL NOT be rewritten by the profile's security `fault_status`
override. Neither the fault message nor any log line SHALL contain a header value.

#### Scenario: Duplicate QName faults

- **GIVEN** a profile allowing one QName
- **WHEN** the request carries two elements with that QName
- **THEN** the bridge returns a Sender fault and the route is never invoked

#### Scenario: Single-header size cap faults

- **GIVEN** a profile allowing the request's header QName
- **WHEN** the serialized header exceeds 16,384 UTF-8 bytes, including a multibyte character count that would fit in characters
- **THEN** the bridge returns a Sender fault and the route is never invoked

#### Scenario: Aggregate size cap faults

- **GIVEN** a profile allowing several header QNames
- **WHEN** each header is below 16,384 bytes but their total exceeds 65,536 bytes
- **THEN** the bridge returns a Sender fault and the route is never invoked

#### Scenario: Exact limits are accepted

- **GIVEN** exported headers whose single and total byte lengths are exactly 16,384 and 65,536
- **WHEN** the request is processed
- **THEN** the headers are exported and the request reaches the route

### Requirement: SOAP headers travel on a dedicated wire field

The bridge-to-Rust contract SHALL carry exported SOAP headers in a dedicated repeated
`SoapHeader` field, additive to the existing message. The consumer SHALL NOT read any
SOAP header value from the HTTP header map, and SHALL map each exported header to
`CxfSoapHeader.<localName>` (serialized XML) and `CxfSoapHeaderSigned.<localName>`
(`"true"`/`"false"`) after the HTTP-header mapping.

#### Scenario: SOAP header reaches the exchange

- **GIVEN** a bridge that exports an allow-listed header
- **WHEN** the Rust consumer assembles the exchange
- **THEN** `CxfSoapHeader.<localName>` holds the XML and `CxfSoapHeaderSigned.<localName>` holds the flag

#### Scenario: HTTP spoof cannot shadow SOAP or Cxf headers

- **GIVEN** an incoming request whose HTTP headers include `cxfsoapheader.organisation`, `cxfoperation`, and `cxfsecurityprofile`
- **WHEN** the Rust consumer assembles the exchange
- **THEN** those HTTP keys are dropped, the bridge-owned `CxfOperation` and `CxfSecurityProfile` values win, and only the dedicated SOAP field can populate `CxfSoapHeader.*`

### Requirement: Application header reaches the route through a real signed call

A real signed request carrying an allow-listed application header next to
`wsse:Security` SHALL complete signature verification and deliver the header to the
route as `CxfSoapHeader.<localName>` together with its `CxfSoapHeaderSigned.<localName>`
flag.

#### Scenario: End-to-end signed request exposes the application header

- **GIVEN** a native bridge and a consumer profile that allow-lists the application header
- **WHEN** a signed SOAP request carrying that header is posted to the profile's endpoint
- **THEN** the route observes `CxfSoapHeader.<localName>` with the expected XML and `CxfSoapHeaderSigned.<localName>`

