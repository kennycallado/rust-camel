# Tasks: cxf-bridge-hardening

Single phase — five independent sidecar fixes, no cross-task build
order. Flat execution; per-task review; holistic review at close.

## Task 1: cxf-token-whitelist

**Files:**
- `bridges/cxf/src/main/java/org/rustcamel/cxf/SecurityProfile.java` (modified)
- `bridges/cxf/src/test/java/org/rustcamel/cxf/SecurityProfileTest.java` (modified)
- `bridges/cxf/README.md` (modified)

**Spec:** `specs/bridge-transport-security/spec.md` — Requirement "CXF
security action tokens are validated against the materialized set"
(all 6 scenarios).

**Steps:**
1. In `SecurityProfile.Builder`, add a private static
   `SUPPORTED_INBOUND_ACTIONS = Set.of("signature", "encrypt",
   "timestamp")` and `SUPPORTED_OUTBOUND_ACTIONS = Set.of("signature",
   "encrypt", "timestamp")` — entries stored LOWERCASE. Each parsed
   token is lowercased before the set membership check, so comparison
   is case-insensitive and case-consistent by construction. Semantics
   match `containsAction`'s exact-token-but-case-insensitive semantics
   (SecurityProfile.java:662-668).
2. In `validateInboundActions()` (SecurityProfile.java:497), after the
   existing blank check: split the raw actions string with the same
   token-splitting helper `containsAction` uses; for each token not in
   `SUPPORTED_INBOUND_ACTIONS`, throw `IllegalArgumentException` with
   message `actions.in contains unsupported action '<token>' (raw:
   '<raw>'); supported inbound actions: Signature, Encrypt, Timestamp
   — UsernameToken/SAML/SignatureConfirmation are not materialized`
   (adapt exact wording to match existing validator error style).
3. In `validateInboundActions()`, add the composition check: if
   `containsAction(raw, "Timestamp")` and not
   `containsAction(raw, "Signature")`, throw with the
   Timestamp-requires-Signature rule text mirroring the outbound
   validator's existing composition error.
4. In `validateOutboundActions()` (SecurityProfile.java:525), add the
   same unknown-token rejection loop against
   `SUPPORTED_OUTBOUND_ACTIONS`.
5. In `SecurityProfile.createInInterceptor` (SecurityProfile.java:297-329,
   called from CxfClientManager.java:115): the inbound composition
   (:303-314) currently has no Timestamp branch — add one that
   includes `WSHandlerConstants.TIMESTAMP` in the WSS4J action
   configuration, mirroring the outbound Timestamp-first ordering
   (:232-237), so timestamp validation is materialized alongside
   Signature/Encrypt. `WssSecurityProcessor.processInbound`'s
   `enforceRequiredActions` + `verifyTimestampSignatureCoverage` path
   is already live — do not change it.
6. README: update the actions documentation — supported inbound
   {Signature, Encrypt, Timestamp (requires Signature)}, outbound
   {Signature, Encrypt, Timestamp (requires Signature)}; unknown
   tokens fail profile build (fail-loud, ADR-0033).

**Tests (all in `SecurityProfileTest` unless noted):**
1. `unknownInboundTokenRejectedAtBuild`
   - setup: Builder with truststore+keystore paths set,
     `actionsIn("UsernameToken")`
   - action: `builder.build()`
   - assert: `IllegalArgumentException` message contains
     `UsernameToken`, `Signature`, `Encrypt`, `Timestamp`, and the raw
     string `UsernameToken`
   - command: `./gradlew test --tests 'org.rustcamel.cxf.SecurityProfileTest'`
   - expected: fails before step 2 (build succeeds today)
2. `unknownOutboundTokenRejectedAtBuild`
   - setup: keystore configured, `actionsOut("SignatureConfirmation")`
   - action: `builder.build()`
   - assert: `IllegalArgumentException` names the token and the
     supported outbound set
   - expected: fails before step 4
3. `bareInboundTimestampRejectedByComposition`
   - setup: truststore+keystore configured,
     `actionsIn("Timestamp")`
   - action: `builder.build()`
   - assert: `IllegalArgumentException` states the
     Timestamp-requires-Signature rule
   - expected: fails before step 3
4. `inboundTimestampSignatureStillBuilds`
   - setup: truststore+keystore configured,
     `actionsIn("Timestamp Signature")`
   - action: `builder.build()`
   - assert: builds successfully
   - expected: passes before AND after (regression pin)
5. `inboundEncryptWithKeystoreBuilds`
   - setup: keystore configured, `actionsIn("Encrypt")`
   - action: `builder.build()`
   - assert: builds successfully (existing material checks unchanged)
   - expected: passes before AND after
6. `mixedCaseTokenTreatedCaseInsensitively`
   - setup: truststore+keystore configured, `actionsIn("signature")`
   - action: `builder.build()`
   - assert: builds successfully (lowercase `signature` is the known
     token, not an unknown one)
   - expected: fails before step 1 if step 1 lowercases tokens against
     a canonical-case Set (the case-handling regression pin)
7. `blankActionsStayRawExempt`
   - setup: no actions set
   - action: `builder.build()`
   - assert: builds successfully
   - expected: passes before AND after
8. In `SecurityProfileTest`: `inboundInterceptorMaterializesTimestamp`
   - setup: profile built with `actionsIn("Timestamp Signature")` and
     crypto material configured (reuse existing builder fixtures)
   - action: call the profile's `createInInterceptor()` and read the
     WSS4J action string it configures (assert on the action value the
     interceptor carries — via the returned interceptor's action
     property or the equivalent observable)
   - assert: the action string is `Timestamp Signature` (Timestamp
     materialized alongside Signature, mirroring the outbound ordering)
   - expected: fails before step 5 (inbound composition
     SecurityProfile.java:303-314 has no Timestamp branch today)
   - note: tamper-enforcement is already covered by
     WssSecurityProcessorIntegrationTest
     .timestampRewriteCannotMintFreshCacheKey — do not duplicate

**Acceptance:**
- `./gradlew test` (full cxf suite) exits 0
- All 8 tests above pass
- `grep -n "SignatureConfirmation\|UsernameToken" bridges/cxf/README.md`
  shows the unsupported-token note in the actions section
- Spec scenarios: unknown-in, unknown-out, bare-in-Timestamp,
  in-Timestamp-Signature, known-sets, blank-exempt — all covered

- [x] 1

## Task 2: cxf-response-cap

**Files:**
- `bridges/cxf/src/main/java/org/rustcamel/cxf/CxfBridgeService.java` (modified)
- `bridges/cxf/src/main/java/org/rustcamel/cxf/BridgeConfig.java` (modified)
- `bridges/cxf/src/main/java/org/rustcamel/cxf/SoapEndpointPublisher.java` (modified — parse delegation only)
- `bridges/cxf/src/test/java/org/rustcamel/cxf/CxfBridgeServiceTest.java` (new)
- `bridges/cxf/README.md` (modified)

**Spec:** Requirement "CXF producer caps response body allocation" (3
scenarios).

**Steps:**
1. In `BridgeConfig`, add BOTH: a static `parseMaxBodyBytes(String
   raw)` (pure parse: malformed/non-positive/over-ceiling →
   `IllegalStateException` naming env var + raw value; DEFAULT when
   raw is null OR BLANK — the current contract at
   SoapEndpointPublisher.java:340-345 defaults on both, and the
   shared parser must preserve it; ceiling `17L * 1024 * 1024`) AND a
   no-arg
   `parseMaxBodyBytes()` that reads the env var and delegates. Refactor
   `SoapEndpointPublisher`'s existing parse
   (SoapEndpointPublisher.java:337-373) to delegate — one parse site,
   both consumers. `CxfBridgeService` captures the parsed cap into a
   package-private instance field `pinnedMaxBodyBytes` (long, default
   -1) at `@PostConstruct`: when positive it wins over
   `BridgeConfig.parseMaxBodyBytes()` — the same pinned-seam pattern
   the publisher uses (SoapEndpointPublisher.java:49-52), and the
   seam the tests set instead of env vars.
2. In `CxfBridgeService`, inject/read the parsed cap at
   `@PostConstruct` (or constructor — match the class's existing
   injection style).
3. Change `toXmlString(Source)` to `toXmlString(Source, long cap)`:
   serialize into a private static `BoundedOutputStream extends
   java.io.OutputStream` that counts bytes and throws
   `ResponseCapExceededException` (new static nested exception class
   carrying the observed byte count) when a write would push the count
   past the cap. Keep the `SecureTransformers.factory()` transformer
   usage unchanged. CRITICAL: the Transformer wraps exceptions in
   `TransformerException` — unwrap the cause chain in `toXmlString`
   and rethrow `ResponseCapExceededException` itself so the service
   catch sees it (a bare cause-inspect in catch is the failure mode
   this step prevents).
4. In `invoke(...)` (CxfBridgeService.java:36), catch
   `ResponseCapExceededException` around the `toXmlString` call and
   answer `responseObserver.onError(Status.RESOURCE_EXHAUSTED
   .withDescription("response body " + observed + " bytes exceeds
   CXF_MAX_BODY_BYTES=" + cap).asRuntimeException())` — no payload
   forwarded, warn-level log.
5. README: extend the `CXF_MAX_BODY_BYTES` paragraph — the cap bounds
   BOTH the inbound request body and the serialized producer response
   body; over-cap responses fail the route exchange with
   RESOURCE_EXHAUSTED naming the size.

**Tests (new `CxfBridgeServiceTest`, plain JUnit — no CDI needed if
the cap path is tested via the static toXmlString overload; otherwise
use the existing test bootstrap style from
SoapEndpointPublisherBodyCapTest):**
1. `oversizedResponseRejectedWithResourceExhausted`
   - setup: `CxfBridgeService` under test with `pinnedMaxBodyBytes =
     1024`; the dispatch path stubbed/mocked (Mockito, per the existing
     CxfClientManagerTest style) to return a `StreamSource` whose XML
     serializes to >1024 bytes; a recording
     `StreamObserver<SoapResponse>`
   - action: call `invoke(request, observer)` end to end
   - assert: `onError` receives `StatusRuntimeException` with code
     `RESOURCE_EXHAUSTED`; description contains both
     `CXF_MAX_BODY_BYTES` and a digit sequence matching the observed
     byte count; `onNext` never called; `onCompleted` never called
   - command: `./gradlew test --tests 'org.rustcamel.cxf.CxfBridgeServiceTest'`
   - expected: fails before step 3 (no bound exists)
2. `responseAtExactlyCapPasses`
   - setup: cap 1024; a base payload padded with an XML comment whose
     length is computed at runtime — serialize the unpadded payload
     once (trial serialization through the same
     SecureTransformers.factory() identity transform), then pad with
     a comment of the exact remaining byte count; keep the fixture
     ASCII so chars == bytes
   - action: serialize via the capped path
   - assert: serialization succeeds, output length == 1024
   - expected: fails before step 3
3. `malformedCapEnvFailsLoud`
   - setup: expose the parse as a static `BridgeConfig
     .parseMaxBodyBytes(String raw)` entrypoint (step 1) — JUnit
     cannot set env vars; the `parseCap(raw)` precedent lives at
     SoapEndpointPublisherBodyCapTest:326-333
   - action: call `BridgeConfig.parseMaxBodyBytes("abc")`
   - assert: `IllegalStateException` message contains
     `CXF_MAX_BODY_BYTES` and `abc`
   - expected: fails before step 1 (method absent)

**Acceptance:**
- `./gradlew test` exits 0
- All 3 tests pass; `grep -c "parseMaxBodyBytes"
  bridges/cxf/src/main/java/org/rustcamel/cxf/SoapEndpointPublisher.java`
  ≥ 1 (delegation happened — no duplicated parse)

- [x] 2

## Task 3: cxf-dispatch-cap

**Files:**
- `bridges/cxf/src/main/java/org/rustcamel/cxf/CxfClientManager.java` (modified)
- `bridges/cxf/src/main/java/org/rustcamel/cxf/BridgeConfig.java` (modified)
- `bridges/cxf/src/test/java/org/rustcamel/cxf/CxfClientManagerTest.java` (modified)
- `bridges/cxf/README.md` (modified)

**Spec:** Requirement "CXF dispatch cache is bounded and closed
deterministically" (4 scenarios).

**Steps:**
1. In `BridgeConfig`, add BOTH a static `parseMaxDispatches(String
   raw)` (pure parse: default 64, ceiling 1024; malformed/non-positive/
   over-ceiling → `IllegalStateException` naming env var + raw value;
   ADR-0033 — `0` is invalid) AND a no-arg `parseMaxDispatches()` that
   reads `CXF_MAX_DISPATCHES` and delegates — the same two-overload
   shape as Task 2's `parseMaxBodyBytes`.
2. In `CxfClientManager`, replace the
   `ConcurrentHashMap<DispatchKey, Dispatch<Source>> dispatches` field
   (CxfClientManager.java:33) with a synchronized access-order
   `LinkedHashMap<DispatchKey, Dispatch<Source>>(16, 0.75f, true)`
   wrapped by `Collections.synchronizedMap`. Add the test seam
   mirroring SoapEndpointPublisher's `pinnedMaxBodyBytes`
   (SoapEndpointPublisher.java:49-52): package-private
   `int pinnedMaxDispatches = -1` consulted before the env value —
   this is how the LRU tests set the cap (no env injection). Add
   `private int resolveMaxDispatches()` returning
   `pinnedMaxDispatches` when positive, else
   `BridgeConfig.parseMaxDispatches()` — call it from the existing
   `@PostConstruct init()` (CxfClientManager.java:37) BEFORE bus
   initialization (startup validation, ADR-0033) and again at cache
   insertion (so tests can pin after construction).
3. Rewrite the lookup+create path (CxfClientManager.java:61-79):
   inside one `synchronized (dispatches)` block — `Dispatch<Source>
   existing = dispatches.get(key); if (existing != null) return
   existing;` else create via `createDispatch(...)` (creation inside
   the lock — cold start serialized), then before inserting, while
   `dispatches.size() >= maxDispatches`, evict the LRU entry
   (`iterator().next()` on the access-ordered map), remove it, and
   close it via `closeQuietly(evicted)` (defined below). Insert the
   new entry. The SOAP invoke stays outside the lock (caller-side).
   DEFINE NOW the package-private method
   `void closeQuietly(Dispatch<Source> d)` on CxfClientManager:
   `if (d instanceof org.apache.cxf.jaxws.DispatchImpl<Source> impl)
   impl.close();` wrapped in try/catch logging at FINE (verify the
   exact DispatchImpl close signature against the 4.1.8 jar and
   adapt — the requirement is a deterministic close call, not
   reflection). This is the ONE close implementation: both the
   eviction path and the `@PreDestroy` pass route through it, and it
   is the seam the tests spy/override to record closes.
4. Amend the EXISTING `@PreDestroy void close()`
   (CxfClientManager.java:126-129) — do NOT add a second
   `@PreDestroy` (CDI allows at most one dispose method; two fail
   deployment): under one `synchronized (dispatches)` pass, call
   `closeQuietly(entry)` for every entry and clear the map — routed
   through the same single close implementation as eviction.
5. Eviction logging at FINE: wsdl basename + address + port only —
   never the operation string (untrusted per ADR-0032).
6. README: update the Dispatch cache section — bounded by
   `CXF_MAX_DISPATCHES` (default 64, ceiling 1024, fail-loud), LRU
   eviction with deterministic close; replace the pure
   "route-topology caution" wording with the structural-cap statement
   (the ADR-0032 caution paragraph stays, now noting the cap bounds
   the blast radius).

**Tests (in `CxfClientManagerTest`; the existing tests cover
createDispatch against fake WSDLs — reuse their fixture pattern; for
LRU tests use distinct key fields with the smallest viable fake):**
1. `evictionBoundsCacheAtCap`
   - setup: `pinnedMaxDispatches = 2` (package-private seam, step 2);
     three distinct DispatchKeys
   - action: request dispatch 1, 2, 3
   - assert: `cacheSize() == 2`; request for key 1 (now evicted, since
     1 was LRU) triggers a fresh creation (observable via creation
     counter or mock), request for key 3 returns cached; the evicted
     entry's close fired (via the `closeQuietly` recording seam from
     test 4)
   - command: `./gradlew test --tests 'org.rustcamel.cxf.CxfClientManagerTest'`
   - expected: fails before step 2 (unbounded today)
2. `lruOrderFollowsAccess`
   - setup: cap 2; entries A then B created; then request A again
   - action: request third distinct key C
   - assert: A remains (request for A returns the SAME instance as
     before — identity check via `assertSame`), B was evicted
   - expected: fails before step 2
3. `malformedDispatchCapFailsLoud`
   - setup: static `BridgeConfig.parseMaxDispatches(String raw)`
     entrypoint (same no-env pattern as Task 2 test 3); raw `"0"`
   - action: call it
   - assert: `IllegalStateException` names `CXF_MAX_DISPATCHES` and
     the raw value
   - expected: fails before step 1
4. `closeClosesAllDispatches`
   - setup: cap 64; two entries created; the package-private
     `closeQuietly(Dispatch<Source> d)` method (step 3) spied or
     overridden (Mockito spy on the manager, or a recording subclass)
     to count calls
   - action: call the existing `close()` (`@PreDestroy`)
   - assert: both recorded closes fired; `cacheSize() == 0`
   - expected: fails before step 4 (close() does not close entries
     today)

**Acceptance:**
- `./gradlew test` exits 0
- All 4 tests pass
- `grep -c "ConcurrentHashMap"
  bridges/cxf/src/main/java/org/rustcamel/cxf/CxfClientManager.java` == 0
  (field replaced)

- [x] 3

## Task 4: cxf-namespace-extraction

**Files:**
- `bridges/cxf/src/main/java/org/rustcamel/cxf/SoapEndpointPublisher.java` (modified)
- `bridges/cxf/src/test/java/org/rustcamel/cxf/SoapEndpointPublisherTest.java` (modified)
- `bridges/cxf/src/test/java/org/rustcamel/cxf/SoapEnvelopeHelperTest.java` (modified)

**Spec:** Requirement "CXF inbound body extraction is
namespace-aware" (4 scenarios).

**Steps:**
1. At the `extractSoapBody` callsite (SoapEndpointPublisher.java:252),
   replace the string-scan call with
   `SoapEnvelopeHelper.extractBody(SoapEnvelopeHelper
   .parseResponse(requestXml))` — `parseResponse(String)` already
   exists (SoapEnvelopeHelper.java:41) and is the hardened parse the
   client path uses.
2. Parse failure → HTTP 400: the only existing 4xx classifier is
   WSS-specific (SoapEndpointPublisher.java:312-325 maps
   `WSSecurityException`→400, else 500) — extend that classifier to
   WALK THE CAUSE CHAIN: any `SAXParseException`/
   `ParserConfigurationException` in the chain maps to a 400 response
   whose message names "malformed XML envelope"; forward nothing.
3. Delete `extractSoapBody` (SoapEndpointPublisher.java:464-487) and
   its `<body`/`:body` helper branches entirely.
4. README: inbound-handling section notes the payload is extracted by
   namespace-aware DOM (SOAP 1.1/1.2, any prefix); malformed
   envelopes are rejected 400; a missing Body forwards an empty
   payload.

**Tests:**
1. `prefixedBodyExtractedByLocalName` (SoapEndpointPublisherTest)
   - setup: envelope with `soapenv:` prefix, payload `<p:order
     xmlns:p="urn:t">X</p:order>`
   - action: POST via the publisher's test harness (reuse existing
     handler-level test utilities; if the class tests at method
     level, call the extraction path directly)
   - assert: forwarded payload == serialized `<p:order ...>X</p:order>`
   - command: `./gradlew test --tests '*.SoapEndpointPublisherTest'`
   - expected: passes before AND after (scan handled the plain
     prefixed case — regression pin) — verify it pins the SAME
     serialized form, not a splice
2. `decoyElementNameNotMisExtracted` (SoapEndpointPublisherTest)
   - setup: envelope with UNPREFIXED default-namespace Body —
     `<Envelope xmlns="http://schemas.xmlsoap.org/soap/envelope/"><Body><xsd:bodyData
     xmlns:xsd="http://www.w3.org/2001/XMLSchema">keep</xsd:bodyData></Body></Envelope>`
     (with a prefixed soapenv:Body the scan happens to splice
     correctly — the corruption requires the decoy to win the first
     `indexOf(":body")`, which happens only when Body itself is
     unprefixed)
   - action: run the extraction path
   - assert: forwarded payload EQUALS the intact serialized `xsd:bodyData`
     root element (namespace declaration included) — the exact shape of
     today's mis-spliced fragment is deliberately NOT pinned (two
     reviewers traced it differently); the assertion is equality with
     the intact element, which no mis-splice satisfies
   - expected: FAILS before (the scan returns a spliced fragment —
     `":body"` latches onto the decoy) — this is the red test proving
     rc-q5be; green after via getElementsByTagNameNS local-name
     resolution
3. `missingBodyForwardsEmptyPayload` (SoapEndpointPublisherTest)
   - setup: envelope with Header but no Body element
   - action: run the extraction path
   - assert: forwarded payload is empty string — NOT the whole
     envelope
   - expected: fails before (scan returns the whole envelope when no
     body marker matches)
4. `malformedEnvelopeFails400` (SoapEndpointPublisherTest)
   - setup: request bytes `<not-xml`
   - action: run the handler/parse path
   - assert: HTTP 400 (or the handler's 4xx response object) with
     "malformed" in the message; nothing forwarded
   - expected: fails before (lenient splice forwards garbage)
5. `soap12NamespaceResolves` (SoapEnvelopeHelperTest)
   - setup: envelope document using the SOAP 1.2 namespace
     (`http://www.w3.org/2003/05/soap-envelope`)
   - action: `SoapEnvelopeHelper.extractBody(doc)`
   - assert: payload extracted (helper-level pin for 1.2 local-name
     resolution)
   - expected: passes before AND after (helper already
     namespace-aware — pin it)

**Acceptance:**
- `./gradlew test` exits 0
- `grep -c "extractSoapBody\|\":body\""
  bridges/cxf/src/main/java/org/rustcamel/cxf/SoapEndpointPublisher.java` == 0
- All 5 tests pass (3 red→green: decoy, missing-body, malformed;
  2 regression pins: prefixed-body, soap12)

- [x] 4

## Task 5: cxf-repo-scoping

**Files:**
- `bridges/cxf/build.gradle.kts` (modified)
- `bridges/cxf/README.md` (modified)

**Spec:** none (build hygiene; design.md D5).

**Steps:**
1. In `bridges/cxf/build.gradle.kts` repositories block, keep the
   Shibboleth maven repo but scope it with `exclusiveContent { ...
   forRepository(...) ... filter { includeGroup("org.opensaml");
   includeGroup("net.shibboleth") } }` — Gradle Kotlin DSL form; only
   those two groups may resolve from the nexus.
2. Run `JAVA_HOME=/nix/store/qqngq35hqpiqm5g5w4wgjj2aam09qxif-openjdk-21.0.12+8
   ./gradlew --refresh-dependencies -q dependencies --configuration
   runtimeClasspath` in `bridges/cxf` — resolution must succeed
   (proves OpenSAML/WSS4J still resolve under the filter). If a group
   other than opensaml/shibboleth fails to resolve, STOP and report
   `repo-scope-gap: <failing group>` — do not widen the filter silently.
3. Run `./gradlew build -x test` — compile must pass.
4. README: build/deps note — the Shibboleth nexus serves only
   `org.opensaml`/`net.shibboleth` (OpenSAML 5.x for WSS4J 4.x;
   Central 404s those); all other artifacts resolve from Maven
   Central.

**Tests:**
1. `repoScopeResolution`
   - name: (build-level, not JUnit)
   - setup: exclusiveContent filter applied
   - action: `./gradlew --refresh-dependencies -q dependencies
     --configuration runtimeClasspath`
   - assert: exit 0, no `Could not resolve` lines in output
   - command: as in action
   - expected: passes after step 1-2 (there is no "before" — the
     filter is the change)
2. `compilePassesUnderFilter`
   - action: `./gradlew build -x test`
   - assert: exit 0
   - expected: passes

**Acceptance:**
- Both commands exit 0
- `grep -c "exclusiveContent" bridges/cxf/build.gradle.kts` ≥ 1
- Report states whether resolution used the network or local cache
  (--refresh-dependencies forces re-resolution; if the environment is
  offline, report that explicitly — the filter is still correct but
  the proof is cache-level)

- [x] 5
