## ADDED Requirements

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
