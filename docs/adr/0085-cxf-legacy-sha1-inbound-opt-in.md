# ADR-0085: Legacy SHA-1 inbound opt-in (per-profile algorithm allow-list)

- Status: Accepted (decided 2026-10-19; bd rc-cln4q)
- Source: bd rc-cln4q (P2 bug); openspec change `cxf-legacy-sha1`
- Cross-refs: ADR-0033 (fail-closed security defaults), ADR-0078 (accepted
  residual limitations)

## Context

An external partner, camel-112 / GEMMA by Atos, operates legacy 112
emergency buses. The partner runs the PUBLISHED native
`cxf-bridge-v0.8.2` artifact. Its partners sign inbound SOAP messages with
`http://www.w3.org/2000/09/xmldsig#rsa-sha1` (digest `sha1`, exclusive
c14n). Every inbound message fails with:

```
It is forbidden to use algorithm http://www.w3.org/2000/09/xmldsig#rsa-sha1 when secure validation is enabled
```

The partner reports HTTP 500. Outbound signing with rsa-sha1 and sha1 works.

Root cause: WSS4J enables secure validation unconditionally. The registered
XML-signature provider decides acceptance. The bridge registers Santuario's
`ApacheXMLDSig` at runtime. GraalVM native images cannot register security
providers at runtime, so the published native binary falls back to the JDK
provider. The JDK secure-validation policy forbids `#sha1` and `#rsa-sha1`.
Both inbound paths are affected: the consumer engine path
(`WssSecurityProcessor.processInbound`) and the producer in-interceptor
path. Neither path read algorithm configuration before this change.

The bridge README advertises legacy rsa-sha1 interop. That statement held
only for outbound.

**Probe evidence (2026-10-19, session-local scratch
`/home/shared/tmp/370/probe-evidence.xml`).** Four probes resolved the open
question:

1. With the provider registered, the pinned engine verifies rsa-sha1+sha1.
2. With the provider absent (native-equivalent), the default engine
   reproduces the partner's exact error string.
3. With the provider absent, the pinned engine verifies rsa-sha1+sha1.
   **The pin alone is sufficient.** Santuario's own secure validation does
   not forbid SHA-1.
4. With the pin present and a suite that excludes SHA-1, the engine rejects.
   Suite scoping works.

## Decision

**Per-profile inbound allow-list, SHA-1 off by default.** Two profile
settings form a replacement allow-list:

| Axis | Environment variable | Property | Rust option |
|------|----------------------|----------|-------------|
| signature | `CXF_PROFILE_<NAME>_ALLOWED_SIGNATURE_ALGORITHMS` | `cxf.security.allowed.signature.algorithms` | `allowed_signature_algorithms` |
| digest | `CXF_PROFILE_<NAME>_ALLOWED_DIGEST_ALGORITHMS` | `cxf.security.allowed.digest.algorithms` | `allowed_digest_algorithms` |

Values are comma-separated absolute algorithm URIs. The defaults are six
modern URIs:

- Signature: `http://www.w3.org/2001/04/xmldsig-more#rsa-sha256`,
  `http://www.w3.org/2001/04/xmldsig-more#rsa-sha384`,
  `http://www.w3.org/2001/04/xmldsig-more#rsa-sha512`.
- Digest: `http://www.w3.org/2001/04/xmlenc#sha256`,
  `http://www.w3.org/2001/04/xmldsig-more#sha384`,
  `http://www.w3.org/2001/04/xmlenc#sha512`.

The lists have replacement semantics. A set value replaces the default; it
does not add to it. Operators who need legacy URIs must list every URI they
accept. The lists are exempt from the outbound signing preconditions, so a
verify-only profile can set them. Construction fails when a set value is
empty, contains a blank segment, contains a non-absolute URI, or the
profile has neither a truststore nor a keystore. An empty list is rejected
because WSS4J skips suite checks on empty sets, which would allow every
algorithm.

**Provider pinning, unconditional on both inbound paths.** Both paths
install `SignatureProcessor(new XMLDSigRI())`:

- Consumer engine path: `WSSConfig.setProcessor(WSConstants.SIGNATURE, ...)`
  on the `WSSecurityEngine` built through the existing `engineFactory` seam.
- Producer in-interceptor path: the same processor through CXF's
  `wss4j.processor.map` (`WSS4JInInterceptor.PROCESSOR_MAP`).

The pin is part of the shared `ProfileSignatureProcessor` wrapper. It is
always on. It is deterministic on JVM and native, and it calls no
`Security.addProvider`. The JDK secure-validation policy is not changed.

**Suite scoping.** The same wrapper installs the profile's `AlgorithmSuite`
on `RequestData`. The suite contains only the configured signature methods
and digest algorithms. Every key-length bound is set to a non-rejecting
value (minimum 0, maximum `Integer.MAX_VALUE`); the derived-key-length knobs
are left at their defaults, which do not reject.
WSS4J shares one suite across the signature, encrypted-key, reference-list,
and encrypted-data processors, so default bounds would make acceptance
depend on security-header element order and would reject long partner keys.
The suite therefore constrains signature-method and digest URIs only.

The producer path needs a second hook. `WSHandler.decodeAlgorithmSuite`
mechanically seeds an empty, permissive suite on the CXF path when none is
preset. A null-only install in the wrapper never fires there. The producer
in-interceptor is therefore a `ProfileInInterceptor extends
WSS4JInInterceptor` that overrides `setAlgorithmSuites` and installs the
same `inboundAlgorithmSuite()` instance after `super`. One allow-list
source keeps the two paths from drifting.

**Startup WARNs.** The bridge logs two startup warnings:

1. When an effective list contains a SHA-1 URI in the detected set
   (`xmldsig#rsa-sha1`, `xmldsig#dsa-sha1`, `xmldsig#hmac-sha1`,
   `xmldsig-more#ecdsa-sha1`, `xmldsig#sha1`). The message names the profile
   and records that inbound SHA-1 acceptance is an accepted risk under this
   ADR.
2. When a profile has inbound verification material, an explicit signing
   knob names a SHA-1 URI, and that URI is not on the effective inbound
   allow-list. The message names the profile and URI, points at the two
   `ALLOWED_*` knobs, and states that the asymmetry is intentional if it
   was chosen deliberately.

**Outbound is unchanged.** The explicit signing knobs
(`signature_algorithm`, `signature_digest_algorithm`) keep their
outbound-only meaning. The WSS4J outbound default remains rsa-sha1 plus
sha1. The signing knobs are not an inbound opt-in.

## Accepted risks

1. **SHA-1 collision weakness on opted-in profiles only.** A profile that
   lists SHA-1 URIs accepts signatures with a broken hash. Profiles that
   keep the defaults do not. This is the requested partner interop.
2. **Rejection outside the six modern URIs is a breaking change on JVM
   too.** The default rejects the SHA-1 family and everything outside the
   six defaults, including ecdsa-sha256/384/512, RSA-PSS variants, HMAC
   signatures, and sha224 digests. Native already rejected the SHA-1
   family before this change. JVM previously accepted more algorithms
   through the runtime-registered Santuario provider; it now matches
   native.
3. **Encrypt-only consumers are affected.** The consumer path verifies
   every signature present in the security header regardless of the
   required in-actions. An `Encrypt`-only profile that receives signed
   messages therefore hits the default list and must opt in to keep
   accepting legacy signers.
4. **Keystore-only profiles are consumer-path-only.** The producer
   in-interceptor needs a truststore for the Signature action. Without a
   truststore it has no Signature action (and is null when no other action
   applies), so the opt-in applies only to the consumer path for that
   configuration.
5. **The Rust URI check is slightly stricter than Java's `isAbsolute`.** The
   Rust config validation requires `://` in every segment. Java remains the
   authoritative validator and uses `URI.create(value).isAbsolute()`. The
   Rust check therefore rejects some opaque absolute URIs that Java would
   accept. Algorithm URIs are hierarchical by definition, so the difference
   is accepted.
6. **Suite-level key-length floors are lifted on every inbound-verifying
   profile.** `SecurityProfile.inboundAlgorithmSuite()` sets the minimum
   symmetric, asymmetric, and EC key lengths to 0 and the maxima to
   `Integer.MAX_VALUE`, and both inbound paths install that suite
   unconditionally. WSS4J's default suite-level key-length bounds (symmetric
   128-256, asymmetric 1024-4096, EC 160-512) therefore no longer reject
   inbound keys, including on profiles that keep the default algorithm
   lists. The lift is a deliberate order-independence measure (WSS4J shares
   one suite across processors) and does not affect provider-level
   certificate validation. See the Consequences note for details.
7. **BSP R5404 (exclusive canonicalization) is not enforced.** The allow-list
   scopes signature and digest algorithms only; the bridge does not constrain
   the `CanonicalizationMethod` and verifies whatever the signer used. The
   partner shape already uses exclusive c14n, so this is not a compatibility
   need for the current integration. Enforcing R5404 on all inbound traffic
   would be a separate policy decision, not part of this opt-in.

## Rejected alternatives

- **Weaken `jdk.xml.dsig.secureValidationPolicy` globally.** Rejected: it
  weakens every profile and every deployment to serve one partner.
- **Add a global `secure_validation=false` knob.** Rejected by design
  review: one switch would disable the check process-wide, including for
  profiles that never opted in. It violates ADR-0033.
- **Treat an explicit legacy signing knob as the inbound opt-in.**
  Rejected: it conflates signing with verification, cannot express an
  asymmetric partner (sign SHA-1, accept only SHA-256), and silently widens
  inbound acceptance when an operator changes an outbound knob.
- **JVM-only fix.** Rejected: the shipped path is native. Runtime provider
  registration does not work there, so only explicit pinning is
  deterministic.
- **Wrapper emptiness-overwrite mechanism.** Rejected: letting the shared
  wrapper overwrite a non-null `RequestData` suite would let a later
  processor silently replace the profile policy, and the consumer path's
  null-only contract would become ambiguous. The explicit
  `setAlgorithmSuites` override is narrower and mirrors CXF's own
  `PolicyBasedWSS4JInInterceptor`.

## Consequences

**Breaking change and migration.** Inbound messages outside the effective
allow-list are rejected on JVM and native. Operators of legacy partners set
`CXF_PROFILE_<NAME>_ALLOWED_SIGNATURE_ALGORITHMS` and
`CXF_PROFILE_<NAME>_ALLOWED_DIGEST_ALGORITHMS` to the URIs they accept.
Because replacement semantics apply, the migration value must include the
modern URIs the profile still needs.

**WARN, not INFO.** The SHA-1 acceptance warning is a WARN. An
intentionally asymmetric profile (sign SHA-1 outbound, accept only SHA-256
inbound) is valid and expected to produce the second WARN. That noise is
accepted by design: the WARN states that the asymmetry is intentional when
deliberate, so an operator can tell intent from misconfiguration.

**Silence is not acceptance.** A symmetric default profile sets no
signing knobs. It still signs rsa-sha1 outbound through the WSS4J default,
but it triggers no mismatch WARN because that warning keys off explicit raw
knob values. The absence of a WARN does not mean inbound SHA-1 is
accepted. The default allow-list still rejects it. Partners must not read
silence as consent.

**ADR-0033 preserved.** The default stays fail-closed. The change adds an
explicit, per-profile opt-in and does not change which signature or digest
algorithms a non-opted profile accepts. It does replace WSS4J's default
suite-level key-length bounds (symmetric 128-256, asymmetric 1024-4096, EC
160-512) with non-rejecting values on every inbound-verifying profile,
including default profiles. This is a deliberate order-independence measure,
because WSS4J shares one suite across processors; provider-level certificate
validation is unaffected (see accepted risk 6).

**ADR-0078 unchanged.** This is a new explicit opt-in, not a silent
residual limitation. The ADR-0078 list is not amended.

**Closure gate.** bd rc-cln4q stays open until a native run (CI or the
post-release published-bridge smoke) shows the opted-in profile accepting
rsa-sha1 and the default profile rejecting it.

## References

- bd rc-cln4q (this decision; native verification gates closure)
- openspec change `cxf-legacy-sha1`
- ADR-0033 (security defaults and fail-closed startup validation)
- ADR-0078 (security accepted-residual limitations; list unchanged)
- Probe evidence: `/home/shared/tmp/370/probe-evidence.xml` (session-local
  scratch, not versioned)
- `bridges/cxf/src/main/java/org/rustcamel/cxf/ProfileSignatureProcessor.java`
- `bridges/cxf/src/main/java/org/rustcamel/cxf/SecurityProfile.java`
- `bridges/cxf/src/main/java/org/rustcamel/cxf/SecurityProfileStore.java`
  (WARN emitters)
- `bridges/cxf/src/main/java/org/rustcamel/cxf/WssSecurityProcessor.java`
  (consumer engine path and `engineFactory` seam)
