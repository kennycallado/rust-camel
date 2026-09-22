## ADDED Requirements

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
