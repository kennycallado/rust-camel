# Proposal: cxf-bridge-hardening

## Why
Five open CXF-bridge defects (one P2, four P3) block bridges 0.6.1 —
the fail-loud / bounded classes the 0.6.0 sweep closed on other
paths, plus supply-chain hygiene:

1. **rc-nxtb (P2)**: unknown WS-Security action tokens pass both
   validators silently and materialize nothing — the endpoint runs
   unsecured while the operator believes otherwise (ADR-0033).
2. **rc-wlgy**: producer responses serialize into the sidecar heap
   unbounded; the consumer direction got `CXF_MAX_BODY_BYTES` in 0.6.0,
   the producer direction did not.
3. **rc-q5be**: `extractSoapBody` string-scans the envelope
   (namespace-blind matching mis-splices on names like `xsd:bodyData`);
   the namespace-aware `extractBody` already exists.
4. **rc-urkv**: the Dispatch cache is unbounded; per-call headers
   echoing untrusted data mint one permanent Dispatch per distinct
   value (ADR-0032 exhaustion primitive).
5. **rc-aq7f**: the Shibboleth nexus repo is broader supply-chain
   surface than the build needs.

## What Changes

- **Token whitelist (both validators).** Every token must be in the
  materialized set — inbound {Signature, Encrypt, Timestamp} (Timestamp
  requires Signature, mirroring the outbound composition rule);
  outbound {Signature, Encrypt, Timestamp}. Unknown tokens fail profile
  build naming the token, supported set, and raw string. The inbound
  interceptor materializes Timestamp validation so `Timestamp
  Signature` is fully enforced. README token lists updated.
- **Producer response cap.** `toXmlString` serializes through a
  counting stream bounded by `CXF_MAX_BODY_BYTES` (same env, default,
  ceiling, fail-loud parse as the consumer cap); over-cap invokes fail
  `RESOURCE_EXHAUSTED` naming the env var and observed size. README
  documents both directions.
- **Namespace-aware extraction.** The inbound handler parses the
  envelope and calls `SoapEnvelopeHelper.extractBody`; the string-scan
  is deleted. No-Body forwards empty; malformed fails 400.
- **Bounded Dispatch cache.** `CXF_MAX_DISPATCHES` (default 64,
  ceiling 1024, fail-loud parse) with LRU eviction; lookup and cold
  creation are serialized under one lock (invocation stays outside),
  evicted and shutdown-time entries are closed deterministically via
  CXF `DispatchImpl.close()`. README caution updated.
- **Scoped Shibboleth repo.** The repo stays (OpenSAML 5.1.6 resolves
  only there — Central 404s) but serves exclusively
  `org.opensaml`/`net.shibboleth` groups via Gradle `exclusiveContent`,
  proven with `--refresh-dependencies`.

## Impact

- Affected: `bridges/cxf/src/main/java/org/rustcamel/cxf/`
  (SecurityProfile, WssSecurityProcessor, CxfBridgeService,
  CxfClientManager, SoapEndpointPublisher), `build.gradle.kts`,
  `bridges/cxf/README.md`, delta spec `bridge-transport-security`.
- No Rust-side changes. No wire-format changes.
- Risks: the whitelist rejects configs that "worked" (silently
  insecure — intended); over-cap responses and malformed envelopes now
  fail loudly (operator action: raise env / fix client); cache
  serialization adds a cold-creation lock (hot invokes unchanged).
