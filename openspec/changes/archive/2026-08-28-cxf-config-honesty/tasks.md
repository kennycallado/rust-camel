# Tasks: cxf-config-honesty

Test harness conventions used below:
- `HARNESS(gradle-args)` = the containerized gradle invocation over
  `bridges/cxf`: `docker run --rm --user=0:0 --volume=<repo>/bridges/cxf:/project:z --workdir=/project --env=GRADLE_USER_HOME=/project/.gradle-docker-cache --tmpfs=/tmp:rw,exec --entrypoint bash quay.io/quarkus/ubi9-quarkus-graalvmce-builder-image:jdk-25 -c 'HOST_UID=$(stat -c "%u" /project); java -cp gradle/wrapper/gradle-wrapper.jar org.gradle.wrapper.GradleWrapperMain <gradle-args> --no-daemon; ST=${PIPESTATUS[0]}; chown -R ${HOST_UID}:${HOST_UID} /project 2>/dev/null; exit $ST'`
- Rust commands run from the worktree root.

## Task 1: profile validation + producer-path application

**Files:**
- `bridges/cxf/src/main/java/org/rustcamel/cxf/SecurityProfile.java` (modified)
- `bridges/cxf/src/test/java/org/rustcamel/cxf/SecurityProfileTest.java` (modified)

**Steps:**
1. In `SecurityProfile.Builder.build()`, add a `validateSignatureKnobs()`
   step invoked before construction that throws
   `IllegalArgumentException` naming the offending env var when:
   (a) any of `signatureAlgorithm`/`signatureDigestAlgorithm`/
   `signatureC14nAlgorithm`/`signatureParts` is non-blank while
   `resolveActionsOut()` (default `"Signature"`) does not contain a
   `Signature` action; (b) any knob is non-blank while `keystorePath`
   is blank; (c) `signatureParts` fails the strict grammar — `;`-
   separated segments, each either a bare non-empty localName (no
   braces) or `{modifier}{namespace}localName` with modifier empty or
   exactly `Element`/`Content`, namespace possibly empty, localName
   non-empty; (d) an algorithm knob is not an absolute URI
   (`java.net.URI#create` + `isAbsolute()`, any scheme).
2. In `createOutInterceptor()`, inside the `Signature` action block,
   after the existing `SIG_KEY_ID` put: when
   `hasText(signatureAlgorithm)` put `ConfigurationConstants.SIG_ALGO`;
   when `hasText(signatureDigestAlgorithm)` put
   `ConfigurationConstants.SIG_DIGEST_ALGO`; when
   `hasText(signatureC14nAlgorithm)` put
   `ConfigurationConstants.SIG_C14N_ALGO`; when
   `hasText(signatureParts)` put `ConfigurationConstants.SIGNATURE_PARTS`
   (all four constants live in `org.apache.wss4j.common.ConfigurationConstants`,
   NOT `WSHandlerConstants`; the parts constant is `SIGNATURE_PARTS`).
   Extend the existing INFO log line to list which knobs were applied.
3. Add a package-private static helper
   `static void validateSignaturePartsSyntax(String parts)` in
   `SecurityProfile` implementing grammar (c) — reused by the builder
   validation and unit-tested directly.

**Tests** (in `SecurityProfileTest.java`):
- `knobWithoutSignatureActionIsRejected`
  - setup: four `Builder`s with keystore set and
    `actionsOut("Encrypt")`, one per knob:
    `signatureAlgorithm(<rsa-sha384 URI>)`,
    `signatureDigestAlgorithm("http://www.w3.org/2001/04/xmlenc#sha384")`,
    `signatureC14nAlgorithm(<c14n URI>)`, `signatureParts("Body")`
  - action: `build()` per builder
  - assert: `IllegalArgumentException` per case whose message names the
    corresponding env var (`SIGNATURE_ALGORITHM`,
    `SIGNATURE_DIGEST_ALGORITHM`, `SIGNATURE_C14N_ALGORITHM`,
    `SIGNATURE_PARTS`)
- `knobWithoutKeystoreIsRejected`
  - setup: `Builder` with `actionsOut("Signature")`,
    `signatureDigestAlgorithm("http://www.w3.org/2001/04/xmlenc#sha384")`,
    no keystore
  - action: `build()`
  - assert: `IllegalArgumentException` naming `SIGNATURE_DIGEST_ALGORITHM`
- `malformedPartsSegmentIsRejected`
  - setup: builders WITH keystore (`TestKeystoreHelper`) and
    `actionsOut("Signature")`, `signatureParts` set to each of
    `{}{http://x}` (empty localName — nothing after the namespace
    brace),
    `{Bogus}{http://x}Body` (bad modifier), `{Content}{http://x}` (empty
    localName via empty tail), `{Content}http://x}Body` (unbalanced
    braces)
  - action: `build()` per case
  - assert: `IllegalArgumentException` naming `SIGNATURE_PARTS` in every
    case
- `nonUriAlgorithmIsRejected`
  - setup: `signatureAlgorithm("not a uri")`, valid keystore+action
  - action: `build()`
  - assert: `IllegalArgumentException` naming `SIGNATURE_ALGORITHM`
- `validPartsFormsAreAccepted`
  - setup: builders WITH keystore and `actionsOut("Signature")`,
    `signatureParts` set to each of `Body`,
    `{Content}{http://schemas.xmlsoap.org/soap/envelope/}Body`,
    `{}{http://x}Body` (empty modifier is valid),
    `{}{}Timestamp`, compound `Body;{}{http://docs.oasis-open.org/wss/2004/01/oasis-200401-wss-wssecurity-utility-1.0.xsd}Timestamp`
  - action: `build()` per case
  - assert: no exception; accessor returns the input verbatim
- `producerInterceptorAppliesSignatureKnobs`
  - setup: profile via builder with keystore (`TestKeystoreHelper`),
    actions `Signature`, all four knobs set
    (`rsa-sha384` algo, sha-384 digest, exclusive c14n URI,
    `{Content}{http://x}Body`-style parts)
  - action: call `createOutInterceptor()`; cast the result to
    `WSS4JOutInterceptor` and read `getProperties()`
  - assert: entries under the literal WSS4J keys `signatureAlgorithm`,
    `signatureDigestAlgorithm`, `signatureC14nAlgorithm`,
    `signatureParts` equal the four configured values (VERIFIED from
    wss4j 4.0.1 jar at implementation time: `SIG_C14N_ALGO`'s runtime
    string is `signatureC14nAlgorithm` — NOT the legacy long form an
    earlier plan round assumed)
- `producerInterceptorOmitsUnsetKnobs`
  - setup: profile with keystore (`TestKeystoreHelper`), actions
    `Signature`, no knobs
  - action: `createOutInterceptor()` properties inspection
  - assert: none of the four literal keys are present
- command: `HARNESS(test --tests "org.rustcamel.cxf.SecurityProfileTest")`
- expected: all named tests fail before steps 1–3 land, pass after.

**Acceptance:**
- `HARNESS(test --tests "org.rustcamel.cxf.SecurityProfileTest")` exits 0.
- `HARNESS(spotlessCheck)` exits 0.
- Coverage note (per design.md §Approach, WSS4J-as-semantic-authority
  decision): scenarios "algorithm lands on the producer out-interceptor"
  and "parts land on the producer out-interceptor" are covered at
  interceptor-property level (literal WSS4J keys carry the configured
  values); the
  emitted-signature behavior behind those keys is WSS4J's documented
  contract. Scenarios covered behaviorally here: "knob without matching
  action aborts construction", "malformed knob values abort
  construction", "unset knobs preserve defaults" (producer side).

- [x] 1

## Task 2: consumer-path application + PARTS prohibition

**Files:**
- `bridges/cxf/src/main/java/org/rustcamel/cxf/WssSecurityProcessor.java` (modified)
- `bridges/cxf/src/test/java/org/rustcamel/cxf/WssSecurityProcessorIntegrationTest.java` (modified)
- `crates/components/camel-cxf/src/component.rs` (modified)

**Steps:**
1. In `WssSecurityProcessor` constructor: if
   `profile.signatureParts()` is non-null and non-blank, throw
   `IllegalStateException` with a message containing `SIGNATURE_PARTS`
   and `Body+Timestamp` (defense-in-depth; unreachable via validated
   Rust path).
2. In `processOutbound()` inside the `Signature` block, before
   `sign.build(profile.getSignatureCrypto())`: when
   `hasText(profile.signatureAlgorithm())` call
   `sign.setSignatureAlgorithm(profile.signatureAlgorithm())`; likewise
   `sign.setDigestAlgo(profile.signatureDigestAlgorithm())` and
   `sign.setSigCanonicalization(profile.signatureC14nAlgorithm())`
   (exact WSS4J 4.x names — verified: `setDigestAlgo`,
   `setSigCanonicalization`; there is no `setDigestAlgorithm`/
   `setCanonicalizationAlgorithm` on `WSSecSignature`), each guarded by
   hasText. Do NOT touch the `parts` list.
3. In Rust `crates/components/camel-cxf/src/component.rs`
   `create_consumer` (the profile is resolved there via
   `self.resolve_profile()`): if the resolved profile's
   `security.signature_parts` is `Some` and non-empty (field path:
   `CxfProfileConfig::security: CxfSecurityFields` →
   `.security.signature_parts`), return an error (existing component
   error type) whose message contains `signature_parts`, the literal
   `SIGNATURE_PARTS`, and `Body+Timestamp replay invariant`.
4. Rust unit test at the bottom of `component.rs` (existing
   `#[cfg(test)]` module): `create_consumer_rejects_parts_profile`.

**Tests:**
- Java `consumerAppliesDigestAlgorithm` (in
  `WssSecurityProcessorIntegrationTest`, keystore via
  `TestKeystoreHelper`):
  - setup: profile with actions `Signature Timestamp`,
    `signatureDigestAlgorithm` = the sha-384 digest URI
    (`http://www.w3.org/2001/04/xmlenc#sha384`)
  - action: `processOutbound(envelope)` on a SOAP 1.1 envelope; parse the
    result
  - assert: signature `DigestMethod` Algorithm equals the sha-384 URI;
    signature references include the envelope Body and the Timestamp
    (coverage unchanged)
- Java `consumerAppliesSignatureAlgorithm`:
  - setup: same profile with `signatureAlgorithm` = rsa-sha384 URI
  - action: `processOutbound(envelope)`; parse result
  - assert: `SignatureMethod` Algorithm equals rsa-sha384 URI
- Java `consumerAppliesCanonicalizationAlgorithm`:
  - setup: same profile with `signatureC14nAlgorithm` = the exclusive
    c14n URI (`http://www.w3.org/2001/10/xml-exc-c14n#`)
  - action: `processOutbound(envelope)`; parse result
  - assert: signature `CanonicalizationMethod` Algorithm equals the
    exclusive c14n URI
- Java `consumerDefaultsUnchangedWithoutKnobs`:
  - setup: profile with actions `Signature Timestamp`, no knobs
  - action: `processOutbound(envelope)`; parse result
  - assert: default `SignatureMethod` (rsa-sha1 in this WSS4J build —
    verified at implementation; the task's earlier rsa-sha256 guess was
    wrong) and Body+Timestamp coverage present
- Java `consumerRefusesPartsProfile`:
  - setup: builder with keystore, actions `Signature`,
    `signatureParts("Body")` (valid syntax — rejection is by PATH, not
    grammar)
  - action: `new WssSecurityProcessor(profile)`
  - assert: `IllegalStateException` whose message contains BOTH
    `SIGNATURE_PARTS` and `Body+Timestamp`
- Rust `create_consumer_rejects_parts_profile`:
  - setup: component config with a consumer endpoint whose selected
    profile sets `signature_parts = "Body"`
  - action: call `create_consumer`
  - assert: returned error message contains ALL of `signature_parts`,
    `SIGNATURE_PARTS`, and `Body+Timestamp replay invariant`
- commands:
  `HARNESS(test --tests "org.rustcamel.cxf.WssSecurityProcessorIntegrationTest")`,
  `cargo test -p camel-component-cxf --lib create_consumer_rejects_parts_profile`
- expected: all fail before steps 1–4 land, pass after.

**Acceptance:**
- `HARNESS(test --tests "org.rustcamel.cxf.WssSecurityProcessorIntegrationTest")` exits 0.
- `cargo test -p camel-component-cxf --lib` exits 0.
- `cargo clippy -p camel-component-cxf -- -D warnings` exits 0.
- `cargo fmt --check -p camel-component-cxf` exits 0.
- Scenarios covered: "algorithm takes effect on signed consumer
  responses", "parts-configured profile cannot serve a consumer
  endpoint", "unset knobs preserve defaults" (consumer side).

- [x] 2

## Task 3: DispatchKey race fix + service-layer cleanup

**Files:**
- `bridges/cxf/src/main/java/org/rustcamel/cxf/CxfClientManager.java` (modified)
- `bridges/cxf/src/main/java/org/rustcamel/cxf/CxfBridgeService.java` (modified)
- `bridges/cxf/src/test/java/org/rustcamel/cxf/CxfClientManagerTest.java` (modified)

Test mechanism (explicit — no doubles exist today): extend the existing
mockito setup (the class already mocks `BridgeConfig` +
`SecurityProfileStore`) with `mockStatic(Service.class)` (mockito-core
5.x, already on the test classpath): the static mock returns a mock
`Service` whose `createDispatch(portQName, Source.class, Service.Mode.PAYLOAD)` returns a mockito-mock
`Dispatch<Source>` whose `getRequestContext()` returns a REAL
`java.util.HashMap<String, Object>` (fresh map per mock). `getDispatch`
calls then exercise the real cache + creation path against inspectable
contexts. No WSDL fixture, no network.

**Steps:**
1. Add `private record DispatchKey(String wsdl, String address, String
   service, String port, String profile, String operation, long
   timeoutMs)` in `CxfClientManager`; change the cache map to
   `ConcurrentHashMap<DispatchKey, Dispatch<Source>>`; delete the `#`-
   concatenated key.
2. Change `getDispatch(String wsdl, String address, String service,
   String port, String operation, String profileName, long timeoutMs)`
   signature to accept the normalized timeout; build `DispatchKey`
   (operation normalized:
   `operation == null || operation.isBlank() ? "" : operation`).
3. Move the soapaction writes into `createDispatch` (needs the
   operation): inside creation, when operation non-blank put
   `soapaction.use=TRUE` and `soapaction.uri=operation`; also put both
   timeout properties from the key's `timeoutMs` instead of reading
   `bridgeConfig` at creation.
4. In `CxfBridgeService`, replace the two post-cache timeout `put`s with
   computing `normalizedTimeout = request.getTimeoutMs() > 0 ?
   request.getTimeoutMs() : bridgeConfig.connectionTimeoutMs()` and
   passing it to the new `getDispatch` signature. No other
   request-context writes remain in the service.
5. Keep `cacheSize()` semantics (entry count).

**Tests** (in `CxfClientManagerTest.java`, all using the mockStatic
mechanism above):
- `operationParticipatesInCacheKey`
  - setup: static-mocked Service returning fresh mock Dispatches
  - action: `getDispatch` twice — same tuple with blank operation, then
    operation `opX`; a third call with whitespace-only operation
    `"   "` on the blank key
  - assert: `cacheSize() == 2` (whitespace normalizes to the blank key)
- `timeoutParticipatesInCacheKey`
  - setup: same mechanism
  - action: `getDispatch` with timeout T, then same tuple + operation
    with timeout T2 != T
  - assert: `cacheSize() == 2`; second Dispatch context
    `receiveTimeout == String.valueOf(T2)`; first entry's context still
    `String.valueOf(T)` (re-read via a same-key lookup)
- `soapActionSetAtCreationNotAfterLookup`
  - setup: same mechanism, capture the created Dispatch's context map
  - action: `getDispatch` with operation `opA`; snapshot the map; call
    `getDispatch` again with the SAME key
  - assert: after the first call `soapaction.uri == "opA"`; the second
    call leaves the map untouched (equals snapshot)
- `concurrentDistinctOperationsDoNotCrossContaminate`
  - setup: same mechanism on the MAIN thread to pre-seed both cache
    entries (`opA`/`opB` on the same tuple) via `getDispatch`; then
    two worker threads that ONLY perform cache-hit `getDispatch`
    lookups with their own operation and read `soapaction.uri` from the
    returned Dispatch's context — no `createDispatch` (hence no
    `Service.create`) runs on workers (MockedStatic is thread-confined)
  - action: each worker loops 200× (lookup + read); join both
  - assert: every read equals the calling thread's own operation (0
    crossings across all iterations)
- command: `HARNESS(test --tests "org.rustcamel.cxf.CxfClientManagerTest")`
- expected: new tests fail before steps 1–4, pass after.

**Acceptance:**
- `HARNESS(test --tests "org.rustcamel.cxf.CxfClientManagerTest")` exits 0.
- `grep -nE "jakarta.xml.ws.client|getRequestContext" bridges/cxf/src/main/java/org/rustcamel/cxf/CxfBridgeService.java` returns no matches.
- `HARNESS(spotlessCheck)` exits 0.
- Scenarios covered: "concurrent distinct operations do not
  cross-contaminate", "differing timeouts get distinct dispatches", "no
  mutation after publish".

- [x] 3

## Task 4: operator documentation

**Files:**
- `bridges/cxf/README.md` (modified)

**Steps:**
1. In the security/profiles section, document the four signature knobs
   (`SIGNATURE_ALGORITHM`, `SIGNATURE_DIGEST_ALGORITHM`,
   `SIGNATURE_C14N_ALGORITHM`, `SIGNATURE_PARTS`) with: canonical
   PARTS grammar (`{modifier}{namespace}localName` or bare localName,
   `;`-separated), that algorithms apply on BOTH producer requests and
   consumer signed responses, and that algorithm values are absolute
   URIs validated at startup (WSS4J remains the authority for support).
2. Document the two fail-loud rules: knobs without a `Signature`
   action or without a signing keystore abort profile construction;
   `SIGNATURE_PARTS` on a consumer endpoint fails Rust endpoint
   construction because consumer coverage (Body+Timestamp) is the
   fixed replay-defense invariant.
3. In the Dispatch-cache section, document that the cache is keyed by
   operation and request timeout (cardinality grows per distinct
   operation/timeout pair, bounded by route config).

**Tests:**
- name: docs-task-4 (documentation-only; no automated test)
  - setup: README updated per steps 1-3
  - action: manual review of the rendered section
  - assert: all four env var names, the grammar, both fail-loud rules,
    and the cache-key change appear verbatim
  - command: none (docs-only)
  - expected: n/a

**Acceptance:**
- `grep -c "SIGNATURE_PARTS" bridges/cxf/README.md` returns >= 3.
- `grep -c "Body+Timestamp" bridges/cxf/README.md` returns >= 1.

- [x] 4
