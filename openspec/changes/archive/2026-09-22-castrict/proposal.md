# Proposal: castrict

## Why

bd rc-hl9cn (P0, retro520 finding). The CA-less-platform webpki fallback
(commit 967bba1c, rc-3j4mq) silently violates `tls.strict`. When the
platform-verifier client build fails (Android/Termux: no system CA
store), `webpki_fallback_client` swaps the whole TLS backend via
`tls_backend_preconfigured(webpki_root_client_config())`. The
preconfigured backend IGNORES the per-builder TLS material, so a
configured custom CA (`tls.ca_cert_path`) and mTLS identity
(`tls.client_cert_path`/`tls.client_key_path`) are dropped even when
`tls.strict=true`. `strict_tls_error` only validates that the material
parses — it cannot see that the effective client discards it. Strict
mode promised "no silent downgrade path" (rc-ayrwk); on CA-less
platforms that promise is broken: a server requiring the configured
identity or custom-CA trust fails the handshake, and a server trusted
by Mozilla roots (but NOT by the operator's CA) is silently trusted
instead — a trust-posture change the operator never asked for.

## What Changes

**In scope** (`crates/components/camel-http`):

- When the webpki fallback triggers AND `tls.enabled && tls.strict` AND
  custom CA and/or mTLS material is configured, the fallback rustls
  `ClientConfig` SHALL be built FROM the configured material:
  custom CA merged INTO the Mozilla root store (trust-union parity
  with the primary path's `add_root_certificate` semantics — platform
  stand-in + custom), mTLS cert/key becomes client auth. Material-free
  strict fallback keeps Mozilla roots alone (today's behavior).
- Every material-loading or rustls-configuration failure in that
  build (unreadable file, zero parseable PEM sections, zero roots
  added by `RootCertStore`, unparseable private key,
  `with_client_auth_cert` rejection) → typed
  `CamelError::EndpointCreationFailed` naming the conflict — fail
  closed, never a silent Mozilla-roots degrade.
- The fallback honors `tls.insecure`/`tls.verify_peer=false` parity:
  when verification is disabled the fallback config uses a
  no-verify `ServerCertVerifier` (mirroring the primary path's
  `danger_accept_invalid_certs`), instead of silently re-enabling
  verification against Mozilla roots.
- `build_client` becomes `Result`-returning; component constructors
  fold the error into the existing `strict_tls_error` field (endpoint
  creation fails; the never-panic rc-3j4mq contract is preserved).
  The DNS-pinned build path (`PinnedClientCache::get_or_build`) gets
  the same fallible treatment; the producer request path surfaces the
  error instead of building a degraded client.
- Hermetic tests for both paths: forced-fallback (existing
  `SSL_CERT_FILE`/`SSL_CERT_DIR` env technique, CA-store mutex)
  loopback TLS handshakes proving material carried (CA trust + mTLS
  identity) and fail-closed (typed error, no client exposed).

**Out of scope:** non-strict FAILURE behavior (documented permissive
F2-7 degradation on material errors, back-compat, unchanged — though
valid non-strict material is now carried instead of discarded);
server-side TLS; other components (redis/mqtt precedents untouched);
primary-path strict validation (already correct, rc-ayrwk).

## Acceptance criteria

- Forced fallback + strict + valid custom CA → client completes a
  TLS handshake with a server certified by that CA; the same request
  WITHOUT the CA configured fails (material is causally carried).
  The assembled root store is assertably a UNION: Mozilla webpki
  anchors plus the custom root (inspectable seam), not exclusive
  custom trust.
- Forced fallback + strict + valid mTLS pair → handshake succeeds
  against a server REQUIRING that client identity; without the pair
  the server rejects the handshake.
- Forced fallback + strict + material in ANY rejection class —
  unreadable path, zero PEM CERTIFICATE sections, zero roots accepted
  by the root store, unparseable private key, PEM-valid key rejected
  at client-auth configuration (no usable key material) — →
  `build_client` returns `EndpointCreationFailed` naming the
  conflict; no client is exposed.
- Forced fallback with verification disabled (`insecure=true` or
  `verify_peer=false`) → handshake succeeds against a self-signed
  server (primary-path `danger_accept_invalid_certs` parity), strict
  or not.
- Material-free CA-less fallback behaves exactly as today: webpki
  roots, client builds. Valid configured material is carried in the
  fallback regardless of strict; the configured-material warn now
  fires only when non-strict material FAILS to load (permissive F2-7
  downgrade retained) — previously valid non-strict material was
  silently discarded with that warn (behavior improvement, documented
  delta).
- Startup never panics on CA-less platforms (rc-3j4mq regression
  tests stay green).

## Risk budget

Acceptable: signature churn inside camel-http (`build_client`,
`get_or_build`) — internal `pub(crate)`, no public API break. Behavior
deltas: (a) strict mode (opt-in, rc-ayrwk) gains enforced material
carriage and typed fail-closed errors; (b) the fallback gains
verification parity for `insecure`/`verify_peer=false`; (c) VALID
configured material is now carried into the fallback (strict or not)
instead of silently discarded. Non-strict FAILURE semantics are
unchanged: item-wise permissive F2-7 downgrade (valid items carried,
failed items warn-dropped). Out of bounds: changing non-strict
failure semantics beyond that retained downgrade; new runtime
dependencies; panics on CA-less platforms; weakening the primary
path; redefining the meaning of `tls.strict` beyond material
fail-closed (rc-ayrwk semantics).

Bd: rc-hl9cn
