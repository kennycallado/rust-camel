## MODIFIED Requirements

### Requirement: Fault class and HTTP status follow a fixed matrix

<!-- openspec-scenario-ops
renamed: SOAP 1.1 always returns 500 -> SOAP 1.1 always returns 500 for faults
-->

The listener SHALL classify every failure as `Sender` or `Receiver` and map it
to HTTP status as follows:

- SOAP 1.1: any fault returns HTTP 500.
- SOAP 1.2: `Sender` returns HTTP 400; `Receiver` returns HTTP 500.
- Request-reply success returns HTTP 200 with SOAP content.
- Resolved input-only success returns HTTP 202 with an empty body after a correlated non-fault response, including successful Rust response marshalling.
- A fault SHALL NOT return 200 or 202.

Classification SHALL be: security/WSS failure, malformed envelope, missing SOAP
`Body`, or invalid/ambiguous operation selection returns `Sender`;
signer-descriptor validation failure, route processing error, response-marshalling failure, or timeout returns
`Receiver`. A Rust response code `soap:Client` SHALL map to `Sender` and
`soap:Server` to `Receiver`; any other SOAP 1.2 fault code SHALL return HTTP 500.
Signer-descriptor validation failure is not a WSS fault. Transport stream
unavailability SHALL retain its existing non-SOAP HTTP 503 response.

#### Scenario: Security failure is a Sender fault

- **GIVEN** a SOAP 1.2 request whose WSS verification fails
- **WHEN** the listener responds
- **THEN** the status is 400 and the fault code is a Sender code

#### Scenario: Malformed envelope is a Sender fault

- **GIVEN** a malformed SOAP 1.2 envelope
- **WHEN** the listener responds
- **THEN** the status is 400

#### Scenario: Missing Body is a Sender fault

- **GIVEN** a SOAP 1.2 request without a `Body` element
- **WHEN** the listener responds
- **THEN** the status is 400

#### Scenario: Invalid signer descriptor is a Receiver fault

- **GIVEN** invalid exported signer metadata on a profile with `fault_status`
- **WHEN** the listener validates the descriptor
- **THEN** the status is 500, the override does not apply, and no route dispatch occurs

#### Scenario: Route error and timeout are Receiver faults

- **GIVEN** a SOAP 1.2 request whose route fails or times out
- **WHEN** the listener responds
- **THEN** the status is 500

#### Scenario: Rust soap:Client maps to Sender and soap:Server to Receiver

- **GIVEN** a Rust consumer response with `soap:Client` or `soap:Server`
- **WHEN** the listener renders the fault
- **THEN** it maps to Sender or Receiver respectively

#### Scenario: SOAP 1.1 always returns 500 for faults

- **GIVEN** a SOAP 1.1 request of any fault class
- **WHEN** the listener returns a SOAP fault
- **THEN** the status is 500, including for input-only operations

#### Scenario: Input-only success returns empty 202

- **GIVEN** a resolved input-only request and a correlated non-fault response after successful Rust response marshalling
- **WHEN** the listener acknowledges completion
- **THEN** the status is 202 with no response body

#### Scenario: Successful route with unsupported output returns Receiver fault

- **GIVEN** an input-only route that completes successfully with `Body::Stream`
- **WHEN** Rust response marshalling fails
- **THEN** the listener returns Receiver HTTP 500, not 202
