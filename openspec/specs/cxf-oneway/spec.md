# cxf-oneway Specification

## Purpose
TBD - created by archiving change 363-cxf-oneway. Update Purpose after archive.
## Requirements
### Requirement: Operation shape follows the selected WSDL binding

The bridge SHALL classify an operation as one-way only when the operation in the selected service/port binding has input and no WSDL output. Explicit producer operation names SHALL resolve in that binding. When no name is supplied, the input payload root QName SHALL resolve to exactly one binding operation input. The listener SHALL resolve the SOAP body root QName in its selected binding; a supplied SOAPAction SHALL agree with that operation's declared action. Unknown, conflicting, or ambiguous operations SHALL fail before remote transmission or route dispatch. The same resolved operation SHALL supply SOAPAction and exchange shape.

#### Scenario: Explicit one-way operation selects binding metadata

- **GIVEN** a selected SOAP 1.1 binding with input-only `ReceiveMessageOp`
- **WHEN** the producer invokes `ReceiveMessageOp`
- **THEN** its declared SOAPAction and input-only shape are selected, regardless of Rust exchange pattern

#### Scenario: Missing name resolves from body QName

- **GIVEN** an input QName matching exactly one operation in the selected binding
- **WHEN** the producer omits its operation name
- **THEN** that operation supplies action and exchange shape

#### Scenario: Unknown or ambiguous operation is rejected

- **GIVEN** an explicit name absent from the binding or an unnamed input matching zero or multiple operations
- **WHEN** the producer sends
- **THEN** it fails before issuing any HTTP request

#### Scenario: Listener rejects conflicting action

- **GIVEN** a SOAP body resolving to an input-only operation and a different supplied SOAPAction
- **WHEN** the listener receives the request
- **THEN** it returns a version-matched Sender fault without dispatching to the route

### Requirement: One-way producer completes without parsing a SOAP reply

For an input-only operation the producer SHALL call `Dispatch.invokeOneWay` exactly once with the bare payload. Empty HTTP 200 and 202 acknowledgments SHALL complete successfully with empty output bytes. The bridge SHALL NOT deserialize or transform an absent response body. The selected WSDL binding SHALL continue to control SOAP version, and CXF SHALL create exactly one envelope. Existing profile mTLS checks SHALL remain enforced.

#### Scenario: Empty 200 and 202 over mTLS

- **GIVEN** a SOAP 1.1 input-only WSDL operation and an mTLS fake server that records each request and returns either HTTP 200-empty or HTTP 202-empty
- **WHEN** `cxf_oneway_empty_200_and_202_complete_ok_single_envelope_over_mtls` sends one message for each isolated variant
- **THEN** each result is `Ok` with empty output, exactly one POST, and exactly one Envelope

### Requirement: One-way fault semantics are explicitly non-robust

Producer one-way dispatch SHALL set `org.apache.cxf.oneway.robust` to false. This change SHALL expose no public `oneway.robust` option. Successful send completion SHALL NOT assert remote business success or guarantee observation of a remote SOAP fault. Failures detected by the local bridge or transport SHALL remain errors; they SHALL NOT be converted into empty successful responses. Neither invocation failures nor lost bridge replies SHALL automatically resend a possibly transmitted invocation.

#### Scenario: Local transport failure is not an ACK

- **GIVEN** an input-only operation and an unreachable transport peer
- **WHEN** one-way dispatch fails locally
- **THEN** the caller receives an error and no automatic second invocation occurs

#### Scenario: Lost bridge response does not replay a send

- **GIVEN** the bridge invocation returns a transport error after a potentially transmitted request
- **WHEN** the Rust producer handles the result
- **THEN** that send fails without replay; bridge recovery may serve a later explicit send

### Requirement: Empty request-reply responses are protocol errors

An operation with WSDL output SHALL continue to use `Dispatch.invoke`. An empty HTTP 200 or 202 response SHALL be an error, never implicit one-way success. Null/empty response Sources and CXF parsing failures for absent SOAP content SHALL remain failures. The bridge SHALL NOT fall back to `invokeOneWay`, fabricate a SOAP payload, or resend.

#### Scenario: Request-reply empty success status is rejected

- **GIVEN** a WSDL operation with output and a server returning HTTP 200-empty or HTTP 202-empty
- **WHEN** the producer invokes that operation once
- **THEN** the result is an error and exactly one POST was recorded

### Requirement: Input-only consumer returns a correlated empty 202 ACK

The listener SHALL retain all existing transport availability, XML, WSS, signer identity, capacity, and timeout gates. For a resolved input-only operation it SHALL wait for a correlated, non-fault `ConsumerResponse`, including successful Rust response marshalling, then return HTTP 202 with a zero-length body. It SHALL discard the successfully marshalled response payload and SHALL NOT wrap or sign an empty ACK. Route success alone SHALL NOT bypass response marshalling. Failures SHALL retain the existing fault/status contract; stream unavailability SHALL retain HTTP 503. Request-reply success SHALL remain SOAP HTTP 200.

#### Scenario: One-way ACK waits for completion

- **GIVEN** a valid input-only request dispatched to a route whose completion is held by a test latch
- **WHEN** the listener waits and the route later completes with a correlated non-fault response containing successfully marshalled nonempty output
- **THEN** no early success response is emitted, and completion yields empty HTTP 202 without an Envelope

#### Scenario: Successful route with unsupported output remains a fault

- **GIVEN** an input-only route that completes successfully with `Body::Stream`
- **WHEN** Rust response marshalling rejects that output
- **THEN** the correlated response is a Receiver fault with HTTP 500, never successful 202

#### Scenario: One-way failure preserves the fault matrix

- **GIVEN** an input-only request with WSS validation failure, route fault, route timeout, or unavailable consumer stream
- **WHEN** the listener responds
- **THEN** WSS/route/timeout failures follow the existing version-matched fault matrix and unavailable streams return 503, never successful 202

#### Scenario: Request-reply response is unchanged

- **GIVEN** a valid operation with WSDL output
- **WHEN** its route completes successfully
- **THEN** the listener returns its existing single SOAP envelope and HTTP 200

