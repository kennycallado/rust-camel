# Design: cxf-pinned-expired-cert

## Approach

The design below is the e_opus-verified design from bd rc-x7wys (code + javap over WSS4J 4.0.1 / CXF 4.1.8). It is implemented as stated; alternatives were settled at verification time.

Facts the design rests on:
- The WSS4J direct-trust branch runs only when `certs.length == 1 && !enableRevocation`: lookup by issuer+serial, comparison via `X509Certificate.equals` (full DER), then `checkValidity`.
- `invalidCert` is thrown by the **protected 3-arg** `Merlin.verifyTrust(X509Certificate[], boolean, Collection)`. The public 4-arg method calls it.
- The verification `Crypto` is built from the `keystore.file` property, so pinned certificates live in `getKeyStore()`, NOT `getTrustStore()`. Scan both.
- No WSS4J property or annotation allows expired certificates.

**R1 — `PinnedTrustMerlin`** (new class, `bridges/cxf`): extends `Merlin`; overrides the 3-arg `verifyTrust`; calls `super`; catches only a `WSSecurityException` whose cause is `CertificateExpiredException`; accepts ONLY when the per-profile opt-in is on, `certs.length == 1`, revocation is disabled, and `DER(certs[0])` equals an entry in the keystore or the truststore. Any other failure is re-thrown unchanged.

**R2 — Wiring.** Change only `SecurityProfile.getVerificationCrypto()` (~line 540). When the flag is on, instantiate the subclass inside the existing volatile/synchronized block; otherwise keep `CryptoFactory`. No global state.

**R3 — Config.** Per-profile `allow_expired_pinned_certificate`, default `false`. Flow: `CxfSecurityFields` (`crates/components/camel-cxf/src/config.rs:215`) → `crates/services/camel-bridge/src/process.rs` env `CXF_PROFILE_<N>_ALLOW_EXPIRED_PINNED_CERTIFICATE` → `SecurityProfileStore` → `Builder`. Reject at startup if resolved inbound actions lack `Signature` or if no store is configured (mirror the SHA-1 knob check at `SecurityProfile.java:784`). Existing `resolveActionsOrDefault` resolves blank inbound actions to `Signature`, including keystore-only consumer profiles; explicit actions without `Signature` reject. Log a startup WARN like `sha1AllowedWarning`. No revocation setting is added; test revocation directly at the Crypto seam.

**R4 — Scope.** Consumer only. The producer `ProfileInInterceptor` path is NOT covered; documented explicitly.

**R5 — Native image.** The class is instantiated with `new`, not by name, so `NativeImageReflectionRegistrations.java` stays unchanged.

**R6 — Docs and ADR.** `docs/src/components/ws-soap.md` (English only) and ADR-0087 (ADR-0086 is reserved by mission 371). Record as accepted risk: for this exact pinned key, expiry as a key-lifetime bound is lost (and any expiry-tied rotation); not-yet-valid, chains longer than one, issuer-instead-of-leaf, and revocation still reject. Regenerate the schema (`cargo xtask schema`).

**R7 — SHA-1 composition (partner requirement).** The partner's certificate is self-signed with an RSA-SHA1 signature, so the SAME inbound profile will have BOTH opt-ins enabled. They are independent by construction: the SHA-1 knob is an algorithm allow-list in `SecurityProfile`/`ProfileSignatureProcessor`; the expiry check is trust validation in `Merlin.verifyTrust`. A test proves composition: both on ⇒ accepted; SHA-1 off ⇒ rejected with the algorithm error; expiry opt-in off ⇒ rejected with `invalidCert`. The two flags are not coupled.

**R8 — Partner validation.** camel-112 tests the published 0.8.4 with their test PKI. Error messages stay distinguishable so a failure tells them WHICH gate rejected the message (algorithm gate vs `invalidCert` gate).

Expired test certificates are generated with JDK tooling: `keytool -startdate -400d -validity 30`.

## Affected crates

- `crates/components/camel-cxf`: `CxfSecurityFields` gains `allow_expired_pinned_certificate` (+ validation mirror, Debug redaction-free boolean).
- `crates/services/camel-bridge`: `process.rs` emits `CXF_PROFILE_<N>_ALLOW_EXPIRED_PINNED_CERTIFICATE`.
- `bridges/cxf` (Java): `PinnedTrustMerlin`, `SecurityProfile` wiring + Builder validation + WARN, `SecurityProfileStore` env parsing; tests T1–T10.
- `crates/camel-cli`: `feature_profiles` golden only if the env-var closure genuinely changes.
- `docs`, `schemas`: ws-soap.md, ADR-0087, regenerated schema.

## Architecture boundaries

- Data/control plane: the knob is operator configuration (trusted, ADR-0032), consumed at bridge startup validation — fail-closed per ADR-0033 (explicit per-item opt-in, no global disable switch).
- Components boundary: camel-cxf config surface grows one boolean following the `allowed_signature_algorithms` precedent (inbound verification knob, exempt from signing preconditions).
- The Java bridge owns trust validation; Rust only transports configuration. No protocol/proto change (no SignerIdentity impact).
- Services boundary: `camel-bridge` env emission only; no runtime behavior.

## Alternatives considered

Settled during e_opus verification, recorded here for history only:
- A WSS4J property/annotation to relax validity — does not exist (verified across the jars).
- Overriding the public 4-arg `verifyTrust` — rejected: the exception is thrown by the protected 3-arg method the 4-arg delegates to; the 3-arg override is the correct seam.
- Truststore-only DER scan — rejected: the verification Crypto is built from `keystore.file`, so pins live in `getKeyStore()` in practice; both stores are scanned.
- Global/env-wide expiry bypass — rejected (out of scope; per-profile opt-in only).
