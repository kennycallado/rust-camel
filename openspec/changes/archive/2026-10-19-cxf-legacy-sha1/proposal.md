# Proposal: cxf-legacy-sha1

## Why

An external consumer (camel-112 / GEMMA by Atos, legacy 112 emergency buses) runs the
**published native** `cxf-bridge-v0.8.2` artifact. Their partners sign inbound SOAP
messages with `http://www.w3.org/2000/09/xmldsig#rsa-sha1` (digest sha1, exclusive
c14n). Every inbound message is rejected with:
`It is forbidden to use algorithm http://www.w3.org/2000/09/xmldsig#rsa-sha1 when
secure validation is enabled` (HTTP 500). Outbound signing with rsa-sha1+sha1 works.

Root cause (e_opus, session `ses_eab83739fffeAVrbOfxkCE2W5V` — binding): WSS4J enables
secure validation unconditionally; which XML-signature *provider* is registered decides
acceptance. The bridge registers Santuario's `ApacheXMLDSig` (forbids only MD5) at
**runtime**, which GraalVM native cannot do — so the shipped native binary falls back to
the JDK provider, whose `jdk.xml.dsig.secureValidationPolicy` forbids `#sha1` and
`#rsa-sha1`. The failure affects both inbound paths (consumer `processInbound` and
producer response verification); neither reads any algorithm configuration.

The bridge README advertises legacy rsa-sha1 interop (CAT112 example) — it currently
holds only for outbound. This is a correctness bug in the framework's WS-Security
surface, not a project-specific request.

bd: **rc-cln4q** (P2, bug).

## What Changes

- **Pin the signature provider** deterministically in JVM and native via
  `WSSConfig.setProcessor(WSConstants.SIGNATURE, new SignatureProcessor(new XMLDSigRI()))`
  (no `Security` provider registration needed): consumer path through the existing
  `engineFactory` seam, producer in-interceptor through CXF's `wss4j.processor.map`
  (`PROCESSOR_MAP`) property.
- **Per-profile opt-in allow-list, OFF by default**: inbound signature/digest algorithm
  URIs (`CXF_PROFILE_<N>_ALLOWED_SIGNATURE_ALGORITHMS` /
  `cxf.security.allowed.signature.algorithms`, plus digest counterpart), enforced via
  `RequestData.setAlgorithmSuite` with a small `Processor` wrapper. Default allow-list:
  rsa-sha256/384/512 + sha256/384/512 — **SHA-1 rejected by default**.
- **Startup WARN** naming the profile whenever SHA-1 is allowed, plus a WARN pointing operators
  who set a legacy signing knob at the new inbound knobs.
- **Breaking change (declared):** the default allow-list rejects inbound messages signed with
  the SHA-1 family and everything outside the six modern URIs (ecdsa, rsa-pss, hmac, sha224
  families included) — on both JVM and native — including consumers with `Encrypt`-only
  in-actions that receive signed messages; changelog + README + ADR must say so.
- Rust plumbing: `camel-cxf` config option → `CxfProfileEnvVars` → sidecar env.
- Un-work-around the test fixture (`generateLegacySha1Fixture`), fix ~63 inbound Java
  test calls broken by reject-by-default, add LEG C to `wss_e2e.rs`.
- Dead-config-policy spec delta (new inbound scenario + exemption for the new flag),
  new ADR (accepted risk), README + `docs/src/components/ws-soap.md` corrections.
- Post-release smoke job (download published artifact, run e2e subset).

**Explicitly excluded**: disabling `jdk.xml.dsig.secureValidationPolicy` globally
(rejected — weakens every profile); changing the **outbound** default (rsa-sha1+sha1
stays; a legacy partner requires it, `WssSecurityProcessorIntegrationTest.java:611-613`
asserts it); global `secure_validation=false` knob (rejected by design review).

## Acceptance criteria

- Inbound rsa-sha1+sha1 is rejected with a default profile, accepted with the opt-in, on JVM —
  on BOTH inbound paths (consumer `processInbound` and producer in-interceptor); the
  provider-removed test (`inboundSha1WithoutRegisteredProvider`) simulates native and passes.
- The three-reference partner shape (BST + Timestamp + Body, exclusive c14n) verifies on an
  opted-in profile; inbound rsa-sha256 is unchanged under the default profile.
- Outbound default remains rsa-sha1+sha1; `consumerDefaultsUnchangedWithoutKnobs` holds.
- Startup WARN fires when SHA-1 is allowed (naming the profile); config parse/fail-loud
  validated, including empty-list and verify-only-profile exemption cases.
- Breaking change declared (default rejects the SHA-1 family and everything outside the six
  modern URIs) in changelog + README + ADR-0085.
- Published `cxf-bridge-v0.8.2` artifact repro recorded (500 + exact partner log line); fix
  verified against the JVM path. **Native verification gates bd closure**: rc-cln4q stays open
  until CI or the post-release smoke run shows LEG C green on the native binary.
- Outbound coverage configurability finding (partner update #2) reported in the park report —
  no new outbound scope in this mission.

## Risk budget

Acceptable: SHA-1 collision weakness for inbound verification on **opted-in profiles
only** (accepted risk, documented in ADR). Fail-closed default (ADR-0033) preserved.
Out of bounds: weakening non-opted profiles, project-specific hacks, outbound changes.

Affected crates: `camel-cxf` (config/plumbing), `camel-test` (e2e), `xtask` (schema);
Java bridge `bridges/cxf` (sidecar, published artifact).
