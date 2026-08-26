## ADDED Requirements

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
requests.

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
