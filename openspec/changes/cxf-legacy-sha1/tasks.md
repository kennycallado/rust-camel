# Tasks: cxf-legacy-sha1

<!--
  Phase grouping per blessed design.md `## Phases`. All phases blessed ONCE.
  Java test battery standard command — substitute a unique unit per task, e.g. fleet-370-t13:

  systemd-run --user --scope --collect --unit=fleet-370-t13 -p MemoryMax=12G -p MemorySwapMax=2G -p TasksMax=1024 -p OOMPolicy=kill -- env TMPDIR=/home/shared/tmp/370 nix shell nixpkgs#jdk21_headless -c ./gradlew test --tests '<pattern>' --console=plain

  (`systemctl --user stop fleet-370-t13.scope` afterwards if the unit lingers.)
  Never run cargo or gradle in the main checkout. Scratch under /home/shared/tmp/370/, NEVER /tmp.

  COMMIT MARKERS: the commits that introduce the behaviour change (tasks 1.3 and 1.5) carry
  `fix(cxf)!:` subjects with a body stating the breaking change (inbound default rejects the
  SHA-1 family + everything outside the six modern URIs, JVM and native) and `Bd: rc-cln4q`.
  Task 4.2 only VERIFIES the markers — do not rewrite history in Phase 4.
  PHASE 1 IS KNOWINGLY RED MID-PHASE: tasks 1.3-1.6 leave parts of the full Gradle suite
  failing (blast radius) until 1.7 re-bases it. Each task's battery runs ONLY its own test
  classes; the full suite is the 1.7 gate. Bisect caveat: land Phase 1 as one green commit
  or accept red intermediates deliberately.
-->

## Phase 1: Java bridge core — deterministic provider + per-profile inbound allow-list

### bridges/cxf — SecurityProfile surface

#### Task 1.1: Parse, validate and expose the ALLOWED_* knobs on SecurityProfile

**Files:**
- `bridges/cxf/src/main/java/org/rustcamel/cxf/SecurityProfile.java` (modified)
- `bridges/cxf/src/main/java/org/rustcamel/cxf/SecurityProfileStore.java` (modified)
- `bridges/cxf/src/test/java/org/rustcamel/cxf/SecurityProfileTest.java` (modified)

**Steps:**
1. Add to `SecurityProfile` two nullable String fields + builder methods `allowedSignatureAlgorithms(String csv)` and `allowedDigestAlgorithms(String csv)` (comma-separated absolute URIs, replacement semantics) + getters `allowedSignatureAlgorithmsRaw()` / `allowedDigestAlgorithmsRaw()`.
2. Add constants: `DEFAULT_ALLOWED_SIGNATURE_ALGORITHMS` = {`http://www.w3.org/2001/04/xmldsig-more#rsa-sha256`, `http://www.w3.org/2001/04/xmldsig-more#rsa-sha384`, `http://www.w3.org/2001/04/xmldsig-more#rsa-sha512`}; `DEFAULT_ALLOWED_DIGEST_ALGORITHMS` = {`http://www.w3.org/2001/04/xmlenc#sha256`, `http://www.w3.org/2001/04/xmldsig-more#sha384`, `http://www.w3.org/2001/04/xmlenc#sha512`}; `SHA1_ALGORITHM_URIS` = {`http://www.w3.org/2000/09/xmldsig#rsa-sha1`, `http://www.w3.org/2000/09/xmldsig#dsa-sha1`, `http://www.w3.org/2000/09/xmldsig#hmac-sha1`, `http://www.w3.org/2001/04/xmldsig-more#ecdsa-sha1`, `http://www.w3.org/2000/09/xmldsig#sha1`}.
3. Add effective-list methods: `List<String> effectiveAllowedSignatureAlgorithms()` / `List<String> effectiveAllowedDigestAlgorithms()` returning the parsed csv (trimmed, blank segments rejected at build) or the defaults when unset; and `boolean inboundAllowsSha1()` (any effective list entry in `SHA1_ALGORITHM_URIS`).
4. In `Builder.build()` (or the existing validation site) add, in this order: (a) when an `ALLOWED_*` value is set and is empty/blank/only-separators → `IllegalArgumentException` naming `CXF_PROFILE_<NAME>_ALLOWED_SIGNATURE_ALGORITHMS` (or `..._DIGEST_...`) with the empty-list-allow-all rationale; (b) when set and any segment is not an absolute URI (`URI.create` + `isAbsolute()`) → fail naming the env var; (c) when set and `!(truststorePath != null || keystorePath != null)` (`canVerifyInbound()` false) → fail naming the env var with the silently-inert rationale. Do NOT require `Signature` out-action or a signing keystore (spec exemption). Do NOT reject profiles whose in-actions contain `Signature` or `Encrypt` (every buildable profile has one of them; `Encrypt`-only consumers may opt in).
5. In `SecurityProfileStore`, map sidecar env `CXF_PROFILE_<NAME>_ALLOWED_SIGNATURE_ALGORITHMS` and `CXF_PROFILE_<NAME>_ALLOWED_DIGEST_ALGORITHMS` to the new builder methods, following exactly how `CXF_PROFILE_<NAME>_SIGNATURE_DIGEST_ALGORITHM` is read today.

**Tests:**
- `allowedSignatureAlgorithms_parse_replacesDefault`: builder with `allowedSignatureAlgorithms("http://www.w3.org/2000/09/xmldsig#rsa-sha1,http://www.w3.org/2001/04/xmldsig-more#rsa-sha256")` → `effectiveAllowedSignatureAlgorithms()` → exactly those two URIs in order.
- `allowedDigestAlgorithms_default_isModernSha2`: builder without digest list, keystore set → `effectiveAllowedDigestAlgorithms()` → the three default URIs, no sha1.
- `allowedSignatureAlgorithms_blank_failsConstruction`: `allowedSignatureAlgorithms("  ,  ")` + keystore → build throws, message contains `ALLOWED_SIGNATURE_ALGORITHMS`.
- `allowedSignatureAlgorithms_relativeUri_failsConstruction`: `allowedSignatureAlgorithms("rsa-sha1")` + keystore → build throws naming the env var.
- `allowedSignatureAlgorithms_withoutVerificationMaterial_failsConstruction`: `allowedSignatureAlgorithms(<rsa-sha1 uri>)` on a profile with neither truststore nor keystore → build throws naming the env var.
- `allowedSignatureAlgorithms_exemptFromSigningPreconditions`: verify-only profile (truststore only, `actionsIn("Signature")`, no keystore, no `actionsOut`) + both `ALLOWED_*` set to legacy URIs → build succeeds.
- `inboundAllowsSha1_trueWhenListed`: `allowedDigestAlgorithms(<xmldsig#sha1>)` → `inboundAllowsSha1()` true; default lists → false.
- `inboundAllowsSha1_detectsAllFiveUris`: parametrised over `SHA1_ALGORITHM_URIS` — each URI listed on its axis → true.

**Acceptance:**
- Battery: `... ./gradlew test --tests 'org.rustcamel.cxf.SecurityProfileTest' --console=plain` exits 0 (in a fleet-370 scope, TMPDIR=/home/shared/tmp/370).
- No placeholder text; every new user-visible message names the offending env var.

- [x] 1.1

#### Task 1.2: Shared wrapper processor — pin + algorithm suite

**Files:**
- `bridges/cxf/src/main/java/org/rustcamel/cxf/ProfileSignatureProcessor.java` (new)
- `bridges/cxf/src/main/java/org/rustcamel/cxf/SecurityProfile.java` (modified)
- `bridges/cxf/src/test/java/org/rustcamel/cxf/ProfileSignatureProcessorTest.java` (new)

**Steps:**
1. Add `AlgorithmSuite inboundAlgorithmSuite()` to `SecurityProfile`: new `org.apache.wss4j.common.crypto.AlgorithmSuite`; `addSignatureMethod` for every effective signature entry, `addDigestAlgorithm` for every effective digest entry; set ALL bounds non-rejecting: `setMinimumSymmetricKeyLength(0)`, `setMaximumSymmetricKeyLength(Integer.MAX_VALUE)`, `setMinimumAsymmetricKeyLength(0)`, `setMaximumAsymmetricKeyLength(Integer.MAX_VALUE)`, `setMinimumEllipticCurveKeyLength(0)`, `setMaximumEllipticCurveKeyLength(Integer.MAX_VALUE)`; if `setEncryptionDerivedKeyLength`/`setSignatureDerivedKeyLength` are the only derived-key knobs, leave them unset (they default to unchecked) — javap-confirmed setters exist for the six bounds above.
2. Create `ProfileSignatureProcessor implements org.apache.wss4j.dom.processor.Processor`: final field `AlgorithmSuite suite`; public constructor `ProfileSignatureProcessor(AlgorithmSuite suite)` delegating to the package-private test seam `ProfileSignatureProcessor(AlgorithmSuite suite, Processor delegate)` with `delegate = new SignatureProcessor(new org.apache.jcp.xml.dsig.internal.dom.XMLDSigRI())`; `handleToken(element, requestData)`: if `requestData.getAlgorithmSuite() == null` → `requestData.setAlgorithmSuite(suite)`; then return `delegate.handleToken(element, requestData)`. Class javadoc: why the pin exists (GraalVM native cannot register providers at runtime; JDK provider policy forbids SHA-1), MUST-VERIFY probe result (pin alone sufficient, evidence `/home/shared/tmp/370/probe-evidence.xml`), and why bounds are non-rejecting (EncryptedKey/ReferenceList/EncryptedData/Signature processors read the same suite; order-independence).
3. Keep the class stateless w.r.t. threads beyond the immutable suite (safe for concurrent handleToken calls).

**Tests:**
- `suite_containsExactlyTheEffectiveEntries`: profile with `allowedSignatureAlgorithms(rsa-sha1)` + digest default → `inboundAlgorithmSuite().getSignatureMethods()` = {rsa-sha1}, `getDigestAlgorithms()` = the 3 defaults, `getC14nAlgorithms()`/`getEncryptionMethods()`/`getKeyWrapAlgorithms()` empty.
- `suite_boundsAreNonRejecting`: assert `getMaximumSymmetricKeyLength() == Integer.MAX_VALUE`, `getMinimumSymmetricKeyLength() == 0`, same for asymmetric and EC min/max.
- `wrapper_installsSuiteIntoCleanRequestData`: fresh `RequestData` (suite null) + wrapper built with a RECORDING delegate via the package-private constructor (a `Processor` stub that captures the `requestData` argument and returns an empty list) → invoke `wrapper.handleToken` → the captured `requestData.getAlgorithmSuite()` IS the wrapper's suite (the public-constructor delegate wiring is covered by the 1.3 behavioural tests).
- `wrapper_preservesExistingSuite`: pre-set a different `AlgorithmSuite` on `RequestData` → recording delegate sees the ORIGINAL suite (not overwritten).

**Acceptance:**
- Battery: `./gradlew test --tests 'org.rustcamel.cxf.ProfileSignatureProcessorTest'` (scoped command above) exits 0.
- `cargo` gates N/A (Java-only task).

- [x] 1.2

#### Task 1.3: Consumer path — pinned engine through the engineFactory seam + core inbound tests

**Files:**
- `bridges/cxf/src/main/java/org/rustcamel/cxf/WssSecurityProcessor.java` (modified)
- `bridges/cxf/src/test/java/org/rustcamel/cxf/WssSecurityProcessorLegacySha1Test.java` (new)

**Steps:**
1. In the production constructor `WssSecurityProcessor(SecurityProfile profile)` keep the `engineFactory` seam but change the default from `WSSecurityEngine::new` to a private static `pinnedEngine(profile)`: `WSSecurityEngine engine = new WSSecurityEngine(); WSSConfig cfg = WSSConfig.getNewInstance(); cfg.setProcessor(WSConstants.SIGNATURE, new ProfileSignatureProcessor(profile.inboundAlgorithmSuite())); engine.setWssConfig(cfg); return engine;`
2. Leave the package-private test constructor untouched (existing fixtures keep injecting engines).
3. New test class `WssSecurityProcessorLegacySha1Test` with keystore fixtures from `TestKeystoreHelper.createTestKeystore()` (alias `alice`, password `changeit`), legacy signing profile (`actionsOut("Signature")`, `signatureAlgorithm(rsa-sha1)`, `signatureDigestAlgorithm(sha1)`, `signatureC14nAlgorithm(exclusive c14n)`), verify profile (truststore + `actionsIn("Signature")`).

**Tests:**
- `inboundRejectsRsaSha1ByDefault`: sign rsa-sha1+sha1 via signing profile (which sets the legacy `signatureAlgorithm`/`signatureDigestAlgorithm` knobs explicitly — proving the signing knobs do NOT opt in) → verify with default verify profile → `processInbound` throws `WSSecurityException` with `getErrorCode() == WSSecurityException.ErrorCode.INVALID_SECURITY` (the AlgorithmSuiteValidator rejection; its message is the generic "does not match the requirement" text and the algorithm URI goes only to the WSS4J log — do NOT assert URI text). (This is the spec scenario "legacy signing knob does not opt in to inbound sha1", behavioural half; the WARN half lives in task 1.6.)
- `inboundAcceptsRsaSha1WhenOptedIn`: same signing profile; verify profile with `allowedSignatureAlgorithms(rsa-sha1)` + `allowedDigestAlgorithms(sha1)` → `processInbound` returns non-null signer covering Body.
- `inboundModernUnchanged`: sign rsa-sha256+sha256 → verify with default verify profile → succeeds.
- `inboundSha1WithoutRegisteredProvider`: `Security.removeProvider("ApacheXMLDSig")` if present (record the provider's ORIGINAL position from `Security.getProviders()` first; restore with `Security.insertProviderAt(new XMLDSigRI(), <originalPosition>)` in `finally`) + opted-in verify profile → `processInbound` succeeds (the native simulation — decisive test). Assert `Security.getProvider("ApacheXMLDSig") == null` DURING the call (guard against vacuous pass). Note: `Security` state is JVM-global — if Gradle test forks run classes in parallel, annotate the class to force sequential execution (e.g. `@Isolated`-equivalent via Gradle `maxParallelForks=1` in the test task config if needed).
- `consumerSignerIdentity_preservedForLegacyOptIn`: opted-in rsa-sha1 verify → `VerifiedInboundMessage.signer()` present with the alice certificate fingerprint.

**Acceptance:**
- Battery: `./gradlew test --tests 'org.rustcamel.cxf.WssSecurityProcessorLegacySha1Test'` (scoped) exits 0.
- The commit introducing this behaviour carries `fix(cxf)!:` + the breaking-change body + `Bd: rc-cln4q` (plan-header rule).
- Known blast-radius failures in the task-1.7 file list are EXPECTED at this point (1.3 pins the production constructor used by `WssSecurityProcessorTest.java:49,165-167`); only this task's new class must be green — the full suite is the 1.7 gate.

- [x] 1.3

#### Task 1.4: Extended inbound behaviour tests (3-ref shape, ecdsa, encrypt-only, header orders, construction failures)

**Files:**
- `bridges/cxf/src/test/java/org/rustcamel/cxf/WssSecurityProcessorLegacySha1Test.java` (modified)
- `bridges/cxf/src/test/java/org/rustcamel/cxf/TestKeystoreHelper.java` (modified)

**Steps:**
1. `TestKeystoreHelper`: add an EC-signed certificate generation (`createTestEcKeystore()` returning a JKS/PKCS12 with alias `ec`, EC P-256 key, self-signed or CA-signed like the RSA fixture; reuse the existing cert-generation helpers).
2. Add to the legacy test class: three-reference fixture driver — direct WSS4J (`WSSecHeader` + `WSSecTimestamp` + `WSSecSignature`), mirroring `WssSecurityProcessor.java:126-145`: `sign.setKeyIdentifierType(BST_DIRECT_REFERENCE)`, `sign.setSignatureAlgorithm("http://www.w3.org/2000/09/xmldsig#rsa-sha1")`, `sign.setDigestAlgo("http://www.w3.org/2000/09/xmldsig#sha1")`, `sign.setSigCanonicalization("http://www.w3.org/2001/10/xml-exc-c14n#")`, and parts via `sign.getParts().add(...)` (WSS4J 4.0.1 has NO `setParts(String)`): `new WSEncryptionPart("Body", doc.getDocumentElement().getNamespaceURI(), "Content")` (exactly as `WssSecurityProcessor.java:131` does), `new WSEncryptionPart("Timestamp", WSConstants.WSU_NS, "")`, `new WSEncryptionPart("BinarySecurityToken", WSConstants.WSSE_NS, "")` — so all THREE references carry the sha1 digest. This mirrors the wse-php partner shape (BST + Timestamp + Body in one signature).
3. Header-order driver: signed-then-encrypted AND encrypted-then-signed messages via `WSSecEncrypt` + `WSSecSignature` in both orders (AES-128 + rsa-oaep-mgf1p like `processOutbound`'s encrypt branch).

**Tests:**
- `inboundThreeReferencePartnerShape_verifiesWhenOptedIn`: 3-ref legacy message → opted-in verify profile (`actionsIn("Signature")`, no `Timestamp` in-action) → succeeds; `signer()` present.
- `inboundThreeReferenceTimestampCoverageNotRequired_whenTimestampNotInActions`: a MODERN-algorithm 3-ref message (rsa-sha256/sha256, signed Timestamp) → verify with a default profile whose in-actions are `Signature` only (no `Timestamp`) → succeeds (proves the timestamp-coverage check is skipped when `Timestamp` is not among the in-actions; the legacy-algorithm variant of rejection is already pinned by `inboundRejectsRsaSha1ByDefault`).
- `inboundEcdsaSha256_rejectedByDefault_thenAcceptedWhenAllowListed`: sign with the EC alias (rsa→ecdsa-sha256 + sha256) → default verify profile fails; verify profile with `allowedSignatureAlgorithms(<ecdsa-sha256 uri>)` → succeeds.
- `inboundSignedAndEncrypted_bothHeaderOrders_defaultProfileUnchanged`: both order messages (rsa-sha256/sha256) → default verify profile → verify+decrypt succeed in BOTH orders.
- `encryptOnlyConsumer_optedIn_acceptsSignedEncryptedLegacy`: profile with `actionsIn("Encrypt")` + truststore/keystore + legacy `ALLOWED_*` → rsa-sha1 signed+encrypted message → verify+decrypt succeed.
- construction-failure cases already covered by 1.1 unit tests stay in `SecurityProfileTest` (do not duplicate).

**Acceptance:**
- Battery: `./gradlew test --tests 'org.rustcamel.cxf.WssSecurityProcessorLegacySha1Test' --tests 'org.rustcamel.cxf.SecurityProfileTest'` (scoped) exits 0.
- `TestKeystoreHelper` change keeps all existing keystore consumers green (`./gradlew test --tests 'org.rustcamel.cxf.WssSecurityProcessorTest'` exits 0 EXCEPT the known rsa-sha1 blast-radius failures fixed in 1.7 — coordinate: run this task's tests only, note the rest).

- [x] 1.4

#### Task 1.5: Producer path — in-interceptor PROCESSOR_MAP carries the wrapper

**Files:**
- `bridges/cxf/src/main/java/org/rustcamel/cxf/SecurityProfile.java` (modified)
- `bridges/cxf/src/test/java/org/rustcamel/cxf/SecurityProfileInInterceptorTest.java` (new)

**Steps:**
1. In `SecurityProfile.createInInterceptor()`, after the existing props population, add: `props.put(WSS4JInInterceptor.PROCESSOR_MAP, Map.of(WSConstants.SIGNATURE, new ProfileSignatureProcessor(inboundAlgorithmSuite())))`. RESOLVED FACT (bytecode-verified by plan review, re-verify with one javap step): CXF 4.1.8 `WSS4JInInterceptor.configureCustomProcessors(WSSConfig, Map<QName,Object>)` accepts BOTH `Class` values (`setProcessor(QName, Class)`) and `Processor` INSTANCES (`setProcessor(QName, Processor)` via `instanceof Processor`); `WSConstants.SIGNATURE` is a `QName`, so the `Map.of` above type-checks.
2. Set the property on the non-null path unconditionally (matching the consumer path's always-pin behaviour); the in-interceptor stays null without verification material (unchanged).

**Tests:**
- `inInterceptor_carriesProcessorMapEntryForSignature`: build profile with truststore + `actionsIn("Signature")` → `createInInterceptor()` → cast to `WSS4JInInterceptor` → its properties contain `WSS4JInInterceptor.PROCESSOR_MAP` with key `WSConstants.SIGNATURE` mapping to a `ProfileSignatureProcessor` (read props via the interceptor's public property accessor).
- `inInterceptor_nullWithoutVerificationMaterial`: profile with neither store → `createInInterceptor()` null (unchanged behaviour).
- `inInterceptorWrapper_enforcesSameAllowList`: CXF-Message-level integration test (MANDATORY — this pins the producer-path half of the "both paths" spec scenarios): build a CXF `SoapMessage` whose content is a DOM parsed from the legacy rsa-sha1 fixture bytes, put the profile's `createInInterceptor()` result on it as an interceptor, invoke `WSS4JInInterceptor.handleMessage` → default profile throws/faults (PhaseFault or WSSecurityException surfaced by the interceptor), opted-in profile completes without fault. Reuse the keystore fixtures and the in-memory legacy message construction from the legacy test class (no static fixture needed — build the signed message in-test with the direct WSS4J driver of task 1.4).

**Acceptance:**
- Battery: `./gradlew test --tests 'org.rustcamel.cxf.SecurityProfileInInterceptorTest'` (scoped) exits 0.
- The commit introducing this behaviour carries `fix(cxf)!:` + the breaking-change body + `Bd: rc-cln4q` (plan-header rule).
- `SoapEndpointPublisherTest` failures at this point are expected ONLY from the rsa-sha1 blast radius (fixed in 1.7).

- [x] 1.5

#### Task 1.6: Startup WARNs — SHA-1 allowed + signing-knob hint

**Files:**
- `bridges/cxf/src/main/java/org/rustcamel/cxf/SecurityProfileStore.java` (modified)
- `bridges/cxf/src/test/java/org/rustcamel/cxf/SecurityProfileStoreTest.java` (modified or new)

**Steps:**
1. After profile construction at startup, emit `LOG.warning(...)` when `profile.inboundAllowsSha1()`: message names the profile and states that inbound SHA-1 signature/digest acceptance is ENABLED for it (reference the accepted-risk ADR). Exact wording (assert in test): `"WS-Security profile '<name>' accepts SHA-1 signature/digest algorithms on inbound verification (opted-in via ALLOWED_*; accepted risk, see ADR-0085)"`.
2. Second WARN when the profile has inbound verification material (truststore or keystore), a signing knob (`signatureAlgorithm`/`signatureDigestAlgorithm` raw value) names a SHA-1 URI, and that URI is NOT on the effective inbound allow-list. Exact wording (assert in test, asymmetric-profile-aware per design): `"WS-Security profile '<name>' signs with SHA-1 (<uri>) but does not accept it on inbound verification; set CXF_PROFILE_<NAME>_ALLOWED_SIGNATURE_ALGORITHMS/ALLOWED_DIGEST_ALGORITHMS if legacy partners must be verified (intentional if this asymmetry is deliberate)"`. Only ONE such WARN per profile (join both URIs if both knobs are excluded).
3. Keep WARN emission out of `SecurityProfile.build()` (it is a startup-store concern); expose the two decision points as package-visible pure helpers `sha1AllowedWarning(profile)` / `signingKnobMismatchWarning(profile)` returning `Optional<String>` so tests assert messages without log capture.

**Tests:**
- `sha1AllowedWarning_namesProfileAndStatesAcceptance`: opted-in legacy profile → helper returns EXACTLY `"WS-Security profile 'legacy' accepts SHA-1 signature/digest algorithms on inbound verification (opted-in via ALLOWED_*; accepted risk, see ADR-0085)"` (assertEquals against the full string with the profile name substituted — O3).
- `sha1AllowedWarning_absentForDefaultProfile`: default lists → helper empty.
- `signingKnobMismatchWarning_firesForLegacyKnobWithoutInboundOptIn`: profile signing rsa-sha1 (keystore + `actionsOut("Signature")`), truststore, `actionsIn("Signature")`, no ALLOWED_* → helper returns EXACTLY `"WS-Security profile '<name>' signs with SHA-1 (http://www.w3.org/2000/09/xmldsig#rsa-sha1) but does not accept it on inbound verification; set CXF_PROFILE_<NAME>_ALLOWED_SIGNATURE_ALGORITHMS/ALLOWED_DIGEST_ALGORITHMS if legacy partners must be verified (intentional if this asymmetry is deliberate)"` with `<name>` and `<NAME>` substituted from the profile (assertEquals).
- `signingKnobMismatchWarning_joinsBothUris`: profile with BOTH legacy knobs excluded → single WARN message containing both URIs joined per the helper's format (assertEquals against the exact joined string).
- `signingKnobMismatchWarning_silentWhenAllowListCovers`: same profile WITH legacy ALLOWED_* → empty.
- `signingKnobMismatchWarning_silentWithoutVerificationMaterial`: signing-only profile (no truststore/keystore) → empty.
- store-level: constructing the store with an opted-in profile logs the SHA-1 WARN (attach a `java.util.logging.Handler` to `SecurityProfileStore`'s logger in the test, assert one record; detach in `finally`).

**Acceptance:**
- Battery: `./gradlew test --tests 'org.rustcamel.cxf.SecurityProfileStoreTest'` (scoped) exits 0.
- WARN wording matches the exact strings above (O3 carry-over).

- [x] 1.6

#### Task 1.7: Blast radius — re-base non-legacy inbound fixtures to rsa-sha256

**Files:**
- `bridges/cxf/src/test/java/org/rustcamel/cxf/WssSecurityProcessorIntegrationTest.java` (modified)
- `bridges/cxf/src/test/java/org/rustcamel/cxf/WssSecurityProcessorTest.java` (modified)
- `bridges/cxf/src/test/java/org/rustcamel/cxf/WssSecurityProcessorTimestampCoverageTest.java` (modified)
- `bridges/cxf/src/test/java/org/rustcamel/cxf/SoapEndpointPublisherTest.java` (modified)
- `bridges/cxf/src/test/java/org/rustcamel/cxf/SecurityProfileTest.java` (modified)
- `bridges/cxf/src/test/java/org/rustcamel/cxf/SecurityProfileWireTest.java` (modified)
- `bridges/cxf/src/test/java/org/rustcamel/cxf/SecurityProfileInWireTest.java` (modified)

**Steps:**
1. Locate the shared signed-request builder (`createProcessor` around `WssSecurityProcessorIntegrationTest.java:721`) and switch its signing profile to rsa-sha256 + sha256 (add explicit `signatureAlgorithm`/`signatureDigestAlgorithm` so the change is self-documenting). Tests that SPECIFICALLY exercise the outbound rsa-sha1 default stay on rsa-sha1 but must opt in on the verify side (or assert outbound-only properties without inbound verification).
2. Re-affirm `consumerDefaultsUnchangedWithoutKnobs` (`WssSecurityProcessorIntegrationTest.java:604-629`): it asserts the OUTBOUND default signature algorithm is rsa-sha1 — keep the assertion, adjust only if it also verifies inbound with the same default profile (then split: outbound default assertion stays default; the inbound round-trip opts in).
3. Sweep each file in the list for inbound verification calls that now fail under reject-by-default (grep for `processInbound\(|createInInterceptor\(` — current counts 17/14/4/5/9/1/1 per file; the mission's original estimate was 21/18/7/6/9/1/1): each call either (a) switches its fixture to rsa-sha256/sha256 (preferred — the test's subject is not algorithm choice), or (b) opts the verify profile in when the test's subject IS legacy behaviour (those belong in the new legacy test class instead — move, don't duplicate).
4. DO NOT touch the outbound default: a profile with no knobs still SIGNS with rsa-sha1+sha1 (`WssSecurityProcessorIntegrationTest.java:611-613` assertion untouched).

**Tests:**
- Full battery: `./gradlew test --console=plain` (scoped) exits 0 — this is the phase gate.

**Acceptance:**
- `./gradlew test` (scoped, full suite) exits 0 with NO `@Disabled`/`@Tag` exclusions added.
- `grep -c 'rsa-sha1'` on the modified test files shows legacy URIs only in legacy-subject tests (the new legacy class) or explicit outbound-default assertions.

- [x] 1.7

#### Task 1.8: Fixture un-work-around — truthful comments + generateLegacySha1Fixture (JVM round-trip only)

**Files:**
- `bridges/cxf/src/test/java/org/rustcamel/cxf/SignedRequestFixtureGeneratorTest.java` (modified)

**Steps:**
1. Rewrite the three false comments at `SignedRequestFixtureGeneratorTest.java:75,133,171` ("WSS4J secure validation (default-on) forbids RSA-SHA1; sign with SHA-256") to the truthful statement: JVM-side WSS4J does not forbid RSA-SHA1 (Santuario allows it); the fixtures sign SHA-256 because the BRIDGE DEFAULT inbound allow-list rejects SHA-1 unless opted in (mission 370 / bd rc-cln4q).
2. Add `generateLegacySha1Fixture` as an in-test generator ONLY (no committed fixture file): build the partner shape with the direct WSS4J driver — `WSSecHeader` + `WSSecTimestamp` + `WSSecSignature` with `setKeyIdentifierType(BST_DIRECT_REFERENCE)`, `setSignatureAlgorithm("http://www.w3.org/2000/09/xmldsig#rsa-sha1")`, `setDigestAlgo("http://www.w3.org/2000/09/xmldsig#sha1")`, `setSigCanonicalization("http://www.w3.org/2001/10/xml-exc-c14n#")`, parts via `sign.getParts().add(...)`: `new WSEncryptionPart("Body", doc.getDocumentElement().getNamespaceURI(), "Content")` (the generator owns its doc — same expression as `WssSecurityProcessor.java:131`, never hardcode the SOAP namespace), `new WSEncryptionPart("Timestamp", WSConstants.WSU_NS, "")`, `new WSEncryptionPart("BinarySecurityToken", WSConstants.WSSE_NS, "")`. CONSTRAINT (from the repo's own fixture policy, `SignedRequestFixtureGeneratorTest.java:38-40`): committed fixtures omit Timestamps because a signed Timestamp expires (~300s WSS4J TTL) — a committed 3-ref legacy fixture would be a time bomb; the generator runs fresh each test/repro so the TTL is a non-issue on the JVM side. Round-trip it against an opted-in profile inside the test.
3. Do NOT write any new fixture file to `crates/camel-test/tests/fixtures/` — the Rust e2e consumes runtime-captured bytes, not static fixtures (task 3.1).

**Tests:**
- `generateLegacySha1Fixture_roundTripsAgainstOptedInProfile`: generate (fresh, in-memory) → verify with opted-in profile → succeeds; assert the signature method URI is rsa-sha1 and all three `DigestMethod`s are sha1.
- `commentsAreTruthful`: (compile-time review criterion) no comment in the file claims JVM WSS4J forbids RSA-SHA1.

**Acceptance:**
- Battery: `./gradlew test --tests 'org.rustcamel.cxf.SignedRequestFixtureGeneratorTest'` (scoped) exits 0.
- `git grep 'secure validation (default-on) forbids' -- bridges/cxf/src/test/` returns nothing (scope limited to this task's surface; the `wss_e2e.rs:126` copy is rewritten in task 3.1); `git status` shows NO new fixture files.

- [x] 1.8

## Phase 2: Rust plumbing — camel-cxf options, env emission, schema

### camel-cxf — config surface

#### Task 2.1: Config options + validation exemption

**Files:**
- `crates/components/camel-cxf/src/config.rs` (modified)

**Steps:**
1. Add to the CXF security options struct (the one holding `signature_algorithm`, around `config.rs:225`): `pub allowed_signature_algorithms: Option<String>` and `pub allowed_digest_algorithms: Option<String>` (comma-separated URIs; `Option<String>` consistent with existing `signature_*` knobs), with serde rename/alias following the file's existing snake_case↔kebab-case conventions and the struct's `Debug` impl redaction rules (these are NOT secrets — plain fields, following `signature_algorithm`'s treatment).
2. NOTE THE RESOLVED FACT: there is NO existing Rust-side signing-precondition list to exempt from — the only Rust-side signature enforcement today is `create_consumer`'s SIGNATURE_PARTS rejection (`component.rs:92-101`), and `CxfPoolConfig::validate()` (`config.rs:436-472`) does not check signature knobs. Java (`SecurityProfile.validateSignatureKnobs()`) is the authoritative precondition validator. The "exemption" is therefore implicit: simply do NOT add the new options to any signing precondition — pin it with the exemption test below.
3. Add the new-rule validation in `CxfPoolConfig::validate`'s per-profile loop (`config.rs:454-469`, following the `fault_status` validation example; the fields themselves go on `CxfSecurityFields`, `config.rs:215`): when `allowed_*` is set it must be a non-empty, comma-separated list of absolute URIs (validate non-empty + `://` present — there is NO existing lightweight URI check for `signature_algorithm` in Rust to mirror; Java is the authoritative fail-loud validator for URI absoluteness), and when set, the profile must have verification material meaning `security.keystore_path` OR `security.truststore_path` set — NOT the TLS stores under `security.tls` (mirror of the Java no-verification-material rule with the same definition Java uses).

**Tests:**
- `allowed_signature_algorithms_parses_and_emits`: config TOML with `allowed_signature_algorithms = "http://www.w3.org/2000/09/xmldsig#rsa-sha1,http://www.w3.org/2001/04/xmldsig-more#rsa-sha256"` → parses into the option; Debug output contains the URIs (not redacted).
- `allowed_algorithms_exempt_from_signing_precondition`: profile with `allowed_signature_algorithms` set, NO keystore, NO `security_actions_out` → the new-rule validation passes for a truststore-only (verify-only) profile; assert no validation requires keystore or `security_actions_out` for these options (pins the implicit exemption).
- `allowed_algorithms_without_verification_material_rejected`: option set, neither keystore nor truststore → validation error naming the option.
- `allowed_algorithms_empty_rejected`: `allowed_signature_algorithms = ""` → validation error.
- `allowed_algorithms_non_uri_rejected`: `allowed_signature_algorithms = "rsa-sha1"` → validation error.

**Acceptance:**
- `cargo test -p camel-cxf` exits 0.
- `cargo clippy -p camel-cxf --all-targets -- -D warnings` exits 0; `cargo fmt --check` exits 0 (worktree, never main).

- [x] 2.1

#### Task 2.2: Env emission through CxfProfileEnvVars

**Files:**
- `crates/components/camel-cxf/src/pool.rs` (modified)

**Steps:**
1. Add `allowed_signature_algorithms: Option<String>` and `allowed_digest_algorithms: Option<String>` to `CxfProfileEnvVars` (after `signature_parts`), populate in the mapping fn (around `pool.rs:40-62`) from the security config.
2. In the env-var emission (the sidecar env builder — follow how `CXF_PROFILE_<NAME>_SIGNATURE_PARTS` is emitted): emit `CXF_PROFILE_<NAME>_ALLOWED_SIGNATURE_ALGORITHMS` / `CXF_PROFILE_<NAME>_ALLOWED_DIGEST_ALGORITHMS` only when `Some`, skipping `None` exactly like the optional knobs do.
3. Extend the env-trace (`bridge_config_with_env_trace` + `pool_env_test`) so the new vars appear in the key-only trace (values are not secrets but follow the existing trace pattern for signature knobs).

**Tests:**
- `pool_env_emits_allowed_algorithms_only_when_set`: profile with both options → env map contains the two `CXF_PROFILE_<NAME>_ALLOWED_*` keys with exact values; profile without → keys absent.
- `pool_env_test` (existing) still passes with the extended trace.

**Acceptance:**
- `cargo test -p camel-cxf` exits 0; `cargo clippy -p camel-cxf --all-targets -- -D warnings` exits 0.

- [x] 2.2

#### Task 2.3: Schema regeneration

**Files:**
- schema artefacts regenerated by `cargo xtask schema` (locate the checked-in schema files the command writes — likely under `crates/` or `docs/`; commit exactly what the command produces, nothing else)

**Steps:**
1. Run `cargo xtask schema` in the worktree root; inspect the diff: it must contain ONLY the two new optional fields on the CXF security options.
2. Run `cargo xtask schema --check` to confirm the checked-in state matches.

**Tests:**
- `cargo xtask schema --check` exits 0.

**Acceptance:**
- ERRATUM (found at apply time, r_glm-verified): the schema generator depends only on camel-api + camel-dsl and NEVER modeled CXF security options (the existing `signature_*` knobs are absent from every generated surface) — so the correct outcome is an EMPTY schema diff with `--check` green; the docs surface for the new knobs is task 4.2 (README/ws-soap.md are hand-written).

- [x] 2.3

## Phase 3: Integration evidence — e2e LEG C, published-artifact repro, post-release smoke

### camel-test — e2e

#### Task 3.1: LEG C in wss_e2e.rs + workaround comment rewrite

**Files:**
- `crates/camel-test/tests/cxf_test/wss_e2e.rs` (modified)

**Steps:**
1. Read the existing LEG structure (LEG A/LEG B around `wss_e2e.rs`) and the comment at `wss_e2e.rs:126`; rewrite it truthfully: the fixture signs SHA-256 because the bridge DEFAULT inbound allow-list rejects SHA-1 unless the profile opts in (not because "WSS4J secure validation forbids RSA-SHA1" — false on the JVM).
2. Add LEG C using the HARNESS-NATIVE pattern (RESOLVED FACT: `wss_e2e.rs` consumes runtime-captured bytes, not static fixtures — LEG B verifies "the EXACT bytes the native bridge just signed", `wss_e2e.rs:323-329`; static fixtures live in `crates/camel-test/tests/fixtures/` and feed `signer.rs`, not the e2e): producer profile with NO signature knobs (the outbound default is already rsa-sha1+sha1) → capture the legacy-signed request bytes through the harness → POST them to (a) an opted-in consumer profile (`allowed_signature_algorithms`/`allowed_digest_algorithms` set to the legacy URIs) → expect HTTP 200, and (b) a default consumer profile → expect a WSS fault (SOAP fault, non-200). `wss_security()` (`cxf_test.rs:386-396`) already supplies keystore+truststore; follow the existing legs' route-config and assertion conventions exactly.
3. Keep the test skipped-or-adaptive in the same way existing legs handle a missing bridge binary locally (document the env var the harness needs; CI builds the bridge natively).

**Tests:**
- `wss_e2e_leg_c_opted_in_profile_accepts_legacy_sha1`: opted-in profile + legacy fixture → 200.
- `wss_e2e_leg_c_default_profile_rejects_legacy_sha1`: default profile + legacy fixture → WSS fault.

**Acceptance:**
- `cargo test -p camel-test --test wss_e2e` compiles and the LEG C cases run to the same pass/skip behaviour as existing legs locally (JVM bridge if the harness supports it, otherwise documented skip) — record which mode ran in the task report.
- CI-runability: the test exercises whatever binary path CI provides (`CAMEL_CXF_BRIDGE_BINARY_PATH` or the harness equivalent — verify the actual env var in the harness code) so the native CI build runs it.

- [x] 3.1

#### Task 3.2: Published-artifact repro (evidence, scratch-only)

**Files:**
- scratch scripts under `/home/shared/tmp/370/` (NOT committed)

**Steps:**
1. Download the published `cxf-bridge-v0.8.2` linux-musl artifact using the exact URL shape from `crates/services/camel-bridge/src/download.rs:253` (GitHub releases download path) into `/home/shared/tmp/370/repro/`.
2. Run the 0.8.2 bridge natively with a verify-only profile (truststore from the repo's test PKI, `security_actions_in = "Signature"`), POST a FRESHLY GENERATED legacy rsa-sha1 fixture (regenerate at repro time in scratch via the task-1.8 generator pattern — a signed Timestamp expires ~300s, so a stale fixture would surface a timestamp-expired error instead of the algorithm error), capture: HTTP status (expect 500) + the exact log line (expect `It is forbidden to use algorithm http://www.w3.org/2000/09/xmldsig#rsa-sha1 when secure validation is enabled`).
3. Repeat the POST against the FIXED bridge on the JVM path (gradle-run or jar) with the SAME profile plus the opt-in → expect verification success (200/no-fault).
4. Save raw evidence (status lines, log excerpts, timestamps) to `/home/shared/tmp/370/repro-evidence.md` and summarise in the park report. NO repo files change in this task.

**Tests:**
- Evidence-only: the repro assertions are the deliverable (exact 500 + exact log line against the published artifact; opt-in success against the fixed JVM path).

**Acceptance:**
- `/home/shared/tmp/370/repro-evidence.md` exists with both captures; `git status` in the worktree shows NO changes from this task.

- [x] 3.2

#### Task 3.3: Post-release smoke workflow

**Files:**
- `.github/workflows/published-bridge-smoke.yml` (new)

**Steps:**
1. Create a workflow with `workflow_dispatch` trigger plus `workflow_run` (or `release` published trigger — check which release workflow exists under `.github/workflows/` and chain after it) that: downloads the published bridge artifact using the same URL shape as `download.rs:253`, sets up JDK 21 (nix or setup-java) + Rust toolchain, builds ONLY `camel-test` (no native rebuild), runs the wss_e2e subset (LEG C + one legacy-positive case) against the DOWNLOADED binary.
2. Header comment documenting: this is release verification, NOT a PR gate; it exists because CI tests what we build, never what users download (bd rc-cln4q finding); expected runtime; how to read failures.
3. Keep it small: one job, no matrix, fail-loud on artifact-download errors.

**Tests:**
- Workflow syntax: `python3 -c "import yaml,sys; yaml.safe_load(open('.github/workflows/published-bridge-smoke.yml'))"` exits 0 (structural check only; the workflow itself runs post-release in CI).

**Acceptance:**
- File committed; YAML parses; header documents the release-verification intent; trigger wired to the existing release flow (name it in the comment).

- [x] 3.3

## Phase 4: ADR + docs + closure bookkeeping

### docs

#### Task 4.1: ADR-0085 — legacy SHA-1 inbound opt-in

**Files:**
- `docs/adr/0085-cxf-legacy-sha1-inbound-opt-in.md` (new)

**Steps:**
1. Write ADR-0085 following the repo's ADR format (mirror `docs/adr/0083-artifact-signing-envelope.md` structure): Context (partner report, GEMMA/Atos, 112 buses; native fallback root cause; probe evidence), Decision (per-profile `ALLOWED_*` inbound allow-list, replacement semantics, OFF by default with the six modern URIs; provider pinning via `SignatureProcessor(XMLDSigRI)` on both inbound paths; WARN on SHA-1 acceptance; WARN on excluded legacy signing knob with asymmetric-profile wording), Accepted risks (SHA-1 collision weakness on opted-in profiles only; inbound rejection of everything outside the six modern URIs incl. ecdsa/rsa-pss/hmac/sha224 families; Encrypt-only consumers affected; keystore-only profiles: the producer in-interceptor stays null so the opt-in is consumer-path-only for that configuration), Rejected alternatives (global secureValidationPolicy weakening; global secure_validation=false; signing-knob-as-opt-in; JVM-only fix), Consequences (breaking change + migration; WARN-vs-INFO decision recorded: WARN chosen, asymmetric profiles are expected noise by design).
2. Cross-reference ADR-0033 (fail-closed defaults preserved) and ADR-0078 (residual-limitations list unchanged; this is a new explicit opt-in, not a silent residual). In Consequences, record: (a) the WARN-vs-INFO decision — WARN chosen, intentionally asymmetric profiles (sign SHA-1 outbound, accept only SHA-256 inbound) are expected WARN noise by design; (b) a SYMMETRIC default profile (no knobs at all) signs rsa-sha1 outbound but triggers NO mismatch WARN, because the mismatch WARN keys off explicit raw knob values — matches the blessed spec; partners must not read silence as acceptance.
3. Update `docs/adr/README.md` or the ADR index if one exists (check `ls docs/adr/*.md | head` for an index convention).

**Tests:**
- `cargo xtask lint-context-citations` exits 0 (docs lint).

**Acceptance:**
- ADR exists, names the flag surface exactly (`CXF_PROFILE_<NAME>_ALLOWED_SIGNATURE_ALGORITHMS` / `..._DIGEST_...`), states the default list, references 0033 + 0078 + bd rc-cln4q; index updated if the convention exists.

- [x] 4.1

#### Task 4.2: README + ws-soap.md + changelog

**Files:**
- `bridges/cxf/README.md` (modified)
- `docs/src/components/ws-soap.md` (modified)

**Steps:**
1. README L31: change "Applied on BOTH producer requests and consumer signed responses." to "Controls signing only (producer requests and consumer signed responses). Inbound verification rejects SHA-1 signature and digest algorithms unless the profile sets `CXF_PROFILE_<NAME>_ALLOWED_SIGNATURE_ALGORITHMS`/`ALLOWED_DIGEST_ALGORITHMS`."
2. README Properties table: add the two new knob rows (env form + property form + default column showing the six-URI default + "SHA-1 rejected by default").
3. README legacy example (CAT112/Baleares section): add both knobs set to the legacy URIs to the properties block, plus an R5404 note. ERRATUM (apply-time, r_glm review 4.2): the originally prescribed sentence "Inbound verification also requires exclusive c14n (BSP R5404)" claimed bridge enforcement that does not exist — the delivered truthful wording is "The legacy partner signs with exclusive c14n (BSP R5404-conformant). The allow-list scopes algorithms only; the bridge does not constrain canonicalization."
4. README: breaking-change note (default rejects the SHA-1 family and everything outside the six modern URIs on JVM and native; migration = set the knobs); note that on a keystore-only profile (no truststore) the producer in-interceptor is null so the opt-in is consumer-path-only (O1 carry-over).
5. `docs/src/components/ws-soap.md`: mirror the two new options with the same defaults/exemption/breaking-change wording.
6. Changelog discipline (RESOLVED FACT: this repo has NO CHANGELOG.md — release notes are generated from Conventional Commits by `cargo xtask changelog` (`scripts/xtask/src/changelog.rs`), breaking detection via the `!:` subject marker): the merge commits for this change carry `fix!:`/`feat!:` subjects whose bodies state the breaking change (inbound default rejects the SHA-1 family + everything outside the six modern URIs, both JVM and native) and reference `Bd: rc-cln4q`. Verify the bodies comply with the caveman-commit convention (AGENTS.md).

**Tests:**
- `cargo xtask lint-single-source` exits 0 (docs duplication lint, if it covers these files).
- Commit-lint path: `cargo xtask changelog --check --from <base> --to HEAD` is CI's gate (runs against FETCH_HEAD per AGENTS.md); locally verify the `!:` marker + body presence on the commits made so far via `git log --format='%s%n%b'`.

**Acceptance:**
- Both docs name the flag surface, defaults, exemption, breaking change, R5404 note; `lint-single-source` exits 0; merge-commit subjects carry the `!:` breaking marker with the breaking-change body.

- [x] 4.2

#### Task 4.3: Closure bookkeeping — native verification gate on bd

**Files:**
- `openspec/changes/cxf-legacy-sha1/tasks.md` (this file — final checkbox)
- scratch: park-report draft under `/home/shared/tmp/370/`

**Steps:**
1. Verify every Phase 1-3 task is checked; run the FULL gate set (AGENTS.md `## QUALITY GATES` minus `lint-commits`, plus `cargo build --workspace`, `cargo test --workspace --lib`, hexagonal test, doc gate with touched crates).
2. Record in the park-report draft: rc-cln4q stays OPEN after merge — closure requires the native LEG C run (CI native build or the published-bridge smoke workflow) showing opted-in → 200 and default → WSS fault (O4 carry-over: this is its own gate, NOT folded into Phase 3 exit criteria).
3. Enumerate the deferrals ledger (scratch): entries for (a) native fix verification deferred to CI, (b) partner real-payload interop (test PKI only proves behaviour; GEMMA payload never shared), (c) asymmetric-profile WARN wording validation with the partner, (d) anything else discovered during implementation.

**Tests:**
- All gates green (recorded per-gate in the park report).

**Acceptance:**
- This checkbox is the LAST one flipped; park report draft complete.

- [x] 4.3
