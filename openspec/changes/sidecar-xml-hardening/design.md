# Design: sidecar-xml-hardening

## Approach

Four independent hardening fixes in the Java sidecars, each landing with a regression test
(TDD per finding). Verification runs inside the established GraalVM container
(`quay.io/quarkus/ubi9-quarkus-graalvmce-builder-image:jdk-21`) invoking the wrapper jar
directly (`java -cp gradle/wrapper/gradle-wrapper.jar org.gradle.wrapper.GradleWrapperMain test
--no-daemon`) with `GRADLE_USER_HOME=/project/.gradle-docker-cache` — the repo's documented
in-container pattern; the `gradlew` shell script mis-resolves `APP_HOME` inside this image.

**N0 (baseline repair, prerequisite).** `XsltTransformerService.secureSaxSource:185` calls
`reader.setProperty(entityExpansionLimit, 100)` unguarded; Xerces 2.12.2 throws
`SAXNotRecognizedException` for that Oracle property on an `XMLReader`, killing every
compile/transform RPC on main (the existing negative tests pass vacuously — bd rc-959h). Fix:
mirror the sibling `XsdValidatorService:247-251` shape (try/catch `SAXException ignored`) —
`FEATURE_SECURE_PROCESSING` provides the enforced limit, proven by the passing billion-laughs
test against the XSD path, which already ships this shape. Add the missing positive XSLT
happy-path control test so pipeline death can never hide again.

**N1 (Saxon secondary resolvers).** In `XsltTransformerService.getOrCompileTemplates`, after
`new Configuration()`, install three deny-all hooks on the Saxon `Configuration`:

- `setUnparsedTextURIResolver((URI absoluteUri, String encoding, Configuration config) ->
  throw XPathException)` — Saxon 12.5 signature; blocks `unparsed-text()` /
  `unparsed-text-available()` (default `StandardUnparsedTextResolver` opens via
  `URLConnection`);
- `setCollectionFinder((context, uri) -> throw XPathException)` — blocks
  `collection()` / `uri-collection()`;
- `setOutputURIResolver(deny-all)` — `resolve()` throws (blocks `xsl:result-document`),
  `close()` is a no-op; both methods are required by the interface.

The resolvers live on the `Configuration`, so they are inherited by every `Templates` produced
from the factory — one wiring point covers compile and runtime. Existing
`ALLOW_EXTERNAL_FUNCTIONS=false` and the denying JAXP `URIResolver` (document()/import/include)
stay untouched. Tests extend `SecurityTest.java`. For the SSRF class the oracle is an
**observing loopback server**: bind a `ServerSocket` to `127.0.0.1:0` (ephemeral port), point
`unparsed-text()` at it, assert the transform returns a `BridgeError` AND the accepted-connection
count stays zero (a closed port cannot distinguish "denied before connect" from "connect
refused"). The `collection()` test asserts `BridgeError`; the result-document test uses
`@TempDir` and asserts both `BridgeError` and no canary file at the target path.

**N2 (WSS replay cache).** `WssSecurityProcessor` gains a `private final ReplayCache
replayCache = new MemoryReplayCache()`. `processInbound` sets
`requestData.setTimestampReplayCache(replayCache)` and `requestData.setNonceReplayCache(replayCache)`.
Lifetime correction: `SoapEndpointPublisher` currently constructs a processor **per request**
(`:84`, inside the request handler), so per-instance state cannot span requests. The publisher
instead keeps one `WssSecurityProcessor` per security profile
(`ConcurrentHashMap<String, WssSecurityProcessor>`; `SecurityProfile` is immutable and
`profileStore.getProfile(name)` is deterministic), so the cache spans the endpoint's lifetime —
same posture as the client path's Bus-managed cache. Additionally, `processOutbound` gains a
`Timestamp` action branch (`WSSecTimestamp(secHeader).build()`, inserted before the Signature
build) so a `Timestamp Signature` profile produces a signed+timestamped envelope — WSS4J's
`SignatureProcessor.testMessageReplay` keys the cache off the `wsu:Timestamp` (prior
TS-processor result or forward-sibling of `ds:Signature`) and returns early when none exists,
so without emission there is nothing to replay-protect; the default `WSSecSignature` covers the
body only, which suffices. WSS4J's Timestamp/UsernameToken processors consult these caches
automatically; a replayed still-fresh message (default TTL 300s) hits the cache and throws
`WSSecurityException`. Tests at two levels: (a) processor-level — build
profile with actions `Timestamp Signature` → `processOutbound` produces signed+timestamped
envelope → first `processInbound` succeeds → second `processInbound` of identical bytes throws;
(b) publisher-level (extends `SoapEndpointPublisherTest`, fixtures via `TestKeystoreHelper`) —
POST the identical signed envelope to the published endpoint twice; first request succeeds,
second gets a security-fault response.

**N3 (identity transformers).** New package-private helper `SecureTransformers.factory()` in
`bridges/cxf` returning a `TransformerFactory` configured once with
`FEATURE_SECURE_PROCESSING=true`, `ACCESS_EXTERNAL_DTD=""`, `ACCESS_EXTERNAL_STYLESHEET=""`
(Java 21 `XMLConstants` defines no `ACCESS_EXTERNAL_TRANSFORM`). Applied at the three sites:
`CxfBridgeService.toXmlString`, `SoapEnvelopeHelper.sourceToBytes`,
`SoapEnvelopeHelper.sourceToString`; private helpers are widened to package-private where the
test needs direct access (same-package tests). Each site's test asserts factory attributes and
compares serialization against a fixed expected literal (checked in the test), covering all
three call sites.

**N4 (fail-loud DBF).** `SoapEnvelopeHelper.SECURE_DBF` initialization: extract a
package-private `static DocumentBuilderFactory configureSecure(DocumentBuilderFactory dbf)`
seam that sets the four features and throws `IllegalStateException` wrapping any
`ParserConfigurationException` (a `ThreadLocal.withInitial` lambda cannot propagate a checked
exception — `IllegalStateException` is the fail-loud carrier, matching the fail-loud style of
`WssSecurityProcessor.parseXml`). The `ThreadLocal.withInitial` initializer calls the seam.
Tests: (a) a stub `DocumentBuilderFactory` subclass whose `setFeature` throws
`ParserConfigurationException`, passed to the seam, asserts `IllegalStateException` propagates
(injectable failure — no silent fallback branch remains); (b) the initialized factory reports
all four hardening features enabled.

## Affected crates

- No Rust crates. Java sidecars only: `bridges/xml` (`XsltTransformerService`,
  `SecurityTest`), `bridges/cxf` (`WssSecurityProcessor`, `SoapEndpointPublisher`,
  `SoapEnvelopeHelper`, `CxfBridgeService`, new `SecureTransformers`,
  `WssSecurityProcessorTest`, `SoapEndpointPublisherTest`, `SoapEnvelopeHelperTest`).

## Architecture boundaries

The sidecars sit behind the runtime-interop-legacy boundary (positioning decision,
analysis-apache-camel-parity): Rust crates call them over gRPC and are untouched. ADR-0032's
trust model is the design driver — exchange data (untrusted) is already fully hardened; this
change closes the operator-trusted-stylesheet class and the WSS posture gap so the trust
assumption is enforced rather than assumed. No new externally-visible types, gRPC surface, or
configuration; `NativeImageReflectionRegistrations` needs no changes (no new reflective call
sites — deny-all lambdas are compile-time constructs).

Single-phase change: four small independent fixes, one verification loop, no ordering
constraints between them.

## Alternatives considered

- **Route consumer inbound through `WSS4JInInterceptor`** (Bus-managed cache) — rejected:
  pulls the standalone processor onto the CXF Bus lifecycle for one field, a large blast-radius
  refactor for a hardening change.
- **EHCache replay cache** — rejected: adds a dependency and disk-backed state for no
  posture gain at this scope; `MemoryReplayCache` matches the client path's effective
  guarantee (single-process scope).
- **Static shared replay cache across processors** — rejected: per-profile processor cache
  isolates cache state per security profile and avoids a process-global mutable singleton;
  sufficient for the replay window.
- **Deny resolvers on the `Controller` per-transform** — rejected: `Configuration`-level is the
  single compile-time wiring point, inherited by all `Templates`; per-transform re-application
  duplicates state for no benefit.
- **Test outside the container via a host JDK** — rejected: repo pattern is the GraalVM
  container (host has no JDK; audit §7 hit exactly this).
