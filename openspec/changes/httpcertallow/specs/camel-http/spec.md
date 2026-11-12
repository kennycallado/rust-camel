## ADDED Requirements

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
