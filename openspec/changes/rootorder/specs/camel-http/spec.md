## MODIFIED Requirements

### Requirement: Strict TLS material survives the CA-less fallback

When the platform-verifier client build fails and the webpki fallback
triggers, and `tls.enabled` and `tls.strict` are both true, and custom
CA and/or mTLS material is configured, the effective fallback client
SHALL carry that material: the custom CA SHALL be merged into the
Mozilla webpki root store (trust-union parity with the primary path's
`add_root_certificate` semantics), the custom root SHALL precede the
Mozilla anchors in the merged store order (parity with the primary
path's extra-roots-before-platform-roots order), and the mTLS
cert/key SHALL be the client identity. If any material-loading or
rustls-configuration step fails — unreadable file, zero parseable
PEM CERTIFICATE sections, zero roots accepted by the root store,
unparseable private key, or client-auth configuration rejection —
`build_client` SHALL return `CamelError::EndpointCreationFailed`
naming the conflict. The fallback SHALL NOT silently substitute
Mozilla roots alone for configured strict material.

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

#### Scenario: Custom CA merged as a custom-first trust union

- **GIVEN** a custom CA PEM containing exactly one root certificate
- **WHEN** the fallback root store is assembled with that CA
- **THEN** the store contains the custom root AND the bundled
  Mozilla webpki anchors (union, not replacement), and the custom
  root precedes every Mozilla anchor — the index of the first
  custom root is lower than the index of the first Mozilla anchor
  — matching the primary path's extra-roots-before-platform-roots
  order; an inspectable root-store construction seam makes the
  contents and their order assertable

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
