# bridge-transport-security Specification

## Purpose
TBD - created by archiving change bridge-lockstep-hardening. Update Purpose after archive.
## Requirements
### Requirement: Secure broker URI schemes activate TLS on the JMS sidecar

The JMS sidecar SHALL map broker URI schemes honestly: `ssl://` and `wss://`
SHALL configure the Artemis remote locator with SSL enabled
using the bridge's TLS material; plaintext-scheme URIs SHALL remain plaintext.
A secure scheme without usable TLS material SHALL abort sidecar startup with
an actionable error. A URI whose scheme is none of `tcp`, `ws`, `ssl`,
`wss` SHALL abort transport setup with an actionable error naming the scheme
and the remediation (unwrap to a single primary broker URL); no scheme SHALL
be silently downgraded to plaintext, and no URI SHALL be silently redirected
to a default host — a URI without a host aborts setup.

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

#### Scenario: failover-wrapped URI aborts transport setup

- **GIVEN** a broker URI whose scheme is `failover` (parenthesized inner or
  `failover://`-prefixed)
- **WHEN** the JMS client factory builds the Artemis transport configuration
- **THEN** setup throws `IllegalStateException` naming the unsupported
  scheme and the remediation (unwrap to a single primary broker URL,
  HA broker-side or via multiple broker entries)
- **AND** no connection to `localhost` or any default host is attempted

#### Scenario: URI without a host aborts transport setup

- **GIVEN** a broker URI whose scheme is known (`ssl`) but whose host is
  missing or blank (e.g. `ssl://:61617`)
- **WHEN** the JMS client factory builds the Artemis transport configuration
- **THEN** setup throws an `IllegalStateException` naming the URL and
  stating that no default host is assumed

#### Scenario: Rust config rejects failover URLs for Artemis at validation

- **GIVEN** an `artemis` broker entry whose `broker_url` starts with
  `failover://`
- **WHEN** the Rust pool config validates
- **THEN** validation fails with an error naming the URL and pointing at
  single-primary-URL or multiple-broker-entries remediation

#### Scenario: Rust config accepts failover URLs for Classic brokers

- **GIVEN** an `activemq` (Classic) broker entry whose `broker_url` starts
  with `failover://`
- **WHEN** the Rust pool config validates
- **THEN** validation passes (the Classic path hands the URL to
  `ActiveMQConnectionFactory`, which supports failover natively)

### Requirement: JMS consumer caps message body allocation

The JMS sidecar consumer SHALL cap forwarded message body allocation at
`JMS_MAX_BODY_BYTES` (ceiling 19 MiB, default 16 MiB), staying at or below
the Rust IPC decode limit. `BytesMessage` bodies SHALL be checked against
the pre-read body length without attempting the full allocation;
`TextMessage` bodies SHALL be materialized and UTF-8 encoded first, then
checked against the encoded byte length — the cap bounds the forwarded
body size, not the peak sidecar allocation. A body whose measured size
exceeds the cap SHALL be rejected with a warn-level diagnostic naming the
measured size in bytes and the cap, and SHALL NOT be forwarded; the reject
is a bridged error outcome (the consumer logs at warn and forwards the
error, the route owns the operational signal). The
bridge README SHALL document that the TextMessage cap counts UTF-8 bytes.

#### Scenario: TextMessage whose UTF-8 encoding exceeds the cap is rejected

- **GIVEN** a `TextMessage` whose text is at or below the cap in UTF-16
  code units but whose UTF-8 encoding exceeds the cap (e.g. CJK-heavy text)
- **WHEN** the consumer converts the message
- **THEN** conversion throws a `JMSException` whose diagnostic reports the
  UTF-8 byte size and the cap
- **AND** the message body never reaches the protobuf body or the stream

#### Scenario: ASCII text at exactly the cap passes

- **GIVEN** a `TextMessage` whose ASCII text encodes to exactly
  `JMS_MAX_BODY_BYTES` UTF-8 bytes
- **WHEN** the consumer converts the message
- **THEN** the message is forwarded with the full body intact

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

### Requirement: CXF producer emits Timestamp when out-actions request it

The bridge-side producer security profile SHALL honor a `Timestamp` token
in an accepted outbound action set (Timestamp paired with Signature) by
emitting a wsu:Timestamp in the outbound message. The outbound action
list SHALL order Timestamp before Signature so that signature-part
resolution covers an already-materialized element, mirroring the consumer
path. When both Signature and Timestamp are active and no explicit
signature parts are configured, the applied coverage SHALL default to
Body + Timestamp.

#### Scenario: Timestamp token produces an emitting, correctly ordered interceptor

- **GIVEN** a security profile with out-actions "Signature Timestamp" and a signing keystore
- **WHEN** the profile builds its outbound WSS4J interceptor
- **THEN** the interceptor's action list contains both tokens with Timestamp preceding Signature

#### Scenario: signed message carries a covered Timestamp

- **GIVEN** out-actions "Signature Timestamp", a signing keystore, and no SIGNATURE_PARTS configured
- **WHEN** a message is processed by the outbound interceptor
- **THEN** the output XML contains a wsu:Timestamp element and the signature's references cover the SOAP Body and the wsu:Timestamp element

#### Scenario: explicit parts are respected verbatim on the wire

- **GIVEN** out-actions "Signature Timestamp", a signing keystore, and an explicit SIGNATURE_PARTS of Body-only ("Body")
- **WHEN** a message is processed by the outbound interceptor
- **THEN** the signature's references cover exactly the SOAP Body element and include no Timestamp reference

#### Scenario: outbound wire format is unchanged for Timestamp-free profiles

- **GIVEN** a security profile with out-actions "Signature", a signing keystore, and no SIGNATURE_PARTS configured
- **WHEN** the outbound interceptor signs a message
- **THEN** the output carries no wsu:Timestamp and the signature's references cover the SOAP Body only, identical to pre-change behavior

### Requirement: CXF producer inbound security processing is wired and functional

The bridge-side producer's inbound WSS4J interceptor SHALL pass crypto
configuration in the property shape WSS4J requires — String reference-id
properties naming keys that hold the crypto `Properties` objects — so
that configured inbound Signature verification and Encrypt decryption of
peer responses function at runtime.

#### Scenario: signed peer response verifies through the inbound interceptor

- **GIVEN** a profile with a truststore and in-actions "Signature"
- **WHEN** a document signed by the outbound interceptor chain is processed by the profile's inbound interceptor on a real in-phase chain
- **THEN** processing completes without exception and the Body content survives verification

#### Scenario: encrypted peer response decrypts through the inbound interceptor

- **GIVEN** a profile with a keystore and in-actions "Encrypt"
- **WHEN** an encrypted document is processed by the profile's inbound interceptor on a real in-phase chain
- **THEN** processing completes without exception and the original content round-trips

#### Scenario: crypto properties use the reference-id shape

- **GIVEN** a profile with inbound security actions configured
- **WHEN** the inbound interceptor is created
- **THEN** SIG_PROP_REF_ID and DEC_PROP_REF_ID hold String reference-ids, and the crypto Properties objects are stored under those reference-id keys

### Requirement: Inbound action validation

The profile builder SHALL reject an explicitly configured inbound action
string whose actions lack their crypto material at build time, so that no
profile can silently skip inbound verification or decryption.

#### Scenario: signature action without truststore is rejected

- **GIVEN** a keystore path is configured
- **WHEN** the builder is given `actions.in` containing `Signature` and no
  truststore path
- **THEN** `build()` throws `IllegalArgumentException` naming
  `cxf.truststore.path` and stating that the keystore fallback applies only
  to the manual consumer path

#### Scenario: encrypt action without keystore is rejected

- **GIVEN** a truststore path is configured
- **WHEN** the builder is given `actions.in` containing `Encrypt` and no
  keystore path
- **THEN** `build()` throws `IllegalArgumentException` naming
  `cxf.keystore.path`

#### Scenario: blank inbound actions stay exempt

- **WHEN** the builder is given no `actions.in` value and no truststore or
  keystore
- **THEN** `build()` succeeds and `createInInterceptor()` returns `null`

#### Scenario: whitespace-only inbound actions stay exempt

- **WHEN** the builder is given `actions.in` of `"   "` and no truststore or
  keystore
- **THEN** `build()` succeeds and `createInInterceptor()` returns `null`

#### Scenario: signature-only with truststore builds

- **GIVEN** a truststore path is configured and no keystore
- **WHEN** the builder is given `actions.in` of `Signature`
- **THEN** `build()` succeeds and `createInInterceptor()` returns a
  non-null interceptor configured for Signature only

#### Scenario: encrypt-only with keystore builds

- **GIVEN** a keystore path is configured and no truststore
- **WHEN** the builder is given `actions.in` of `Encrypt`
- **THEN** `build()` succeeds and `createInInterceptor()` returns a
  non-null interceptor configured for Encrypt only

#### Scenario: both actions with both materials build

- **GIVEN** truststore and keystore paths are configured
- **WHEN** the builder is given `actions.in` of `Signature Encrypt`
- **THEN** `build()` succeeds and `createInInterceptor()` returns a
  non-null interceptor configured for both actions

### Requirement: Bridge consumer teardown destroys each consumer exactly once

The JMS bridge service SHALL destroy each consumer exactly once across all
interleavings of stream cleanup and sidecar shutdown: a teardown path SHALL
destroy a consumer only when it wins the owner-checked removal of that
consumer's map entry. Shutdown SHALL set an admission flag before draining
the map, and `subscribe` SHALL refuse new streams once the flag is set
(destroying its freshly created consumer). A late stream-termination path
racing or following `@PreDestroy` shutdown SHALL NOT trigger a second
destroy of any consumer, and no consumer present in the map at shutdown
SHALL leak (never destroyed).

#### Scenario: late stream termination after shutdown does not double-destroy

- **GIVEN** an active subscription whose consumer was already stopped and
  destroyed by `@PreDestroy` shutdown (entry removed by the shutdown drain)
- **WHEN** the stream's termination path subsequently runs its cleanup
- **THEN** cleanup stops nothing new, removes no entry, and does NOT call
  the factory destroy for that consumer a second time

#### Scenario: shutdown and cleanup racing on the same consumer destroy exactly once

- **GIVEN** an active subscription while shutdown begins iterating the
  consumer map
- **WHEN** stream cleanup and shutdown both attempt teardown of the same
  consumer
- **THEN** exactly one of the two destroys the consumer (the winner of the
  owner-checked removal), and the loser performs no destroy

#### Scenario: normal stream completion still destroys its consumer

- **GIVEN** an active subscription not racing shutdown
- **WHEN** the stream completes and its cleanup runs
- **THEN** the consumer is stopped and destroyed exactly once

#### Scenario: subscribe after shutdown begins is refused

- **GIVEN** the shutdown admission flag is set
- **WHEN** a new subscribe stream arrives
- **THEN** it is rejected with an unavailable status, and its freshly
  created consumer is destroyed without ever registering

#### Scenario: registration racing the shutdown flag is linearized

- **GIVEN** a subscribe stream and `@PreDestroy` shutdown racing
- **WHEN** both execute their admission/registration and flag-set critical
  sections
- **THEN** exactly one order holds: the registration completed first (the
  entry is in the map when shutdown drains, so the consumer is destroyed
  by the drain) or the flag was set first (the subscribe refuses and
  destroys its own consumer) — no registration both passes admission and
  escapes the drain

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

