# camel-http Specification

## Purpose
HTTP endpoint behavior. It covers JSON error replies, the
`maxInflightRequests` upper bound, TLS material under the CA-less webpki
fallback, and duplicate consumer-registration rejection.
## Requirements
### Requirement: JSON error replies preserve existing behavior

The HTTP error finalizer SHALL construct JSON replies for
`TypeConversionFailed`, `ValidationError`, `UnsupportedMediaType`, and
`NotAcceptable` through one private helper. `ValidationError` SHALL map to
400, `UnsupportedMediaType` to 415 (media negotiation gate, REST lowering),
and `NotAcceptable` to 406 (same gate). `TypeConversionFailed` SHALL map to
500 with the JSON error code `conversion_failed` and a log line that does not
claim a bad request (ADR-0096 fault blame: a conversion failure on data that
is not by definition request-borne is a server fault).
`Unauthenticated`/`Unauthorized` keep their `401`/`403` mappings and
`ConsumerStopping` keeps its `503 Service Unavailable` mapping. The remaining
fallback arm SHALL carve out `CamelError::Business`: a business error SHALL
map through the route's error mapping per the requirement "business error
HTTP mapping with safe closure" (built-in default kind map, then 500), never
through this fallback arm; every other remaining error maps to
`500 Internal Server Error`. The reply arm SHALL also never answer 2xx for an
exchange that still carries an error state (bd `rc-7lkp2`): when the route
result is an `Ok` exchange whose error is present, the finalizer SHALL treat
it as an error and map it like any other pipeline error — through the
EXISTING variant mappings for non-business errors (`ValidationError` 400,
`ConsumerStopping` 503, and so on) and through the route's error mapping for
a business error — never returning a success status. The never-2xx guarantee
scopes to errors not successfully handled: a handled (and therefore cleared)
error that completed the pipeline is the error handler's documented outcome,
not a finalizer input.

#### Scenario: Type conversion failure is a server error

- **GIVEN** `pipeline_error_to_reply` receives `TypeConversionFailed("bad")`
- **WHEN** it maps the error to an HTTP reply
- **THEN** the reply has status 500, `Content-Type: application/json`, and
  JSON fields `error: "conversion_failed"` and `message: "bad"`; neither the
  JSON error code nor the log line claims a bad request

#### Scenario: Consumer shutdown remains 503

- **GIVEN** `pipeline_error_to_reply` receives `CamelError::ConsumerStopping`
- **WHEN** it maps the error to an HTTP reply
- **THEN** the reply has status 503 and the shutdown mapping is unchanged

#### Scenario: Validation failure remains a bad request

- **GIVEN** `pipeline_error_to_reply` receives `ValidationError("invalid")`
- **WHEN** it maps the error to an HTTP reply
- **THEN** the reply has status 400, `Content-Type: application/json`, and
  JSON fields `error: "validation_error"` and `message: "invalid"`

#### Scenario: Unsupported media remains 415

- **GIVEN** `pipeline_error_to_reply` receives `UnsupportedMediaType` with
  consumed and declared media values
- **WHEN** it maps the error to an HTTP reply
- **THEN** the reply has status 415, `Content-Type: application/json`, and
  message `consumed {consumed}, declared {declared}`

#### Scenario: Unacceptable media remains 406

- **GIVEN** `pipeline_error_to_reply` receives `NotAcceptable` with accept and
  produced media values
- **WHEN** it maps the error to an HTTP reply
- **THEN** the reply has status 406, `Content-Type: application/json`, and
  message `accept {accept}, produced {produced}`

#### Scenario: Empty messages remain valid JSON

- **GIVEN** the helper receives an empty message
- **WHEN** it serializes the reply
- **THEN** the JSON message field is an empty string and the reply remains
  application/json

#### Scenario: business errors leave the fallback arm

- **GIVEN** the finalizer receives `CamelError::Business { kind: "conflict", .. }`
- **WHEN** it maps the error to an HTTP reply
- **THEN** the reply follows the business mapping (409 from the built-in
  default kind map), not the fallback 500

#### Scenario: an Ok exchange carrying an error never answers 2xx

- **GIVEN** a handler-configured route whose boundary path returns
  `Ok(exchange)` with the error still present on the exchange (the
  `rc-7lkp2` path)
- **WHEN** the reply arm produces the response
- **THEN** the reply maps the carried error like any other pipeline error —
  the route's error mapping for a business error, the existing variant
  mappings for non-business errors, 500 otherwise — and is never 2xx

#### Scenario: Ok exchange carrying ValidationError maps through the existing mapping

- **GIVEN** an `Ok` exchange whose error is `ValidationError("invalid")` (the
  `rc-7lkp2` path)
- **WHEN** the reply arm produces the response
- **THEN** the reply has status 400 — the existing variant mapping, reused

#### Scenario: Ok exchange carrying ConsumerStopping maps to 503

- **GIVEN** an `Ok` exchange whose error is `ConsumerStopping` (the
  `rc-7lkp2` path)
- **WHEN** the reply arm produces the response
- **THEN** the reply has status 503 — the existing shutdown mapping, reused

### Requirement: HTTP maxInflightRequests upper bound

The system SHALL reject `maxInflightRequests` values above
`tokio::sync::Semaphore::MAX_PERMITS` with a typed configuration
error (`CamelError::Config`) that names the parameter, the configured
value, and the limit. The rejection SHALL occur at URI parse, at
`create_consumer`, and at consumer start before shared-server
registry interaction, listener binding, consumer envelope-channel
construction, or inflight-semaphore construction; `spawn_entry` SHALL
additionally validate before any listener side effect and before the
semaphore primitive as defense-in-depth. The system SHALL NOT panic
during route startup for any representable `maxInflightRequests`
value. The value `0` SHALL remain accepted with its existing
reject-everything (503) semantics; no zero normalization SHALL be
introduced.

#### Scenario: oversized value rejected at URI parse

- **GIVEN** an http consumer URI with `maxInflightRequests` greater than `tokio::sync::Semaphore::MAX_PERMITS`
- **WHEN** the URI is parsed into `HttpServerConfig` (including via `from_uri_with_defaults`)
- **THEN** parsing fails with a typed configuration error naming `maxInflightRequests`, the configured value, and the limit

#### Scenario: oversized value rejected at consumer creation even when constructed directly

- **GIVEN** an `HttpComponent` whose `HttpServerConfig` was constructed directly (not via URI parse) with `maxInflightRequests` greater than the limit
- **WHEN** `create_consumer` is invoked
- **THEN** it returns a typed configuration error before any `HttpConsumer` is constructed

#### Scenario: oversized value rejected at consumer start before side effects

- **GIVEN** an `HttpConsumer` constructed directly with `maxInflightRequests` greater than the limit
- **WHEN** `start` is invoked
- **THEN** it returns a typed configuration error before shared-server registry interaction, listener binding, envelope-channel construction, or semaphore construction, and no panic occurs

#### Scenario: defense-in-depth before the semaphore primitive

- **GIVEN** `spawn_entry` invoked with `max_inflight_requests` greater than the limit
- **WHEN** it runs
- **THEN** it returns a typed configuration error before any listener side effect and before `tokio::sync::Semaphore::new` is called, and no panic occurs

#### Scenario: boundary values

- **GIVEN** the limit `L = tokio::sync::Semaphore::MAX_PERMITS`
- **WHEN** bound validation runs with `L-1`, `L`, and `L+1`
- **THEN** `L-1` and `L` are accepted unchanged and `L+1` is rejected with a typed configuration error

#### Scenario: zero-value semantics retained

- **GIVEN** a consumer configured with `maxInflightRequests=0`
- **WHEN** it starts and receives a request
- **THEN** the request is rejected with HTTP 503 as before (rc-3y6j reject-everything semantics); validation does not normalize or reject the zero value

#### Scenario: accepted boundary is constructible in Tokio primitives

- **GIVEN** the limit `L = tokio::sync::Semaphore::MAX_PERMITS`
- **WHEN** a semaphore of `L` permits is constructed
- **THEN** construction succeeds without panicking (the accepted upper bound is exactly the primitive's bound)

### Requirement: Strict TLS material survives the CA-less fallback

When the platform-verifier client build fails and the webpki fallback
triggers, and `tls.enabled` and `tls.strict` are both true, and custom
CA and/or mTLS material is configured, the effective fallback client
SHALL carry that material: the custom CA SHALL be merged into the
Mozilla webpki root store (trust-union parity with the primary path's
`add_root_certificate` semantics) and the mTLS cert/key SHALL be the
client identity. If any material-loading or rustls-configuration step
fails — unreadable file, zero parseable PEM CERTIFICATE sections, zero
roots accepted by the root store, unparseable private key, or
client-auth configuration rejection — `build_client` SHALL return
`CamelError::EndpointCreationFailed` naming the conflict. The fallback
SHALL NOT silently substitute Mozilla roots alone for configured
strict material.

#### Scenario: Custom CA carried into forced fallback

- **GIVEN** a forced-fallback environment (empty platform CA store),
  `tls` with `enabled=true`, `strict=true`, and a `ca_cert_path`
  whose PEM signs a local TLS server's certificate
- **WHEN** `build_client` builds the client and it requests `https://`
  from that server
- **THEN** the TLS handshake succeeds and the response is received

#### Scenario: Material-free client fails against custom-CA server

- **GIVEN** the same forced-fallback environment and local
  custom-CA-certified server
- **WHEN** `build_client` builds a client with strict on but NO
  `ca_cert_path` and requests the same URL
- **THEN** the TLS handshake fails (the CA material is causally
  carried, not incidental)

#### Scenario: Custom CA merged as trust union, not replacement

- **GIVEN** a custom CA PEM containing exactly one root certificate
- **WHEN** the fallback root store is assembled with that CA
- **THEN** the store contains the bundled Mozilla webpki anchors
  AND the custom root (union), not the custom root alone — an
  inspectable root-store construction seam makes the contents
  assertable

#### Scenario: mTLS identity carried into forced fallback

- **GIVEN** a forced-fallback environment, a local TLS server that
  REQUIRES a client certificate, and `tls` with `enabled=true`,
  `strict=true`, a valid `ca_cert_path`, and a matching
  `client_cert_path`/`client_key_path` pair
- **WHEN** `build_client` builds the client and it requests `https://`
  from that server
- **THEN** the TLS handshake succeeds including client-auth and the
  response is received

#### Scenario: mTLS-less client rejected by identity-requiring server

- **GIVEN** the same forced-fallback environment and
  client-cert-requiring server
- **WHEN** `build_client` builds a client with strict on, valid
  `ca_cert_path`, but NO client cert/key pair and requests the same
  URL
- **THEN** the server rejects the TLS handshake

#### Scenario: CA with zero PEM certificate sections fails closed

- **GIVEN** a forced-fallback environment and `tls` with
  `enabled=true`, `strict=true`, and a `ca_cert_path` whose file
  contains no parseable PEM CERTIFICATE section (e.g. only a private
  key or plain text)
- **WHEN** `build_client` is called
- **THEN** it returns `Err(CamelError::EndpointCreationFailed)` whose
  message names the strict/webpki-fallback conflict, and no fallback
  client is returned

#### Scenario: CA accepted by zero roots fails closed

- **GIVEN** a forced-fallback environment and `tls` with
  `enabled=true`, `strict=true`, and a `ca_cert_path` whose
  certificate bytes parse as PEM but are rejected by the root store
  (zero roots accepted)
- **WHEN** `build_client` is called
- **THEN** it returns `Err(CamelError::EndpointCreationFailed)` whose
  message names the strict/webpki-fallback conflict, and no fallback
  client is returned

#### Scenario: mTLS key with no parseable private-key section fails closed

- **GIVEN** a forced-fallback environment and `tls` with
  `enabled=true`, `strict=true`, a valid `client_cert_path`, and a
  `client_key_path` with no parseable private-key section
- **WHEN** `build_client` is called
- **THEN** it returns `Err(CamelError::EndpointCreationFailed)` whose
  message names the strict/webpki-fallback conflict, and no fallback
  client is returned

#### Scenario: PEM-valid but rustls-rejected mTLS key fails closed

- **GIVEN** a forced-fallback environment and `tls` with
  `enabled=true`, `strict=true`, a valid `client_cert_path`, and a
  `client_key_path` holding a PEM private-key section that parses at
  the PEM layer but is rejected by rustls at client-auth
  configuration (no usable key material)
- **WHEN** `build_client` is called
- **THEN** it returns `Err(CamelError::EndpointCreationFailed)` whose
  message names the strict/webpki-fallback conflict, and no fallback
  client is returned

#### Scenario: Unreadable strict material fails closed with typed error

- **GIVEN** a forced-fallback environment and `tls` with
  `enabled=true`, `strict=true`, and a `ca_cert_path` or mTLS path
  that cannot be read
- **WHEN** `build_client` is called
- **THEN** it returns `Err(CamelError::EndpointCreationFailed)` whose
  message names the strict/webpki-fallback conflict, and no fallback
  client is returned

#### Scenario: Material-free strict fallback retained

- **GIVEN** a forced-fallback environment and `tls` with
  `enabled=true`, `strict=true`, and no material configured
- **WHEN** `build_client` is called
- **THEN** the client builds successfully on Mozilla webpki roots
  with no configured-material warning

#### Scenario: Non-strict valid material carried into fallback

- **GIVEN** a forced-fallback environment, `tls` with `strict=false`,
  a valid `ca_cert_path` whose PEM signs a local TLS server's
  certificate
- **WHEN** `build_client` builds the client and it requests `https://`
  from that server
- **THEN** the TLS handshake succeeds — valid material is carried
  regardless of `strict`; no degradation warn fires for loadable
  material

#### Scenario: Non-strict material failure downgrades permissively, item-wise

- **GIVEN** a forced-fallback environment, `tls` with `strict=false`,
  a VALID `ca_cert_path` whose PEM signs a local TLS server's
  certificate (server does not require a client certificate), and an
  INVALID mTLS item (`client_key_path` unreadable)
- **WHEN** `build_client` builds the client and it requests `https://`
  from that server
- **THEN** the TLS handshake succeeds — the valid CA item is carried
  and enforces peer trust despite the identity item failing — and the
  existing configured-material warn fires for the failed mTLS item
  (permissive item-wise F2-7 degradation, matching the primary
  path's independent load sites)

#### Scenario: Default construction never panics on CA-less platforms

- **GIVEN** a forced-fallback environment
- **WHEN** `HttpComponent::new()` runs (default config, no TLS
  material)
- **THEN** construction completes without panic and the component
  serves endpoints on the material-free webpki fallback

#### Scenario: Strict error folding surfaces at endpoint creation without panic

- **GIVEN** a forced-fallback environment
- **WHEN** `HttpComponent::with_config()` runs with strict-on
  configuration whose material fails to fold
- **THEN** construction completes without panic and
  `create_endpoint` returns the folded `EndpointCreationFailed`
  error instead of exposing an endpoint

### Requirement: CA-less fallback honors disabled verification

When the webpki fallback triggers and verification is disabled
(`tls.insecure=true` or `tls.verify_peer=false`), the fallback client
SHALL disable certificate verification — parity with the primary
path's `danger_accept_invalid_certs` — instead of silently re-enabling
verification against Mozilla roots.

#### Scenario: Insecure client handshakes with self-signed server in forced fallback

- **GIVEN** a forced-fallback environment, a local TLS server with a
  self-signed certificate, and `tls` with `enabled=true`,
  `insecure=true`
- **WHEN** `build_client` builds the client and it requests `https://`
  from that server
- **THEN** the TLS handshake succeeds and the response is received

#### Scenario: verify_peer=false parity in forced fallback

- **GIVEN** a forced-fallback environment, a local TLS server with a
  self-signed certificate, and `tls` with `enabled=true`,
  `verify_peer=false`
- **WHEN** `build_client` builds the client and it requests `https://`
  from that server
- **THEN** the TLS handshake succeeds and the response is received

### Requirement: Fallback rebuild failure fails closed under strict TLS

When the webpki fallback triggers and the fallback client REBUILD
fails (the second build error), the system SHALL apply the strict
policy: with `tls.strict=true`, `build_client` SHALL return a typed
`CamelError::EndpointCreationFailed` whose message carries the
`tls.strict/webpki-fallback:` prefix and a rebuild-specific fragment
naming the fallback rebuild terminal (distinguishing it from every
material-loading error) — it SHALL NOT return a
material-free emergency client, so configured custom CA, mTLS
identity, and disabled-verification settings cannot silently drop.
Component construction SHALL stay no-panic: the typed error SHALL fold
into `strict_tls_error` and surface at endpoint creation. Without
strict mode, the rebuild failure SHALL keep the permissive degrade: a
loud error log plus the material-free emergency client returned as
`Ok`. Tests SHALL exercise this terminal through a scoped RAII guard
that arms both the forced-fallback and forced-rebuild-failure
thread-local seams, rejects nesting, resets both flags in `Drop`
(panic-safe), and verifies the fallback counter delta — never through
raw flag mutation.

#### Scenario: Strict rebuild failure returns typed error with material configured

- **GIVEN** both seams armed via the scoped guard, and `tls` with
  `enabled=true`, `strict=true`, a VALID `ca_cert_path` (PEM that
  parses and is accepted by the root store) and a VALID matching
  `client_cert_path`/`client_key_path` pair — so no material-loading
  error can precede the rebuild terminal
- **WHEN** `build_client` is called
- **THEN** it returns `Err(EndpointCreationFailed)` whose message
  contains `tls.strict/webpki-fallback:` AND the rebuild-specific
  fragment (under the forced seam that fragment carries the
  forced-failure sentinel, distinguishing this terminal from every
  material-loading error), the fallback counter delta is exactly one,
  and no emergency client is returned

#### Scenario: Strict rebuild failure surfaces at endpoint creation without panic

- **GIVEN** the same armed guard and valid strict configuration
- **WHEN** the HTTP component is constructed and an endpoint is
  created from it
- **THEN** construction completes without panic and endpoint creation
  returns the `tls.strict/webpki-fallback:` rebuild-failure typed
  error (folded through `strict_tls_error`)

#### Scenario: Non-strict rebuild failure degrades to the emergency client

- **GIVEN** both seams armed via the scoped guard, `tls` with
  `enabled=true`, `strict=false`, and a VALID `ca_cert_path` whose
  PEM signs a local TLS server's certificate
- **WHEN** `build_client` is called and the returned client requests
  `https://` from that server
- **THEN** `build_client` returns `Ok` with the material-free
  emergency (webpki-rooted) client, the rebuild terminal emits its
  exact system-broken error log, and the TLS handshake against the
  custom-CA server FAILS — proving the returned client dropped the
  custom trust (material-free by construction, not incidentally)

### Requirement: Duplicate HTTP Consumer registration fails closed

For each shared listener, legacy HTTP Consumers SHALL have unique path keys and method-aware HTTP Consumers SHALL have unique method and parsed path-template keys. Registration SHALL reject an occupied key atomically with CamelError::Config without replacing its sender. A rejected Consumer SHALL fail before signaling readiness and SHALL NOT unregister another Consumer. Distinct methods SHALL coexist, and completed orderly cancellation SHALL free the key for another registration. Existing co-hosting precedence and template ambiguity handling SHALL remain unchanged.

#### Scenario: Duplicate legacy path preserves the original Consumer

- **GIVEN** a ready Consumer without httpMethod on /duplicate
- **WHEN** another Consumer starts on the same listener and path
- **THEN** startup returns a configuration error before readiness, and cancelling the rejected Consumer leaves requests routed to the original Consumer

#### Scenario: Duplicate method and template preserves the original Consumer

- **GIVEN** a ready GET Consumer on /users/{id}
- **WHEN** another GET Consumer starts on the same listener and parsed template
- **THEN** startup returns a configuration error before readiness and the original remains registered

#### Scenario: Concurrent registration has exactly one winner

- **GIVEN** an unoccupied registry key
- **WHEN** two distinct senders register that key concurrently
- **THEN** exactly one registration succeeds and the stored sender belongs to that winner

#### Scenario: Sibling methods survive cancellation

- **GIVEN** GET and POST Consumers share /users
- **WHEN** GET is cancelled and its start loop exits
- **THEN** POST still dispatches and a replacement GET can register

#### Scenario: Orderly restart reclaims a key

- **GIVEN** a registered Consumer
- **WHEN** it is cancelled and its start loop completes, then a new Consumer starts with the same key
- **THEN** the replacement becomes ready and receives requests

### Requirement: Response-side marshal failures are server errors

The HTTP error finalizer SHALL map `CamelError::ProcessorErrorWithSource` to
status 500 with the fixed body `Internal Server Error` and no error detail in
the reply. The REST lowering raises that variant for a failed response-encode
step (see `rest-dsl`, response-side marshal failures), so a server-side
marshalling defect is never reported as a client error. The finalizer SHALL
map `CamelError::TypeConversionFailed` to status 500 with the
`conversion_failed` JSON body (ADR-0096 fault blame): a conversion failure
that the REST binding seam has not reclassified request-borne is a server or
upstream fault, and the injected response step no longer raises
`TypeConversionFailed`.

#### Scenario: processor error with source is a server error

- **GIVEN** `pipeline_error_to_reply` receives
  `CamelError::ProcessorErrorWithSource("response marshal failed (xml)", source)`
- **WHEN** it maps the error to an HTTP reply
- **THEN** the reply has status 500 and body `Internal Server Error`, and the
  reply body contains none of the message or source text

### Requirement: Per-host pinned expired-certificate allowlist

The system SHALL tolerate a server certificate whose verification fails
with an expiry error (`CertificateError::Expired` or
`CertificateError::ExpiredContext`) if and only if an allowlist is
configured AND the presented end-entity certificate's SHA-256 fingerprint
matches the allowlist entry for the exact server name, AND an independent
hostname verification of the presented certificate against the server name
passes. Every other verification error SHALL propagate unchanged even for
a listed host. Enforcement operates on host + fingerprint; the configured
`port` is declarative (validation/dedup) because the rustls verifier API
carries no port. When the allowlist is active, the client SHALL be built on
the preconfigured rustls backend with a wrapper verifier that delegates
non-expiry verification to the default webpki verifier over the bundled
Mozilla roots plus any configured custom CA; platform OS trust settings do
not apply on this path.

#### Scenario: Default unchanged when allowlist is unset

- **GIVEN** a TLS server presenting an expired certificate and a client
  config with `tls.enabled=true` and NO `expired_allowlist`
- **WHEN** the client handshakes with the server
- **THEN** the handshake fails with the existing expired-certificate
  rejection (behavior identical to today)

#### Scenario: Expired certificate on listed host with matching fingerprint is accepted

- **GIVEN** a TLS server presenting an expired end-entity certificate with
  SHA-256 fingerprint F and SAN covering the connection host, and an
  allowlist containing exactly that host and fingerprint F
- **WHEN** the client handshakes with the server
- **THEN** the handshake succeeds and the request completes

#### Scenario: Expired certificate on unlisted host or port is rejected

- **GIVEN** a TLS server presenting an expired certificate and an allowlist
  that does not contain the connection host
- **WHEN** the client handshakes with the server
- **THEN** the handshake fails with the existing TLS/connect error class

#### Scenario: Wrong fingerprint on a listed host is rejected

- **GIVEN** a TLS server presenting an expired certificate whose SHA-256
  fingerprint differs from the fingerprint pinned for that host
- **WHEN** the client handshakes with the server
- **THEN** the handshake fails even though the host is listed

#### Scenario: Hostname mismatch fails even when the certificate fingerprint is listed

- **GIVEN** a TLS server presenting an expired certificate whose SANs cover
  only a DNS name (not the IP address the client connects to), and an
  allowlist entry pinning the connection host and that certificate's
  fingerprint
- **WHEN** the client handshakes with the server
- **THEN** the handshake fails: the wrapper independently verifies hostname
  before accepting an expiry error

#### Scenario: Non-expiry errors still fail on a listed host

- **GIVEN** a TLS server presenting an UNEXPIRED self-signed certificate
  (unknown issuer) and an allowlist entry for that host and certificate
- **WHEN** the client handshakes with the server
- **THEN** the handshake fails with the unknown-issuer error (only expiry
  errors are tolerated)

#### Scenario: Valid certificates unaffected by a configured allowlist

- **GIVEN** a TLS server presenting a valid, non-expired certificate
  chaining to a configured `tls.ca_cert_path` CA, and a non-empty allowlist
- **WHEN** the client handshakes with the server
- **THEN** the handshake succeeds through the default verification path
  (bundled Mozilla roots plus the configured CA — no expiry tolerance
  involved)

#### Scenario: Certificate rooted only in the OS trust store fails on the allowlist path

- **GIVEN** a TLS server presenting a valid certificate that chains only to
  a root present in the platform OS trust store (not in the bundled Mozilla
  set nor the configured `tls.ca_cert_path`), and a non-empty allowlist
- **WHEN** the client handshakes with the server
- **THEN** the handshake fails with the unknown-issuer error — the
  allowlist path uses the bundled Mozilla roots plus the configured CA and
  excludes platform OS trust settings (documented divergence; operators add
  such roots via `tls.ca_cert_path`)

#### Scenario: Empty allowlist list behaves as no allowlist

- **GIVEN** `expired_allowlist: []` (present but empty) with TLS enabled
  and verification on
- **WHEN** an endpoint is created and the client handshakes with a server
  presenting an expired certificate
- **THEN** endpoint creation succeeds and the handshake fails exactly as
  with no allowlist configured (default behavior unchanged)

#### Scenario: Malformed allowlist entry fails endpoint creation

- **GIVEN** an `expired_allowlist` entry with a missing `sha256:` prefix,
  a non-64-hex fingerprint, a wildcard host, a host containing whitespace,
  or port 0 (a port above 65535 is rejected at deserialization by the
  `u16` field type — endpoint creation fails either way)
- **WHEN** an endpoint is created from the config
- **THEN** endpoint creation fails (`CamelError::Config` for validation
  catches, a deserialization error for the above-range port); no entry is
  silently skipped

#### Scenario: Duplicate allowlist entries fail endpoint creation

- **GIVEN** an `expired_allowlist` containing two identical normalized
  entries (same host, port, fingerprint)
- **WHEN** an endpoint is created from the config
- **THEN** endpoint creation fails with `CamelError::Config`

#### Scenario: Allowlist conflicts with disabled verification fail endpoint creation

- **GIVEN** a non-empty `expired_allowlist` together with `tls.insecure=true`
  or `tls.verify_peer=false`
- **WHEN** an endpoint is created from the config
- **THEN** endpoint creation fails with `CamelError::Config`

#### Scenario: Allowlist with TLS disabled fails endpoint creation

- **GIVEN** a non-empty `expired_allowlist` with `tls.enabled=false`
- **WHEN** an endpoint is created from the config
- **THEN** endpoint creation fails with `CamelError::Config`

#### Scenario: Exact host match only

- **GIVEN** an allowlist entry for DNS host `backend.internal`
- **WHEN** the client connects to a server presenting the pinned
  certificate under a different name
- **THEN** the allowlist does not match (no wildcard or subdomain
  matching); an IP-literal connection matches only an IP-literal entry

#### Scenario: Acceptance logging is rate-limited per host and fingerprint

- **GIVEN** an allowlist acceptance for an expired certificate
- **WHEN** multiple connections within the rate-limit window are accepted
  for the same host and fingerprint
- **THEN** at most one WARN carrying the host and the `sha256:<hex>`
  fingerprint is emitted per window; the log contains no secret material

#### Scenario: Expired intermediate with pinned leaf is tolerated

- **GIVEN** a TLS server presenting the pinned, currently-valid-at-a-glance
  end-entity certificate whose chain contains an expired intermediate, and
  the matching allowlist entry
- **WHEN** the client handshakes with the server
- **THEN** the handshake succeeds — tolerance applies to expiry errors on
  the presented path while the pinned leaf is presented (documented
  semantics)

#### Scenario: Allowlist does not leak across components

- **GIVEN** two components constructed from different configs, one with an
  allowlist and one without, each presenting the same expired certificate
  server
- **WHEN** both clients handshake with the server
- **THEN** the allowlisted client succeeds and the non-allowlisted client
  fails — the allowlist is baked into the built client per component

### Requirement: business error HTTP mapping with safe closure

The HTTP error finalizer `pipeline_error_to_reply` SHALL accept an optional
error mapping and SHALL map `CamelError::Business` errors through it. The
mapping type `HttpErrorMapping { codes: <code-to-status>, kinds:
<kind-to-status> }` SHALL live in camel-api and ride the ROUTE as a separate
route-level field (`RouteDslRoute.http_error_mapping` in the DSL AST, carried
through the route translation into the compiled route), NOT as a field of
`ErrorHandlerConfig` — the mapping is orthogonal to the error handler, which
stays untouched (a route may carry a dead-letter channel and a mapping at the
same time). The DSL AST field SHALL be `#[serde(skip_deserializing)]`,
schema-skipped, and `#[ts(skip)]`: plain-route YAML authoring is deferred to the phase-3
`errors:` catalog, and the rest block's `error_mapping` is the only
document-level authoring path in this scope. Table keys SHALL be exact,
non-blank, case-sensitive strings; values SHALL be client/server error
statuses (400-599). Load-time validation SHALL reject, with a typed error:
blank keys and every value outside 400-599 — including informational,
success, and redirect statuses such as 101 and 304. Because programmatic
construction can bypass document validation, the finalizer SHALL also defend:
a mapping entry whose value falls outside 400-599 SHALL be treated as absent
at reply time, and normal status resolution SHALL resume — the remaining
tables, then the built-in default kind map, then 500. Carriage: the
route-to-consumer seam is trusted registration configuration — the route's
mapping travels as an immutable snapshot on the consumer's registration
context (`ConsumerContext`), captured per registration (start and resume) and
passed to `pipeline_error_to_reply` per invocation; no global or ambient
lookup SHALL be used. Status resolution precedence: explicit code map, then
explicit kind map, then the built-in default kind map (`invalid-argument` 400,
`conflict` 409, `not-found` 404, `forbidden` 403, `failed` 500 — kebab-case,
matching the existing error kind strings), then 500. A business error SHALL
NEVER produce a 2xx reply. The reply body for a business error SHALL be
exactly `{"error": <code>, "kind": <kind>}` as JSON; the `message` and `data`
payload SHALL NOT appear in the reply body (ADR-0032: runtime exchange data is
untrusted; `code` and `kind` are operator-authored). No mapping configuration
authorizes `message` or `data` disclosure. Non-business errors keep their
existing variant mappings unchanged. A route without a mapping still maps
business errors through the default kind map and the 500 closure.

#### Scenario: code mapping wins over kind mapping

- **GIVEN** a mapping `{codes: {"ORDER_NOT_FOUND": 404}, kinds: {"not-found": 410}}`
- **WHEN** `pipeline_error_to_reply` receives `Business { code: "ORDER_NOT_FOUND", kind: "not-found", .. }`
- **THEN** the reply has status 404 and body `{"error": "ORDER_NOT_FOUND", "kind": "not-found"}`

#### Scenario: kind mapping applies when no code matches

- **GIVEN** a mapping `{kinds: {"conflict": 409}}`
- **WHEN** the finalizer receives `Business { code: "STALE", kind: "conflict", .. }`
- **THEN** the reply has status 409

#### Scenario: built-in default kind map applies without any explicit mapping

- **GIVEN** no explicit mapping is configured
- **WHEN** the finalizer receives `Business { kind: "invalid-argument", .. }`
  and, in a second request, `Business { kind: "forbidden", .. }`
- **THEN** the replies have status 400 and 403 respectively

#### Scenario: unmapped kind falls back to 500 and never 2xx

- **GIVEN** no explicit mapping is configured
- **WHEN** the finalizer receives `Business { kind: "custom-kind", .. }`
- **THEN** the reply has status 500 and the body is `{"error": <code>, "kind": "custom-kind"}`

#### Scenario: mapped 2xx, 1xx, 3xx, and out-of-range statuses fail at load time

- **GIVEN** mappings containing `{"not-found": 200}`, `{"not-found": 101}`,
  `{"not-found": 304}`, and `{"not-found": 700}` in four route documents
- **WHEN** each route loads
- **THEN** loading fails with a typed error rejecting the non-400-599 value

#### Scenario: blank table keys fail at load time

- **GIVEN** a mapping containing a blank code key or a blank kind key
- **WHEN** the route loads
- **THEN** loading fails with a typed error naming the blank key

#### Scenario: programmatic out-of-domain entries are treated as absent and normal precedence resumes

- **GIVEN** a mapping constructed programmatically (bypassing document
  validation) with an invalid code entry and an invalid `not-found` kind entry,
  plus a valid explicit kind entry for a second error
- **WHEN** the finalizer receives a business error whose code entry is invalid
  but whose kind has a valid explicit entry
- **THEN** the reply uses the valid kind mapping's status
- **WHEN** the finalizer receives a business error of kind `not-found` whose
  kind entry is invalid (204)
- **THEN** the invalid entry is absent, the built-in default kind map decides,
  and the reply is 404
- **WHEN** the finalizer receives a business error of an unknown kind whose
  kind entry is invalid
- **THEN** the reply is 500

#### Scenario: message and data never reach the reply body

- **GIVEN** a business error whose `message` embeds exchange data and whose
  `data` carries structured fields
- **WHEN** the finalizer produces any reply for it
- **THEN** the body contains only the `error` (code) and `kind` fields, and
  neither the message text nor any data field appears in the body

#### Scenario: mapping coexists with a dead-letter error handler

- **GIVEN** a route with `error_handler` carrying a dead-letter channel AND a
  route-level mapping (`kinds: {"not-found": 404}`)
- **WHEN** a business error propagates to the transport
- **THEN** the reply maps through the mapping (404) even though the error
  handler is configured, because the mapping is a separate route-level field
  and business errors bypass default dead-letter dispatch

