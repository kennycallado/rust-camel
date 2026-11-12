# Proposal: httpcertallow

## Why

Today the only way to call a peer whose certificate has expired is
`tls.insecure=true` / `verify_peer=false`, which maps to
`danger_accept_invalid_certs(true)` in
`crates/components/camel-http/src/tls.rs` — it disables ALL certificate
verification (chain, signature, hostname, expiry) for every host the client
talks to. An operator who needs to tolerate exactly one expired backend
certificate is forced to accept any MITM certificate for the whole endpoint.
bd rc-ishar (P2, owner request 2026-11-12). Apache Camel has no equivalent
(upstream comparison recorded in rc-ishar); this design must be finer than
both `insecure` and Camel's all-or-nothing TrustManager delegation.

## What Changes

- camel-http gains an opt-in, fail-closed, per-host pinned allowlist for
  expired certificates: `tls.expired_allowlist` (structured component YAML,
  exact `host`, `port`, `sha256:<hex>` end-entity fingerprint). Only the
  `CertificateExpired` verification error is tolerated, and only when the
  server name and the presented DER both match an allowlist entry.
- A wrapped rustls `ServerCertVerifier` delegates every non-expiry error to
  the default webpki verifier and independently re-verifies hostname before
  accepting an expired certificate.
- Affected crates: `camel-component-http` only (tls.rs, config.rs, new
  `tls_allowlist` module, lib.rs build path), plus docs
  (`docs/src/components/http.md`) and changelog.
- Explicitly excluded (non-goals): server-side TLS, cxf (has its own
  pinned-expiry feature), accepting `UnknownIssuer`, global/JVM-wide trust
  overrides, changing `insecure` semantics, a URI-parameter config form.

## Acceptance criteria

- Allowlist unset: expired certificates are rejected exactly as today.
- Expired certificate on a listed host with matching SHA-256 fingerprint:
  handshake succeeds; one rate-limited WARN with host and fingerprint.
- Unlisted host, wrong fingerprint, hostname mismatch, unknown issuer, or
  any other verification error still fails — even on a listed host.
- Valid (non-expired) certificates are unaffected by an allowlist being
  configured.
- Malformed allowlist entries (a present-but-empty list behaves as no
  allowlist), duplicate entries, and the
  insecure/verify_peer-off/TLS-off conflicts fail endpoint creation with a
  `CamelError::Config` (no silent skip, ADR-0032: config never from
  exchange headers).
- The allowlist is baked into the built client (per component config); it
  does not leak across endpoints/components.

## Risk budget

- The allowlist path bypasses reqwest's platform verifier (reqwest 0.13.5
  exposes no custom-verifier hook) and uses the bundled Mozilla root set
  plus any configured `tls.ca_cert_path` — an accepted, documented
  divergence, not a risk to ship silently.
- Acceptable risk: tolerance of an expired INTERMEDIATE when the pinned
  end-entity certificate itself is presented (documented semantics).
- Out of bounds: weakening hostname verification, host-only (fingerprint
  less) allowlist entries, accepting revocation or unknown-issuer errors.
