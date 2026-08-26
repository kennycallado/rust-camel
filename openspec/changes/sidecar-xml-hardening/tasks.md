# Tasks: sidecar-xml-hardening

Test invocation for every task (host has no JDK; repo in-container pattern — see design.md).
From the host, `<bridge>` being `bridges/xml` or `bridges/cxf`:

```
docker run --rm --user=0:0 --network=host \
  --volume=<repo>/<bridge>:/project:z --workdir=/project \
  --env=GRADLE_USER_HOME=/project/.gradle-docker-cache --tmpfs=/tmp:rw,exec \
  --entrypoint bash quay.io/quarkus/ubi9-quarkus-graalvmce-builder-image:jdk-21 \
  -c 'HOST_UID=$(stat -c "%u" /project); java -cp gradle/wrapper/gradle-wrapper.jar org.gradle.wrapper.GradleWrapperMain test --no-daemon 2>&1 | tail -30; ST=${PIPESTATUS[0]}; chown -R ${HOST_UID}:${HOST_UID} /project/build /project/.gradle-docker-cache 2>/dev/null; exit $ST'
```

Append `--tests "<fqcn>"` (or `--tests "<fqcn>.<method>"`) to the Gradle invocation to scope a
run. The `gradlew` shell script MUST NOT be used inside the container (the image exports
`APP_HOME=/home/quarkus`, which breaks it). Gradle cache persists at `bridges/<x>/.gradle-docker-cache`
(gitignored); do not commit build outputs.

## bridges/xml (XSLT sidecar)

### Task 1.0: Repair entityExpansionLimit wiring (baseline defect) + positive control test

Baseline defect (bd rc-959h, discovered during task 1.1 red-phase attempt): `XsltTransformerService.secureSaxSource`
line 185 calls `reader.setProperty(PROPERTY_ENTITY_EXPANSION_LIMIT, 100)` with NO try/catch;
Xerces 2.12.2 does not recognize that Oracle property on an `XMLReader` and throws
`SAXNotRecognizedException` → EVERY compileStylesheet/transform RPC fails
(COMPILATION_FAILED). The XSLT pipeline is dead on main; existing negative security tests pass
vacuously (no positive XSLT test exists). The sibling `XsdValidatorService:247-251` already
ships the correct shape: try/catch `(SAXException ignored)` — the tight limit is best-effort
Xerces-specific, while `FEATURE_SECURE_PROCESSING=true` provides the enforced limit (proven by
the passing billion-laughs test against the XSD path).

**Files:**
- `bridges/xml/src/main/java/org/rustcamel/xmlbridge/XsltTransformerService.java` (modified)
- `bridges/xml/src/test/java/org/rustcamel/xmlbridge/SecurityTest.java` (modified)

**Steps:**
1. Write the positive control test below first (TDD); run the scoped container command and
   confirm it FAILS against untouched production code (compile returns COMPILATION_FAILED
   error — the defect evidence).
2. Wrap the `reader.setProperty(PROPERTY_ENTITY_EXPANSION_LIMIT, 100);` call at line 185 in
   `try { reader.setProperty(PROPERTY_ENTITY_EXPANSION_LIMIT, 100); } catch
   (org.xml.sax.SAXException ignored) {}` — byte-for-byte the sibling shape
   of `XsdValidatorService:247-251` (add the `org.xml.sax.SAXException` import). No other
   production edits.
3. Re-run scoped (positive test green), then the FULL `bridges/xml` container test command —
   the three resolver tests committed in 876985da now exercise real attack vectors instead of
   the compile-error loophole (they must STILL pass: with the pipeline alive but deny-all hooks
   not yet installed, the unparsed-text observing-server test would go red — that is Task 1.1's
   red phase, NOT this task's; if `unparsedTextSsrfAttemptRejectedWithoutConnection` fails
   here, that is EXPECTED red evidence for 1.1 — record it and mark this task's full-suite
   acceptance as "1.0 tests green + 1.1 red-phase evidence captured", do not chase it in 1.0).

**Tests:**
- `happyPathTransformSucceeds` (in `SecurityTest`): setup — benign stylesheet
  `<xsl:stylesheet xmlns:xsl="http://www.w3.org/1999/XSL/Transform" version="3.0"><xsl:output omit-xml-declaration="yes"/><xsl:template match="/"><out><xsl:value-of select="name(/*)"/></out></xsl:template></xsl:stylesheet>`
  (the `xsl:output` pins serialization so no XML declaration is emitted and the bytes
  comparison is deterministic); action — compileStylesheet (assert NO error), then transform
  document `<doc/>`; assert — transform response has NO error AND result decodes exactly to
  `<out>doc</out>`.

**Acceptance:**
- Scoped run: `happyPathTransformSucceeds` green after the fix (red before).
- Full container `gradle test` for `bridges/xml`: `happyPathTransformSucceeds` + the four
  original tests + `collectionAttemptRejected` (still port-9-shaped until 1.1 upgrades it)
  green; `unparsedTextSsrfAttemptRejectedWithoutConnection` and
  `resultDocumentFileWriteRejected` MAY go red once the pipeline lives — that is Task 1.1's
  red evidence (attacks now actually fire); record which of them went red, do not chase in 1.0.
- `bd rc-959h` fix delivered by this task (close after merge).

- [x] 1.0

### Task 1.1: Deny-all Saxon secondary resolvers + SSRF/write regression tests

Covers spec requirement "Saxon secondary resolvers deny-all" (all three scenarios).

**Files:**
- `bridges/xml/src/main/java/org/rustcamel/xmlbridge/XsltTransformerService.java` (modified)
- `bridges/xml/src/test/java/org/rustcamel/xmlbridge/SecurityTest.java` (modified)

**Steps:**
0. The three regression tests are ALREADY committed (876985da) but currently pass vacuously
   through the compile-error loophole; Task 1.0 repairs the pipeline. After 1.0 lands, upgrade
   the `collectionAttemptRejected` oracle FIRST (see Tests) so it cannot pass via
   connection-refused ambiguity, then capture the red phase: with the pipeline alive and the
   deny-all hooks NOT yet installed, `unparsedTextSsrfAttemptRejectedWithoutConnection` and
   the upgraded `collectionAttemptRejected` go RED (observing server records a connection /
   count > 0 — the attack actually fires); `resultDocumentFileWriteRejected` goes RED the same
   way if result-document executes at transform time (canary file appears) or stays
   compile-error-shaped if Saxon rejects at compile — record which.
1. Run the scoped container command and confirm/capture the red state per test before touching
   production code.
2. In `XsltTransformerService.getOrCompileTemplates`, immediately after
   `var config = new Configuration();` (line 156), install three deny-all hooks:
   - `config.setUnparsedTextURIResolver((java.net.URI absoluteUri, String encoding, Configuration cfg) -> { throw new net.sf.saxon.trans.XPathException("Unparsed-text access denied: " + absoluteUri); })` — Saxon 12.5 signature `resolve(URI, String, Configuration)` returning `Reader`;
   - `config.setCollectionFinder((context, uri) -> { throw new net.sf.saxon.trans.XPathException("Collection access denied: " + uri); })`;
   - `config.setOutputURIResolver(new net.sf.saxon.lib.OutputURIResolver() { @Override public javax.xml.transform.Result resolve(String href, String base) throws javax.xml.transform.TransformerException { throw new javax.xml.transform.TransformerException("Result-document access denied: " + href); } @Override public net.sf.saxon.lib.OutputURIResolver newInstance() { return this; } @Override public void close(javax.xml.transform.Result result) {} })` — anonymous class; Saxon-HE 12.5's interface has THREE abstract methods (resolve/newInstance/close — javap-verified); `newInstance() { return this; }` is sound for a stateless deny-all.
3. Add imports (`java.net.URI`, `net.sf.saxon.lib.OutputURIResolver`, `net.sf.saxon.trans.XPathException`); no other production edits.
4. Re-run the scoped container command; then the full `bridges/xml` container test command.

**Tests:** (all in `org.rustcamel.xmlbridge.SecurityTest`, `@QuarkusTest`, using the existing
`xslt` stub and `bytes()` helper; compile via `xslt.compileStylesheet`, transform via
`xslt.transform`)
- `unparsedTextSsrfAttemptRejectedWithoutConnection`: setup — observing server:
  `ServerSocket ss = new ServerSocket(0, 10, InetAddress.getByName("127.0.0.1"))`, an
  `AtomicInteger accepted` incremented by a dedicated accept-loop daemon thread:
  `while (true) { Socket s = ss.accept(); accepted.incrementAndGet(); s.close(); }`
  (accepting AND closing each socket guarantees the loop never blocks on an unhandled
  connection — a vulnerable transform yields count 1, not a hang); teardown closes `ss`
  (unblocking the loop) and joins the thread with a timeout. Stylesheet
  `<xsl:stylesheet xmlns:xsl="http://www.w3.org/1999/XSL/Transform" version="3.0"><xsl:template match="/">rejected:<xsl:value-of select="unparsed-text('http://127.0.0.1:PORT/poc')"/></xsl:template></xsl:stylesheet>`
  with the real ephemeral port substituted; action — compileStylesheet then transform a trivial
  `<doc/>` document; assert — response `hasError()` is true, error kind is not
  `BridgeError.Kind.UNKNOWN`, error message does NOT contain the string `rejected:`, and
  `accepted.get() == 0`.
- `collectionAttemptRejected` (UPGRADED oracle — replaces the committed port-9 version): same
  observing-server pattern as the unparsed-text test (ephemeral `ServerSocket` +
  `AtomicInteger accepted` + accept-loop daemon thread + teardown). Stylesheet template calls
  `collection('http://127.0.0.1:PORT/poc')` with the real ephemeral port; action —
  compileStylesheet and, if compile succeeds, transform a trivial document; assert — compile or
  transform response `hasError()` with kind not `UNKNOWN` AND `accepted.get() == 0` (port-9
  could not distinguish deny-all from connection-refused; the observing server can).
- `resultDocumentFileWriteRejected`: setup — `@TempDir Path tmp` canary target
  `tmp.resolve("canary.txt")`, stylesheet `<xsl:template match="/"><xsl:result-document href="file://CANARY">x</xsl:result-document>ok</xsl:template>`
  with the absolute `canary` path spliced in; action — compileStylesheet then transform;
  assert — response `hasError()` (compile or transform), AND `Files.notExists(canary)`.

**Acceptance:**
- Full container `gradle test` for `bridges/xml` exits 0 (old 4 tests + 3 new green).
- `grep -c "setUnparsedTextURIResolver\|setCollectionFinder\|setOutputURIResolver" bridges/xml/src/main/java/org/rustcamel/xmlbridge/XsltTransformerService.java` returns 3.
- No canary file created; observing-server connection count 0.

- [x] 1.1

## bridges/cxf (CXF sidecar)

### Task 2.1: WSS replay cache + per-profile processor reuse in publisher

Covers spec requirement "WSS replay protection on consumer inbound path" (both scenarios).

**Files:**
- `bridges/cxf/src/main/java/org/rustcamel/cxf/WssSecurityProcessor.java` (modified)
- `bridges/cxf/src/main/java/org/rustcamel/cxf/SoapEndpointPublisher.java` (modified)
- `bridges/cxf/src/test/java/org/rustcamel/cxf/WssSecurityProcessorIntegrationTest.java` (modified)
- `bridges/cxf/src/test/java/org/rustcamel/cxf/SoapEndpointPublisherTest.java` (modified)

**Steps:**
1. Write the tests below first; confirm they fail for the right reason before production edits
   (replay test fails because the second `processInbound` succeeds — see step 3 for why the
   Timestamp emission is a required production edit, not test scaffolding).
2. `WssSecurityProcessor`: add field `private final org.apache.wss4j.common.cache.ReplayCache replayCache = new org.apache.wss4j.common.cache.MemoryReplayCache();` and in `processInbound`, after
   `requestData.setCallbackHandler(profile.createPasswordCallback());` (line 95), add
   `requestData.setTimestampReplayCache(replayCache);` and `requestData.setNonceReplayCache(replayCache);`.
3. `WssSecurityProcessor.processOutbound`: add a Timestamp branch BEFORE the Signature branch:
   `if (containsAction(actions, "Timestamp")) {
   new org.apache.wss4j.dom.message.WSSecTimestamp(secHeader).build(); }` — `WSSecTimestamp`
   (WSS4J 4.0.0) constructs with the header and `build()` takes no args. This is REQUIRED for
   the spec premise ("a signed+timestamped envelope produced by processOutbound"): WSS4J's
   `SignatureProcessor.testMessageReplay` locates the `wsu:Timestamp` from the prior
   TS-processor results or a forward-sibling scan of `ds:Signature` and returns early when
   none exists — without emission there is nothing for the replay cache to key on and the
   replay tests cannot pass. Insertion order before the Signature build is what makes the
   sibling scan find the TS; do NOT add the timestamp to the signature parts (default
   `WSSecSignature` signs the body only — that is fine).
4. `SoapEndpointPublisher`: add field `private final java.util.concurrent.ConcurrentHashMap<String, WssSecurityProcessor> wssProcessorsByProfile = new java.util.concurrent.ConcurrentHashMap<>();`
   and package-private accessor `WssSecurityProcessor wssProcessorFor(String profileName, SecurityProfile profile)`
   computing `wssProcessorsByProfile.computeIfAbsent(profileName, n -> new WssSecurityProcessor(profile))`.
   Replace the per-request construction at line 84 with
   `WssSecurityProcessor wssProcessor = wssProcessorFor(profileName, profile);`.
5. Run scoped container tests, then the full `bridges/cxf` container test command.

**Tests:**
- `replayedFreshSignedMessageRejectedAtProcessorLevel` (in `WssSecurityProcessorIntegrationTest`,
  real crypto fixtures following its existing TestKeystoreHelper pattern; profile with inbound
  AND outbound actions `Timestamp Signature`): setup — `WssSecurityProcessor p` built on a real
  keystore profile; `String signed = p.processOutbound(plainEnvelope)`; action —
  `p.processInbound(signed)` then `p.processInbound(signed)` again with identical bytes;
  assert — first call returns normally, second call throws `WSSecurityException`.
- `publisherReusesProcessorPerProfile` (in `SoapEndpointPublisherTest`, mock harness): setup —
  mock `SecurityProfileStore.getProfile("test_profile")` returning a minimal profile stub;
  action — call `publisher.wssProcessorFor("test_profile", profile)` twice and once with a
  different profile name; assert — the two same-name calls return the SAME instance
  (`assertSame`) and the different-name call returns a different instance (`assertNotSame`).
  (Request-handler routing through the accessor is proven behaviorally by the endpoint replay
  test below; this test pins the memoization contract only.)
- `replayedRequestRejectedThroughPublishedEndpoint` (in `SoapEndpointPublisherTest`): setup —
  `SecurityProfileStore` stub returning a REAL profile built with `TestKeystoreHelper`
  (actions `Timestamp Signature` inbound AND outbound); construct the signed request body via
  `new WssSecurityProcessor(realProfile).processOutbound(plainEnvelope)`; drive the publisher's
  request handler twice with the identical body buffer using the existing harness pattern
  (mocked `httpRequest`/`bodyBuffer`, `vertx.executeBlocking` captor-mocked per
  `SoapEndpointPublisherTest:87-94` so both the `Callable` and the
  `Handler<AsyncResult<String>>` are captured). First POST: execute the captured callable
  inline (it succeeds), invoke the captured result handler with
  `Future.succeededFuture(result)`, verify `httpResponse.setStatusCode(200)` and the signed
  body via mock verify. Second POST with identical bytes: `assertThrows` on executing the
  newly captured callable (WSS replay throws `WSSecurityException`), then invoke ITS captured
  result handler with `Future.failedFuture(thrown)` — the mocked `executeBlocking` does not
  run handlers itself, so the handler MUST be driven explicitly with the failure before
  asserting the fault path — then mock-verify `httpResponse.setStatusCode(400)` and capture
  the response body asserting it contains `soap:Client` (the WSSecurityException fault
  convention of `SoapEndpointPublisher` lines 156-175).

**Acceptance:**
- Full container `gradle test` for `bridges/cxf` exits 0.
- `grep -c "setTimestampReplayCache\|setNonceReplayCache" WssSecurityProcessor.java` returns 2.
- `grep -c "WSSecTimestamp" WssSecurityProcessor.java` returns ≥1 (Timestamp emission present).
- `SoapEndpointPublisher` contains exactly one `new WssSecurityProcessor(` call site (inside
  the `computeIfAbsent` lambda).

- [x] 2.1

### Task 2.2: Hardened identity TransformerFactory at all three sites

Covers spec requirement "Hardened identity transformer factories" (single scenario, three call
sites).

**Files:**
- `bridges/cxf/src/main/java/org/rustcamel/cxf/SecureTransformers.java` (new)
- `bridges/cxf/src/main/java/org/rustcamel/cxf/CxfBridgeService.java` (modified)
- `bridges/cxf/src/main/java/org/rustcamel/cxf/SoapEnvelopeHelper.java` (modified)
- `bridges/cxf/src/test/java/org/rustcamel/cxf/SecureTransformersTest.java` (new)

**Steps:**
1. Create `final class SecureTransformers` (package-private, private constructor) with a
   COMPILE-ONLY stub first: `static TransformerFactory factory()` that just returns
   `TransformerFactory.newInstance()` with no configuration. Widen visibility so the test
   module compiles: `CxfBridgeService.toXmlString` and `SoapEnvelopeHelper.sourceToBytes` go
   from `private static` to package-private `static` (same-package test access;
   `sourceToString` is already public). No behavior change.
2. Write BOTH tests below; run them scoped — `factoryReportsHardenedAttributes` FAILS
   (red: stub returns an unconfigured factory); `serializationUnchangedAtAllThreeSites` PASSES
   (current factories already produce correct output — its role is the regression freeze).
3. Configure the factory (green phase): in `SecureTransformers.factory()` set feature
   `XMLConstants.FEATURE_SECURE_PROCESSING=true`, attributes
   `XMLConstants.ACCESS_EXTERNAL_DTD=""` and `XMLConstants.ACCESS_EXTERNAL_STYLESHEET=""`
   (NO `ACCESS_EXTERNAL_TRANSFORM` — Java 21 `XMLConstants` has no such constant), wrapping
   any `javax.xml.transform.TransformerConfigurationException` in `IllegalStateException`
   (fail-loud, same style as Task 2.3's seam). Re-run: attributes test now green.
4. Replace the three `TransformerFactory.newInstance()` sites: `CxfBridgeService.toXmlString`
   (line 178), `SoapEnvelopeHelper.sourceToBytes` (line 171), `SoapEnvelopeHelper.sourceToString`
   (line 178) — each site's `TransformerFactory.newInstance()` expression is replaced by
   `SecureTransformers.factory()` (keeping the chained `.newTransformer()` calls as-is).
5. Re-run the tests: serialization literals UNCHANGED across the factory switch (the freeze
   assertion — hardening did not alter serialization). Then run the full `bridges/cxf`
   container test command.

**Tests:** (new `SecureTransformersTest`, plain JUnit 5)
- `factoryReportsHardenedAttributes`: setup — `TransformerFactory f = SecureTransformers.factory()`;
  action — query attributes/feature; assert — `f.getFeature(XMLConstants.FEATURE_SECURE_PROCESSING)`
  is true, `f.getAttribute(XMLConstants.ACCESS_EXTERNAL_DTD)` equals `""`,
  `f.getAttribute(XMLConstants.ACCESS_EXTERNAL_STYLESHEET)` equals `""`.
- `serializationUnchangedAtAllThreeSites`: setup — fixed input DOM built by parsing the literal
  `<soap:Envelope xmlns:soap="http://schemas.xmlsoap.org/soap/envelope/"><soap:Body><ping/></soap:Body></soap:Envelope>`
  via `SoapEnvelopeHelper.parseResponse`. FROZEN expected literal (verified byte-exact on the
  container JDK 21 identity transformer — all three sites emit the same bytes for this input):
  `<?xml version="1.0" encoding="UTF-8" standalone="no"?><soap:Envelope xmlns:soap="http://schemas.xmlsoap.org/soap/envelope/"><soap:Body><ping/></soap:Body></soap:Envelope>`
  (no trailing newline); action — `SoapEnvelopeHelper.sourceToString(new DOMSource(doc), false)`,
  `new String(SoapEnvelopeHelper.sourceToBytes(new DOMSource(doc)), UTF_8)`,
  `CxfBridgeService.toXmlString(new DOMSource(doc))`; assert — each result
  `equals(EXPECTED)` where EXPECTED is the single frozen literal above.

**Acceptance:**
- Full container `gradle test` for `bridges/cxf` exits 0.
- `grep -n "TransformerFactory.newInstance()" bridges/cxf/src/main/java/org/rustcamel/cxf/CxfBridgeService.java` has no output; same for `SoapEnvelopeHelper.java` (all sites now via `SecureTransformers.factory()`; the single remaining `newInstance()` call lives inside `SecureTransformers`).

- [x] 2.2

### Task 2.3: Fail-loud SECURE_DBF via injectable configuration seam

Covers spec requirement "Fail-loud secure DocumentBuilderFactory configuration" (both
scenarios).

**Files:**
- `bridges/cxf/src/main/java/org/rustcamel/cxf/SoapEnvelopeHelper.java` (modified)
- `bridges/cxf/src/test/java/org/rustcamel/cxf/SoapEnvelopeHelperTest.java` (modified)

**Steps:**
1. Write the two tests below first. The feature-query test passes against current code; the
   stub test fails to compile until step 2 adds the seam (acceptable red: add the seam, then
   confirm the stub test passes and no swallow remains).
2. In `SoapEnvelopeHelper`, extract package-private
   `static DocumentBuilderFactory configureSecure(DocumentBuilderFactory dbf)` that applies the
   initializer's current settings (namespace-aware, disallow-doctype-decl true,
   external-general-entities false, external-parameter-entities false, load-external-dtd false,
   XIncludeAware false) — of these, the four `setFeature` calls can throw the checked
   `javax.xml.parsers.ParserConfigurationException`; the seam wraps any such exception in
   `IllegalStateException` with the original as cause. Rewrite the `ThreadLocal.withInitial`
   initializer to
   `ThreadLocal.withInitial(() -> configureSecure(DocumentBuilderFactory.newInstance()))` —
   the empty `catch (ParserConfigurationException ignored)` block at line 34 is deleted.
3. Run scoped container tests, then the full `bridges/cxf` container test command.

**Tests:** (in `SoapEnvelopeHelperTest`, plain JUnit 5)
- `configureSecureThrowsIllegalStateWhenFeatureUnsupported`: setup — test-local stub class
  extending `DocumentBuilderFactory` whose `setFeature(String name, boolean value)` throws
  `new ParserConfigurationException("boom")` (remaining abstract methods throw
  `UnsupportedOperationException`); action —
  `assertThrows(IllegalStateException.class, () -> SoapEnvelopeHelper.configureSecure(stub))`;
  assert — the thrown exception's cause is the original `ParserConfigurationException`.
- `secureDbfHasAllHardeningFeaturesEnabled`: setup —
  `DocumentBuilderFactory dbf = SoapEnvelopeHelper.configureSecure(DocumentBuilderFactory.newInstance())`;
  action/assert — `dbf.getFeature("http://apache.org/xml/features/disallow-doctype-decl")` is
  true, `dbf.getFeature("http://xml.org/sax/features/external-general-entities")` is false,
  `dbf.getFeature("http://xml.org/sax/features/external-parameter-entities")` is false,
  `dbf.getFeature("http://apache.org/xml/features/nonvalidating/load-external-dtd")` is false,
  `dbf.isXIncludeAware()` is false, `dbf.isNamespaceAware()` is true.

**Acceptance:**
- Full container `gradle test` for `bridges/cxf` exits 0.
- `grep -n "ignored" bridges/cxf/src/main/java/org/rustcamel/cxf/SoapEnvelopeHelper.java` has no output (no silent-swallow catch remains).

- [x] 2.3
