# cxf-bridge-hardening — delta: bridge-transport-security

## ADDED Requirements

### Requirement: CXF security action tokens are validated against the materialized set

The CXF sidecar SHALL validate every token in a profile's inbound and
outbound action lists against the set the bridge actually materializes
— inbound: `Signature`, `Encrypt`, `Timestamp`; outbound: `Signature`,
`Encrypt`, `Timestamp`. `Timestamp` SHALL require `Signature` in the
same list (both directions) — a bare timestamp is not tamper-evident.
An unrecognized token SHALL fail profile construction with an error
naming the token, the supported set, and the raw actions string.
Blank/unset action lists SHALL remain raw-exempt.

#### Scenario: unknown inbound token rejected at build

- **GIVEN** a profile with `actions.in` containing `UsernameToken` and a
  configured truststore
- **WHEN** the profile is built
- **THEN** an `IllegalArgumentException` is thrown naming
  `UsernameToken`, the supported inbound set (Signature, Encrypt,
  Timestamp), and the raw actions string

#### Scenario: unknown outbound token rejected at build

- **GIVEN** a profile with `actions.out` containing `SignatureConfirmation`
  and a configured keystore
- **WHEN** the profile is built
- **THEN** an `IllegalArgumentException` is thrown naming the token and
  the supported outbound set (Signature, Encrypt, Timestamp)

#### Scenario: bare inbound Timestamp rejected by composition

- **GIVEN** a profile with `actions.in=Timestamp` (no Signature) and a
  configured truststore
- **WHEN** the profile is built
- **THEN** construction fails with the Timestamp-requires-Signature
  composition rule — the same rule the outbound validator enforces

#### Scenario: inbound Timestamp Signature is fully enforced

- **GIVEN** a profile with `actions.in=Timestamp Signature` and crypto
  material configured
- **WHEN** the profile is built and the producer verifies a signed,
  timestamped response
- **THEN** construction succeeds, the inbound interceptor materializes
  WSS4J timestamp validation alongside signature validation, and the
  timestamp-signature coverage check applies — the pre-existing live
  combination is preserved end to end

#### Scenario: known token sets still build

- **GIVEN** profiles with `actions.in=Encrypt` (keystore present) and
  `actions.out=Timestamp Signature` (keystore present, composition
  satisfied)
- **WHEN** each profile is built
- **THEN** both constructions succeed and existing material checks apply
  unchanged

#### Scenario: blank actions stay raw-exempt

- **GIVEN** a profile with unset `actions.in` and `actions.out`
- **WHEN** the profile is built
- **THEN** construction succeeds with no action validation performed

### Requirement: CXF producer caps response body allocation

The CXF producer path SHALL bound the serialized remote response body
by `CXF_MAX_BODY_BYTES` (same env, default, ceiling, and fail-loud
parse as the consumer request cap). A response whose serialized size
exceeds the cap SHALL fail the invoke with `RESOURCE_EXHAUSTED` whose
description names the env var and the observed size that exceeded it,
without forwarding a payload.

#### Scenario: oversized response rejected with RESOURCE_EXHAUSTED

- **GIVEN** a sidecar with `CXF_MAX_BODY_BYTES=1024` and a remote whose
  response payload serializes beyond 1024 bytes
- **WHEN** a route invokes the producer
- **THEN** the gRPC call fails `RESOURCE_EXHAUSTED` with a description
  containing both `CXF_MAX_BODY_BYTES` and the observed serialized
  size, and no payload is forwarded

#### Scenario: response at exactly the cap passes

- **GIVEN** a sidecar with `CXF_MAX_BODY_BYTES=1024` and a remote whose
  response serializes to exactly 1024 bytes
- **WHEN** a route invokes the producer
- **THEN** the response payload is forwarded unchanged

#### Scenario: cap env malformed fails startup loud

- **GIVEN** a sidecar with `CXF_MAX_BODY_BYTES=abc`
- **WHEN** the sidecar starts
- **THEN** startup aborts with an `IllegalStateException` naming the
  env var and the raw value (ADR-0033)

### Requirement: CXF inbound body extraction is namespace-aware

The CXF inbound request handler SHALL extract the SOAP Body payload by
parsing the envelope (namespace-aware DOM) and taking the first element
child of the Body — via `SoapEnvelopeHelper.extractBody` — instead of
string-scanning. An envelope without a Body SHALL forward an empty
payload; malformed XML SHALL fail as a 400 without forwarding.

#### Scenario: prefixed body extracted by local name

- **GIVEN** an inbound envelope using a `soapenv:` prefix
- **WHEN** the handler extracts the payload
- **THEN** the first element child of the `soapenv:Body` is forwarded,
  serialized

#### Scenario: decoy element name is not mis-extracted

- **GIVEN** an inbound envelope whose payload root element is named
  `xsd:bodyData`
- **WHEN** the handler extracts the payload
- **THEN** extraction keys on the Body element by local name and
  namespace — the `xsd:bodyData` payload is forwarded intact, not
  spliced at the `":body"` substring

#### Scenario: missing body forwards empty payload

- **GIVEN** an inbound envelope with no Body element
- **WHEN** the handler extracts the payload
- **THEN** an empty payload is forwarded — not the whole envelope

#### Scenario: malformed envelope fails as 400

- **GIVEN** an inbound request whose bytes are not well-formed XML
- **WHEN** the handler parses the envelope
- **THEN** the request is rejected with HTTP 400 and nothing is
  forwarded

### Requirement: CXF dispatch cache is bounded and closed deterministically

The CXF sidecar SHALL bound the Dispatch cache by `CXF_MAX_DISPATCHES`
(default 64, ceiling 1024, fail-loud parse per ADR-0033), evicting the
least-recently-used entry when an insertion would exceed the cap.
Cache lookup and cold creation SHALL be serialized under one lock with
the SOAP invoke outside it, preserving per-key creation atomicity.
Evicted entries and entries remaining at shutdown SHALL be closed via
the CXF Dispatch close API, best-effort per entry.

#### Scenario: eviction bounds the cache at the cap

- **GIVEN** `CXF_MAX_DISPATCHES=2` and three distinct dispatch keys
- **WHEN** the third dispatch is requested
- **THEN** the least-recently-used entry is evicted and closed, the
  cache size stays at 2, and the new dispatch serves the request

#### Scenario: LRU order follows access

- **GIVEN** `CXF_MAX_DISPATCHES=2` with entries A (older) and B (newer),
  and a subsequent request for A
- **WHEN** a third distinct key is requested
- **THEN** B is evicted (A was touched most recently) and A remains
  cached

#### Scenario: malformed dispatch cap fails startup loud

- **GIVEN** `CXF_MAX_DISPATCHES=0` (or a non-numeric value)
- **WHEN** the sidecar starts
- **THEN** startup aborts with an `IllegalStateException` naming the
  env var and the raw value

#### Scenario: shutdown closes all cached dispatches

- **GIVEN** a sidecar with two cached dispatches
- **WHEN** the sidecar shuts down (`@PreDestroy`)
- **THEN** both entries are closed via the Dispatch close API
  best-effort and the cache is empty
