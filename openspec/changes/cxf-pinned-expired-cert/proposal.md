# Proposal: cxf-pinned-expired-cert

## Why

Partner camel-112 (engine 0.59.0, cxf-bridge 0.8.3) signs with ONE system certificate shared by many tenants. Their test PKI contains a self-signed X509v3 certificate (subject = issuer, RSA 2048) that expired 2024-06-11, and they pin that exact certificate in the truststore. The bridge rejects the envelope with `The provided certificate is invalid ... NotAfter`, so the partner cannot even run their test environment against us.

WSS4J 4.0.1 already pins exactly: in the direct-trust branch (`certs.length == 1 && !enableRevocation`) it looks up by issuer+serial, compares full DER via `X509Certificate.equals`, then calls `checkValidity`. The date is the only blocker — and there is no WSS4J property or annotation to allow expired certificates (verified across the jars, e_opus).

The partner's legacy PHP service verifies with a plain `openssl_verify` and never looks at the date; expiry adds no security value for a cert they pinned by choice.

## What Changes

- New `PinnedTrustMerlin` (Java, `bridges/cxf`): extends `Merlin`, overrides the protected 3-arg `verifyTrust`; on a `WSSecurityException` caused by `CertificateExpiredException` it accepts ONLY when the per-profile opt-in is on, exactly one certificate is presented, revocation is disabled, and the DER of the presented cert byte-equals an entry in the keystore or the truststore. Every other failure is re-thrown unchanged.
- `SecurityProfile.getVerificationCrypto()` instantiates the subclass inside the existing volatile/synchronized block when the opt-in is on; otherwise `CryptoFactory` as today.
- New per-profile config knob `allow_expired_pinned_certificate` (default `false`), flowing `CxfSecurityFields` (camel-cxf) → `CXF_PROFILE_<N>_ALLOW_EXPIRED_PINNED_CERTIFICATE` (camel-bridge) → `SecurityProfileStore` → `Builder`. Startup fails if inbound actions lack `Signature` or no store is configured (mirrors the SHA-1 knob check). Startup logs a WARN like `sha1AllowedWarning`.
- Consumer path only; the producer `ProfileInInterceptor` path is explicitly NOT covered. No TLS trust handling changes (`CxfClientManager` untouched). No global/env-wide kill switch.
- Docs: `docs/src/components/ws-soap.md` (English) + ADR-0087 (accepted risk recorded); schema regenerated.

Affected crates: `crates/components/camel-cxf`, `crates/services/camel-bridge`, `crates/camel-cli` (feature-profile golden only if the closure changes), `bridges/cxf` (Java), `docs`, `schemas`.

Bd: rc-x7wys.

## Acceptance criteria

- T1–T10 green; `./gradlew --no-daemon spotlessCheck`; full Java suite green.
- `cargo test -p camel-cxf` and `cargo test -p camel-bridge` green under containment.
- `cargo test -p camel-cli --test feature_profiles` green (golden regenerated only if the closure genuinely changed).
- `cargo fmt --check --all`; `cargo clippy -p camel-cxf -p camel-bridge --all-targets -- -D warnings`.
- `openspec validate cxf-pinned-expired-cert --type change` clean; delta on the cxf security capability.
- ADR-0087 written; docs updated; schema regenerated and committed.
- No bridge version bump, no release.

## Risk budget

Accepted risk (ADR-0087): for the exact pinned key, expiry as a key-lifetime bound is lost, and any expiry-tied rotation with it. Still rejected: not-yet-valid certs, chains longer than one, issuer-instead-of-leaf, revocation-enabled paths. Out of bounds: accepting an expired leaf inside a chain, skipping the date check globally or by environment variable, producer path, TLS trust changes.
