# sidecar-xml-security Specification

## Purpose
TBD - created by archiving change sidecar-xml-hardening. Update Purpose after archive.
## Requirements
### Requirement: Saxon secondary resolvers deny-all

The XML sidecar SHALL reject any stylesheet attempt to resolve secondary Saxon resources —
`unparsed-text()`, `unparsed-text-available()`, `collection()`, `uri-collection()`, and
`xsl:result-document` — by installing deny-all `UnparsedTextURIResolver`,
`CollectionFinder`, and `OutputURIResolver` on the Saxon `Configuration` used to compile
stylesheets.

#### Scenario: unparsed-text SSRF attempt rejected without connection

- **GIVEN** a compiled stylesheet whose transform calls `unparsed-text('<observing-server-url>')`,
  where the observing server is a local `ServerSocket` bound to `127.0.0.1` on an ephemeral
  port that counts accepted connections
- **WHEN** the transform is executed
- **THEN** the response carries a `BridgeError` AND the observing server's accepted-connection
  count is zero (the resolution was denied before any network attempt)

#### Scenario: collection attempt rejected

- **GIVEN** a compiled stylesheet whose transform calls `collection('<observing-server-url>')`,
  where the observing server is a local `ServerSocket` bound to `127.0.0.1` on an ephemeral port
- **WHEN** the transform is executed
- **THEN** the response carries a `BridgeError`

#### Scenario: result-document file-write attempt rejected

- **GIVEN** a compiled stylesheet containing `xsl:result-document href="file://<tmp>/canary.txt"`
- **WHEN** the transform is executed
- **THEN** the response carries a `BridgeError` AND no canary file exists at the target path

### Requirement: WSS replay protection on consumer inbound path

The CXF sidecar's inbound WS-Security processing SHALL attach a replay cache spanning the
endpoint lifetime to `RequestData` (both timestamp and nonce caches), so a captured,
still-fresh signed SOAP message is rejected when replayed. The publisher SHALL construct at
most one `WssSecurityProcessor` per security profile so that cache state persists across
requests. The emitted `wsu:Timestamp` SHALL be covered by the signature action, and inbound
verification SHALL require a Timestamp on messages whose profile declares the Timestamp
action — so an attacker cannot mint a fresh cache key by rewriting or stripping the
unsigned timestamp of a captured message.

#### Scenario: replayed fresh signed message rejected at processor level

- **GIVEN** a `WssSecurityProcessor` with signing configured and actions `Timestamp Signature`,
  and a signed+timestamped SOAP envelope produced by `processOutbound`
- **WHEN** `processInbound` processes the identical envelope bytes twice on the same processor
- **THEN** the first invocation succeeds and the second invocation throws a
  `WSSecurityException`

#### Scenario: replayed message rejected through the published endpoint

- **GIVEN** a published consumer endpoint whose profile enables inbound verification with
  actions `Timestamp Signature`, and a validly signed+timestamped SOAP request
- **WHEN** the identical request bytes are POSTed to the endpoint twice
- **THEN** the first request succeeds and the second request fails with a security failure

#### Scenario: timestamp rewrite cannot mint a fresh cache key

- **GIVEN** a validly signed+timestamped SOAP request accepted by the endpoint, and the same
  request bytes with the `wsu:Timestamp` element rewritten to fresh Created/Expires values
  (unsigned, signature broken)
- **WHEN** the original bytes and the timestamp-rewritten variant are each POSTed
  again after the original was accepted
- **THEN** both are rejected with a security failure (replay or signature validation), and
  neither is processed as a fresh message

### Requirement: Hardened identity transformer factories

All identity `TransformerFactory` instances in the CXF sidecar SHALL set
`FEATURE_SECURE_PROCESSING=true` and empty `ACCESS_EXTERNAL_DTD` and
`ACCESS_EXTERNAL_STYLESHEET`, without changing serialization output.

#### Scenario: identity serialization unchanged with hardened factory

- **GIVEN** a fixed SOAP DOM input and a fixed expected output literal for each of the three
  call sites (`CxfBridgeService.toXmlString`, `SoapEnvelopeHelper.sourceToBytes`,
  `SoapEnvelopeHelper.sourceToString`)
- **WHEN** each site serializes the fixed input
- **THEN** each site's output equals its expected literal AND the factory reports
  secure-processing enabled with empty external DTD and stylesheet access lists

### Requirement: Fail-loud secure DocumentBuilderFactory configuration

The CXF sidecar's secure `DocumentBuilderFactory` configuration SHALL propagate feature
configuration failures as `IllegalStateException` instead of silently returning a
weakly-configured factory.

#### Scenario: configuration failure surfaces as IllegalStateException

- **GIVEN** a `DocumentBuilderFactory` stub whose `setFeature` throws
  `ParserConfigurationException`
- **WHEN** the secure configuration seam is applied to the stub
- **THEN** an `IllegalStateException` wrapping the original cause is thrown

#### Scenario: initialized factory has all hardening features enabled

- **GIVEN** the shared `SECURE_DBF` factory after initialization
- **WHEN** its hardening features are queried
- **THEN** DOCTYPE declarations are disallowed, external general/parameter entities are
  disabled, external DTD loading is disabled, and XInclude awareness is off

### Requirement: Deterministic Xerces factory selection in the XML bridge

The XML bridge SHALL construct its schema and parser factories directly from the Xerces-J reference implementations on the classpath, without JAXP ServiceLoader discovery; SHALL include the Xerces message-bundle resources in the native image; and SHALL register the Xerces implementation classes that its internal `ObjectFactory` loads reflectively by name, so validation works and diagnostics are available in every build mode.

#### Scenario: schema registration and validation work in the native image

- **GIVEN** the XML bridge native binary with the Xerces reflective fallback targets registered
- **WHEN** a schema is registered and a document validated
- **THEN** the `ObjectFactory` lookups resolve and no `Provider ... not found` configuration error surfaces

#### Scenario: invalid document yields a schema diagnostic in the native image

- **GIVEN** the XML bridge native binary built with the GraalVM 25 toolchain
- **WHEN** a document that violates the configured XSD is validated
- **THEN** the returned error is of kind VALIDATION_FAILED carrying a schema diagnostic, and does not reference any resource-bundle load failure

#### Scenario: security hardening applies to the explicit factories

- **GIVEN** the bridge constructs Xerces-J factories directly
- **WHEN** a schema factory or a SAX parser factory is configured for validation work
- **THEN** the schema factory enables secure processing and denies external DTD and schema access, and the SAX source path denies external entities and DOCTYPE declarations and installs a deny-all entity resolver, identical to the previous discovery-based configuration

#### Scenario: deny-all behavior survives the factory switch

- **GIVEN** the bridge native binary with explicit Xerces-J factories
- **WHEN** a validated document carries a DOCTYPE declaration or entity reference, or the schema declares an external import
- **THEN** the external access is denied or ignored exactly as before the switch

#### Scenario: XSLT path keeps parser diagnostics in the native image

- **GIVEN** the bridge native binary
- **WHEN** a malformed XML document is transformed through the XSLT path
- **THEN** the reported error carries a readable parser diagnostic and does not reference any resource-bundle load failure

