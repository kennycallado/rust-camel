# cxf-soap-envelope Specification

## Purpose
TBD - created by archiving change 361-cxfsoap. Update Purpose after archive.
## Requirements
### Requirement: SOAP version derives from the selected WSDL port binding

The CXF bridge producer SHALL derive the SOAP version from the selected WSDL
port's binding (`dispatch.getBinding().getBindingID()`). It SHALL NOT read a
`soap-version` request header; that header is ignored. An unrecognized binding
ID SHALL fail the invocation explicitly; the bridge SHALL NOT silently default
to SOAP 1.1.

#### Scenario: SOAP 1.1 binding emits a SOAP 1.1 request

- **GIVEN** a WSDL port bound to SOAP 1.1 (`jakarta.xml.ws.soap.SOAPBinding.SOAP11HTTP_BINDING` = `http://schemas.xmlsoap.org/wsdl/soap/http`)
- **WHEN** the producer invokes an operation
- **THEN** the captured request uses the SOAP 1.1 envelope namespace, a
  `text/xml` content type, and the WSDL's `SOAPAction`

#### Scenario: SOAP 1.2 binding emits a SOAP 1.2 request

- **GIVEN** a WSDL port bound to SOAP 1.2 (`jakarta.xml.ws.soap.SOAPBinding.SOAP12HTTP_BINDING` = `http://www.w3.org/2003/05/soap/bindings/HTTP/`)
- **WHEN** the producer invokes an operation
- **THEN** the captured request uses the SOAP 1.2 envelope namespace and an
  `application/soap+xml` content type carrying the action

#### Scenario: Both SOAP versions complete a success roundtrip and capture the wire

- **GIVEN** a capturing mock SOAP service parameterized to reply with a version-
  matched response: SOAP 1.1 with `text/xml` and SOAP 1.2 with
  `application/soap+xml`
- **WHEN** the producer invokes over the matching WSDL binding (1.1 and 1.2)
- **THEN** the invocation succeeds on both bindings and the captured request
  carries the matching envelope namespace and content type

#### Scenario: soap-version header is ignored

- **GIVEN** a SOAP 1.1 binding and a request header `soap-version: 1.2`
- **WHEN** the producer invokes
- **THEN** the emitted envelope remains SOAP 1.1

#### Scenario: Unrecognized binding fails explicitly

- **GIVEN** a dispatch whose selected binding ID is neither SOAP 1.1 nor SOAP 1.2
  (supplied by a binding stub, since a real WSDL cannot advertise an unsupported
  binding ID)
- **WHEN** the producer invokes
- **THEN** the invocation fails naming the binding ID and no request is sent

### Requirement: PAYLOAD dispatch emits exactly one SOAP envelope

The bridge producer SHALL pass the operation payload to a
`Service.Mode.PAYLOAD` dispatch without manually wrapping it in a SOAP
envelope, so the wire request contains exactly one `Envelope` with exactly one
`Body`, and the operation element is a direct child of that `Body`.

#### Scenario: Wire request has a single envelope and a direct operation child

- **GIVEN** a producer invocation against a capturing mock SOAP server
- **WHEN** the request is captured
- **THEN** the body contains exactly one `Envelope` with exactly one `Body`,
  and the operation element is a direct child of `Body`

#### Scenario: A payload element is not nested in a duplicate envelope

- **GIVEN** a producer invocation whose payload is a bare operation element
- **WHEN** the captured request is parsed
- **THEN** there is no `Envelope` inside `Body` and no second `Body` element

### Requirement: A full SOAP Envelope payload is rejected before network

When the supplied payload's root element is already a SOAP `Envelope`, the
bridge producer SHALL reject the invocation before any network call and SHALL
NOT send the request.

#### Scenario: Pre-wrapped payload fails fast

- **GIVEN** a producer invocation whose payload root is
  `<soapenv:Envelope ...>`
- **WHEN** the bridge processes the invocation
- **THEN** it fails with an actionable error and no HTTP request is sent

### Requirement: Producer timeout is a reconciled nonnegative uint64

Both `cxf_bridge.proto` copies SHALL declare `uint64 timeout_ms = 7` on
`SoapRequest`. A value of `0` SHALL preserve the existing default-timeout
behavior (the configured connection timeout, currently 30 000 ms). A value
greater than `2147483647` (2^31−1) SHALL be rejected, which also rejects a
legacy negative `int32` representation decoded as an unsigned value. Rejection
SHALL occur before the dispatch is invoked.

#### Scenario: Zero preserves the existing default

- **GIVEN** a `SoapRequest` with `timeout_ms = 0`
- **WHEN** the bridge normalizes the timeout
- **THEN** it uses the configured default connection timeout

#### Scenario: Oversized or legacy-negative timeout is rejected

- **GIVEN** a `SoapRequest` with `timeout_ms = 2147483648` (or the unsigned
  representation of a legacy `-1`)
- **WHEN** the bridge validates the request
- **THEN** it rejects the invocation before dispatch

#### Scenario: In-range timeout is honored

- **GIVEN** a `SoapRequest` with `timeout_ms = 5000`
- **WHEN** the bridge validates the request
- **THEN** it applies 5000 ms

