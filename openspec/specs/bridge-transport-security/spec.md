# bridge-transport-security Specification

## Purpose
TBD - created by archiving change bridge-lockstep-hardening. Update Purpose after archive.
## Requirements
### Requirement: Secure broker URI schemes activate TLS on the JMS sidecar

The JMS sidecar SHALL map broker URI schemes honestly: `ssl://` and `wss://`
SHALL configure the Artemis remote locator with SSL enabled
using the bridge's TLS material; plaintext-scheme URIs SHALL remain plaintext.
A secure scheme without usable TLS material SHALL abort sidecar startup with
an actionable error. No scheme SHALL be silently downgraded to plaintext.

#### Scenario: ssl scheme enables SSL transport

- **GIVEN** broker URI `ssl://broker:61617` and valid TLS material configured
  for the sidecar
- **WHEN** the JMS client factory builds the Artemis remote locator
- **THEN** the transport configuration has `SSL_ENABLED_PROP_NAME=true` and
  key/trust store properties set from the sidecar TLS material

#### Scenario: secure scheme without TLS material fails startup

- **GIVEN** broker URI `ssl://broker:61617` and missing or placeholder TLS
  material
- **WHEN** the sidecar starts
- **THEN** startup aborts with an `IllegalStateException` naming the missing
  material, and no plaintext connection is attempted

#### Scenario: plaintext scheme stays plaintext

- **GIVEN** broker URI `tcp://broker:61616`
- **WHEN** the JMS client factory builds the Artemis remote locator
- **THEN** the transport configuration has no SSL properties and connects
  without TLS

### Requirement: JMS consumer caps message body allocation

The JMS sidecar consumer SHALL bound every `BytesMessage` body allocation by
a configurable limit (`JMS_MAX_BODY_BYTES`, default 16 MiB). A message whose
body length exceeds the cap SHALL be routed to the bridged-error path
(warn-logged) without attempting the full allocation.

#### Scenario: oversized BytesMessage rejected without full allocation

- **GIVEN** a consumer with `JMS_MAX_BODY_BYTES=1024` and a mocked
  `BytesMessage` whose `getBodyLength()` reports 4096
- **WHEN** the message is consumed
- **THEN** no allocation of 4096 bytes occurs, the exchange carries the
  error outcome, and a `warn`-level log names the cap

### Requirement: CXF consumer address scheme validated fail-loud

The CXF sidecar SHALL bind its SOAP listener only for `http://` consumer
addresses. Any other scheme — including `https://` — SHALL fail loudly:
the Rust `camel-cxf` config layer SHALL reject the address at route-build
time, and the sidecar SHALL abort startup if it ever receives a non-`http`
address. Silent plaintext serving of a secure-scheme address is prohibited.

#### Scenario: https consumer address rejected at config layer

- **GIVEN** a camel-cxf consumer endpoint configured with address
  `https://0.0.0.0:9000/soap`
- **WHEN** the route is built
- **THEN** configuration validation rejects the address with an error stating
  TLS listener support is not yet available

#### Scenario: sidecar aborts on non-http address

- **GIVEN** the CXF sidecar started with `CXF_ADDRESS=https://0.0.0.0:9000/soap`
- **WHEN** the publisher initializes
- **THEN** startup aborts with an actionable message before any socket binds

### Requirement: CXF listener caps request body size

The CXF sidecar SOAP listener SHALL cap aggregated request bodies by a
configurable limit (`CXF_MAX_BODY_BYTES`, default 16 MiB). A request whose
declared `Content-Length` exceeds the cap SHALL be rejected before body
aggregation begins; a request that streams past the cap mid-body SHALL be
rejected mid-stream. Oversized requests SHALL receive an HTTP error, not an
out-of-memory failure.

#### Scenario: declared oversized Content-Length rejected upfront

- **GIVEN** a published endpoint with the default body cap
- **WHEN** a POST arrives with `Content-Length` greater than the cap
- **THEN** the listener responds with an HTTP 413 error without reading the
  body

#### Scenario: lying Content-Length rejected mid-stream

- **GIVEN** a published endpoint with the default body cap
- **WHEN** a POST declares a small `Content-Length` but streams more bytes
  than the cap
- **THEN** the listener aborts reading and responds with an HTTP 413 error

### Requirement: Bridge dependency security floor

All three bridges SHALL ship dependency versions at or above the security
floor: Apache CXF ≥ 4.1.8, ActiveMQ Classic client ≥ 5.19.10, and a
non-EOL Quarkus release whose resolved Netty is ≥ 4.1.137, verified via
resolved-dependency output of each bridge's Gradle build.

#### Scenario: resolved dependencies meet the floor

- **WHEN** the Gradle dependency report is resolved for each bridge
- **THEN** CXF resolves ≥ 4.1.8, ActiveMQ resolves ≥ 5.19.10, and Netty
  resolves ≥ 4.1.137 in all three bridges

### Requirement: CXF producer Dispatch cache is request-scoped and immutable after publish

The cxf bridge's cached `Dispatch<Source>` clients SHALL be keyed by a
typed key comprising WSDL, address, service, port, security profile,
operation, and normalized request timeout (request timeout when set,
default connection timeout otherwise). The request context of a cached
Dispatch SHALL NOT be mutated after the Dispatch is published to the
cache: endpoint address, both client timeouts, and the SOAPAction
properties (`jakarta.xml.ws.soap.http.soapaction.use`,
`jakarta.xml.ws.soap.http.soapaction.uri`) SHALL be set during Dispatch
creation only. Concurrent invokes that differ in operation or timeout
SHALL each observe their own values.

#### Scenario: concurrent distinct operations do not cross-contaminate

- **GIVEN** an endpoint tuple warm for operation `opA`
- **WHEN** a caller invokes the same tuple with operation `opB` concurrently with an `opA` invoke
- **THEN** two distinct cache entries exist and each invoke carries its own SOAPAction

#### Scenario: differing timeouts get distinct dispatches

- **GIVEN** an endpoint tuple warm with the default timeout
- **WHEN** a caller requests the same tuple with an explicit per-request timeout
- **THEN** a distinct cache entry is created carrying that timeout, and the default-timeout entry's context is unchanged

#### Scenario: no mutation after publish

- **GIVEN** a warm cache entry
- **WHEN** any subsequent request path executes
- **THEN** the service layer performs no request-context writes on the cached Dispatch (all context is set in creation only)

