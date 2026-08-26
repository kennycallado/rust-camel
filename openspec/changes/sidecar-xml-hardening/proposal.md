# Proposal: sidecar-xml-hardening

## Why

The sidecar-xml-security spike (2026-08-07, verdict CLEAN-WITH-GAPS, bd rc-wp9d) left four
defense-in-depth gaps in the Java sidecars. None is exploitable through untrusted input today,
but each becomes a live hole if a trust assumption shifts (e.g. a future stylesheet-upload
feature or a non-idempotent consumer operation fronted by WSS):

- N1 — Saxon secondary resolvers (`unparsed-text()`, `collection()`, `xsl:result-document`) are
  reachable from the operator-trusted stylesheet and neither blocked nor tested. Blast radius if
  trust breaks: SSRF, local file read, arbitrary file write.
- N2 — the consumer/server inbound path (`WssSecurityProcessor.processInbound`, untrusted SOAP)
  builds a standalone `WSSecurityEngine` with no `ReplayCache` on `RequestData`; a captured,
  still-fresh signed message replays within the TTL window. Compounding this, the publisher
  constructs a fresh processor per request, so no per-instance state could span requests.
- N3 — identity `TransformerFactory` sites (`CxfBridgeService.toXmlString`,
  `SoapEnvelopeHelper.sourceToBytes`/`sourceToString`) set no hardening attributes.
- N4 — `SoapEnvelopeHelper.SECURE_DBF` swallows `ParserConfigurationException`, silently
  falling back to a weakly-configured factory if a feature is ever unsupported.

Post-v1.0 hardening per the audit recommendation (explicitly does NOT block the v1.0 freeze).

## What Changes

- `bridges/xml` baseline repair (bd rc-959h, discovered by this change's red phase):
  `XsltTransformerService.secureSaxSource` sets the Oracle `entityExpansionLimit` property on
  the `XMLReader` where Xerces 2.12.2 does not recognize it — every compile/transform RPC
  fails on main today. Wrapped in try/catch exactly like the proven sibling
  `XsdValidatorService` (FSP provides the enforced limit), plus the missing positive
  XSLT happy-path control test.
- `bridges/xml` `XsltTransformerService.getOrCompileTemplates`: deny-all
  `UnparsedTextURIResolver`, `CollectionFinder`, `OutputURIResolver` on the Saxon
  `Configuration`. Regression tests for `unparsed-text` SSRF (observing loopback server:
  connection counter must stay at zero), `collection()`, and `result-document` file-write
  (canary file must not appear).
- `bridges/cxf` `WssSecurityProcessor` + `SoapEndpointPublisher`: the publisher reuses one
  processor per security profile (today it constructs one per request), and each processor
  owns a `MemoryReplayCache` wired as both timestamp and nonce replay cache on `RequestData`.
  Replay regression at both levels: reused-processor `processInbound` (fresh message accepted
  once, replay throws) and end-to-end publisher test (identical POST twice: first succeeds,
  second gets a security failure).
- `bridges/cxf`: hardened identity transformer factory (shared helper; `FEATURE_SECURE_PROCESSING`
  + empty `ACCESS_EXTERNAL_DTD`/`ACCESS_EXTERNAL_STYLESHEET` — Java 21 defines no
  `ACCESS_EXTERNAL_TRANSFORM` constant) applied at all three `TransformerFactory` sites;
  `SECURE_DBF` made fail-loud through an injectable configuration seam (rethrow
  `ParserConfigurationException` wrapped in `IllegalStateException`; a `ThreadLocal.withInitial`
  lambda cannot propagate a checked exception).

Excluded: no Rust crate changes (camel-cxf/camel-xslt/camel-xj clients are untouched), no new
configuration surface, no EHCache dependency (in-memory cache suffices), no revocation
(CRL/OCSP) work, no cross-process or post-restart replay protection (`MemoryReplayCache`
protects a single sidecar process lifetime — same class of guarantee as the client path).

## Acceptance criteria

- Stylesheets using `unparsed-text()`, `unparsed-text-available()`, `collection()`, or
  `uri-collection()` against any URI return a `BridgeError`, and an observing loopback server
  (connection counter) records zero connection attempts for the `unparsed-text` case.
- A stylesheet with `xsl:result-document href="file://…"` fails the transform and writes no file.
- The publisher constructs at most one `WssSecurityProcessor` per security profile; a replayed,
  still-fresh signed+timestamped SOAP message is rejected on the consumer inbound path (first
  delivery succeeds, replay throws), both at processor level and end-to-end through the
  published endpoint.
- All identity `TransformerFactory` instances set `FEATURE_SECURE_PROCESSING=true` and empty
  `ACCESS_EXTERNAL_DTD`/`ACCESS_EXTERNAL_STYLESHEET`; serialization output is unchanged
  (verified against fixed expected bytes at every call site).
- A `ParserConfigurationException` raised during secure `DocumentBuilderFactory` configuration
  surfaces as `IllegalStateException` (fail-loud, via an injectable seam tested with a
  throwing factory stub); the initialized factory has all four hardening features enabled.
- `./gradlew test` green in both `bridges/xml` and `bridges/cxf` (GraalVM JDK-21 container).

## Risk budget

Java-only, additive hardening inside existing trust boundaries; regression risk confined to
XSLT transforms that legitimately use secondary outputs (none exist in the operator surface —
stylesheets are read-only transforms). No parser semantics change on untrusted paths. The
replay cache is per-processor-instance, in-memory: restart clears it, which matches the
existing client-path (Bus cache) posture. Out of bounds: any Rust-side change, any new
external dependency, any behavior change to valid transforms.
