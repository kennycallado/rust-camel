# ADR-0087: Expired pinned certificate opt-in (consumer WS-Security)

- Status: Accepted (decided 2026-10-20; bd rc-x7wys)
- Source: bd rc-x7wys (P2 partner interop); openspec change `cxf-pinned-expired-cert`
- Cross-refs: ADR-0033 (fail-closed security defaults), ADR-0078 (accepted
  residual limitations), ADR-0085 (legacy SHA-1 inbound opt-in), ADR-0083
  (artifact signing envelope; unrelated trust surface)

## Context

Partner camel-112 (engine 0.59.0, cxf-bridge 0.8.3) signs with one system
certificate shared by many tenants. Its test PKI pins a self-signed X.509
certificate (subject equals issuer, RSA 2048) that expired on 2024-06-11.
The bridge rejects every envelope signed with that certificate:

```text
The provided certificate is invalid ... NotAfter
```

WSS4J 4.0.1 already pins exactly. In the direct-trust branch
(`certs.length == 1 && !enableRevocation`) it looks up the certificate by
issuer and serial, compares full DER with `X509Certificate.equals`, and only
then calls `checkValidity`. The date is the only blocker. No WSS4J property
or annotation relaxes it. The partner's PHP service verifies with a plain
`openssl_verify` and never checks the date. Expiry adds no security value for
a certificate that the deployment pins by choice.

The published bridge is a native image. A runtime-only workaround is not
available, and a process-wide validity switch would weaken every profile.
The change therefore adds one explicit, per-profile, fail-closed opt-in.

## Decision

**Per-profile opt-in, default off.** Add the boolean
`allow_expired_pinned_certificate`. Default `false`. A profile that does not
set it keeps stock WSS4J behavior. Configuration flows per profile:

`CxfSecurityFields` (Rust) → `CXF_PROFILE_<NAME>_ALLOW_EXPIRED_PINNED_CERTIFICATE`
→ `SecurityProfileStore` → `SecurityProfile.Builder` →
`getVerificationCrypto()`. The store accepts only the literals `true` and
`false`; `null` and blank resolve to `false`, and any other value rejects
startup with an error that names the variable.

**`PinnedTrustMerlin`.** The bridge defines a `Merlin` subclass. It overrides
the protected three-argument `verifyTrust`, calls `super`, and catches only a
`WSSecurityException` whose cause is `CertificateExpiredException`. It accepts
that failure only when all of these hold:

1. The per-profile opt-in is `true`.
2. Exactly one certificate is presented.
3. Revocation is disabled (`enableRevocation == false`).
4. The DER encoding of the presented certificate byte-equals an entry in the
   profile's keystore or truststore.

Every other failure is re-thrown unchanged. The verification `Crypto` is built
from the `keystore.file` property, so pins live in `getKeyStore()` in
practice; the scan therefore checks both stores. A store scan error does not
relax trust: it falls through to the original failure.

**Wiring.** `SecurityProfile.getVerificationCrypto()` instantiates
`PinnedTrustMerlin` inside the existing volatile/synchronized block when the
opt-in is on. Otherwise it calls `CryptoFactory.getInstance(...)` exactly as
before. The class is instantiated with `new`, so no native-image reflection
registration is added and no global state changes.

**Startup fail-closed checks and WARN.** When the opt-in is true,
`SecurityProfile.Builder` validates the profile before the bridge announces
ready:

- The resolved inbound actions must include `Signature`. Blank actions resolve
  through the existing `resolveActionsOrDefault`, which yields `Signature`
  (`Signature` also for keystore-only consumer profiles). Explicit actions
  without `Signature` reject.
- The profile must configure a truststore or a keystore. Without verification
  material there is nothing to pin.
- The bridge logs a startup WARN that names the profile and the environment
  variable, and records the loss of expiry as a key-lifetime bound and of any
  expiry-tied rotation. The WARN cites this ADR.

Both failures name the environment variable. A misconfigured opt-in never
reaches message processing.

**Consumer only.** The opt-in covers the consumer inbound verification path
(`WssSecurityProcessor.processInbound`). The producer `ProfileInInterceptor`
path always uses the stock `CryptoFactory`; a producer response signed with
the pinned expired certificate still fails with `invalidCert`. No TLS trust
handling changes, and `CxfClientManager` is untouched.

## Still rejected

The opt-in relaxes certificate expiry and nothing else. With the opt-in on,
these cases still fail:

- A not-yet-valid certificate (`CertificateNotYetValidException`).
- A chain of more than one certificate, including an expired leaf inside a
  chain.
- An issuer certificate pinned in place of the presented leaf.
- Any verification with revocation enabled. This ADR adds no profile-level
  revocation setting.

## Accepted risks

1. **Expiry as a key-lifetime bound is lost for this exact pinned key.** The
   profile accepts the pinned certificate after its `notAfter` date. Any
   expiry-tied rotation is lost with it: rotating the certificate requires
   updating the pin, not waiting for expiry.
2. **The relaxation is scoped by identity, not by date.** Acceptance requires
   full DER equality with a pinned entry. An expired certificate that is not
   pinned, or that differs in one byte, still fails. Other profiles and other
   keys keep the stock validity check.
3. **The opt-in depends on the WSS4J direct-trust branch shape.** It keys off
   `CertificateExpiredException` from that branch and re-throws everything
   else. A WSS4J upgrade that changes the failure topology can disable the
   opt-in (fail-closed) but cannot widen it. Tests pin the current shape.
4. **The thrown message does not name the algorithm.** This is intentional and
   covered by the error split below. Operators diagnose algorithm rejections
   from the WSS4J WARN log.
5. **ADR-0033 remains the default.** The default is fail-closed. There is no
   environment-wide or process-wide switch. Enabling the opt-in is an explicit
   per-profile operator decision. This is a new opt-in, not a silent residual
   limitation, so the ADR-0078 list is unchanged.

## Distinct from ADR-0083

ADR-0083 pins an **artifact-signing key** by its `blake3:` fingerprint for
artifact verification at boot and through `--verify`. ADR-0087 pins an
**X.509 certificate** by full DER equality inside the WS-Security `Crypto`
store, for inbound SOAP signature trust. The values, the stores, the
verification engines, and the trust surfaces are unrelated. An artifact
truststore entry cannot authorize a SOAP signer, and a WS-Security keystore
entry cannot authorize an artifact.

## Error split and SHA-1 composition

The expiry opt-in and the ADR-0085 SHA-1 allow-list are independent knobs.
The allow-list is evaluated by signature-algorithm validation; this opt-in is
evaluated only by trust validation. They may both be enabled, as they must be
for the partner case (the partner certificate is self-signed with RSA-SHA1 and
expired).

The partner must be able to tell which gate rejected a message:

- **Expiry gate off:** the verifier throws `WSSecurityException` with
  `ErrorCode.FAILED_CHECK`. Its rendered message is
  `The provided certificate is invalid` (the WSS4J `invalidCert` key), and its
  cause is `CertificateExpiredException`.
- **Algorithm gate off:** the verifier throws `WSSecurityException` with the
  generic `ErrorCode.INVALID_SECURITY`. The rejected algorithm URI appears
  only in the WSS4J WARN log (`SignatureMethod ... does not match required
  values`); the thrown message does not include it and must not be parsed for
  it.

The two errors are therefore distinguishable without exposing algorithm
details in the protocol response.

## Rejected alternatives

- **Global `secure_validation` or validity switch.** Rejected: it weakens every
  profile and every deployment to serve one partner, and violates ADR-0033.
- **Override `checkValidity` behavior by property or annotation.** Rejected:
  no such WSS4J setting exists in 4.0.1, and the published native bridge cannot
  rely on runtime provider changes.
- **Override the public four-argument `verifyTrust`.** Rejected: the exception
  is thrown by the protected three-argument method that the public method
  delegates to. The protected method is the correct seam.
- **Truststore-only DER scan.** Rejected: the verification `Crypto` is built
  from `keystore.file`, so the pinned certificate can live in the keystore.
  Both stores are scanned.
- **Extend the opt-in to the producer path.** Rejected: partner traffic is
  inbound. Producer coverage would broaden the relaxation without a
  requirement. The producer keeps stock `CryptoFactory`.
- **Rotate or replace the partner certificate.** Rejected: the partner controls
  the test PKI, and this bridge change is the requested interop.

## Consequences

The default behavior is unchanged. Only an opted-in consumer profile accepts
an expired certificate, and only when that exact certificate is pinned.
Profiles that never set the option keep stock `Merlin` / `CryptoFactory`
behavior on JVM and native.

The option is documented in `docs/src/components/ws-soap.md`. No generated
schema covers `CxfSecurityFields`, so `cargo xtask schema --check` is unaffected. There is no bridge version bump and no release metadata change in
this decision.

**Closure gate.** bd rc-x7wys stays open until the published bridge accepts the
partner's expired pinned certificate with both opt-ins enabled, and rejects it
with the distinguishable `invalidCert` error when the expiry opt-in is off.

## References

- bd rc-x7wys (this decision)
- openspec change `cxf-pinned-expired-cert`
- ADR-0033 (security defaults and fail-closed startup validation)
- ADR-0078 (security accepted-residual limitations; list unchanged)
- ADR-0085 (legacy SHA-1 inbound opt-in)
- ADR-0083 (artifact signing envelope; unrelated trust surface)
- `bridges/cxf/src/main/java/org/rustcamel/cxf/PinnedTrustMerlin.java`
- `bridges/cxf/src/main/java/org/rustcamel/cxf/SecurityProfile.java`
  (`getVerificationCrypto`, Builder validation)
- `bridges/cxf/src/main/java/org/rustcamel/cxf/SecurityProfileStore.java`
  (env parsing and WARN)
- `crates/components/camel-cxf/src/config.rs` (`CxfSecurityFields`)
- `crates/services/camel-bridge/src/process.rs` (env emission)
