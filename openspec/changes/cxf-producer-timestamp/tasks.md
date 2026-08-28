# Tasks: cxf-producer-timestamp

Single-phase change (design.md: one coherent slice, bridge-local).
All Java commands run through the container harness (no JDK on host).
From the repo root, with `BRIDGE=.worktrees/cxf-producer-timestamp/bridges/cxf`:

```
docker run --rm --user=0:0 --volume="$PWD/$BRIDGE":/project:z --workdir=/project \
  --env=GRADLE_USER_HOME=/project/.gradle-docker-cache --tmpfs=/tmp:rw,exec \
  --entrypoint bash quay.io/quarkus/ubi9-quarkus-graalvmce-builder-image:jdk-25 \
  -c 'HOST_UID=$(stat -c "%u" /project); java -cp gradle/wrapper/gradle-wrapper.jar org.gradle.wrapper.GradleWrapperMain <ARGS> --no-daemon; ST=${PIPESTATUS[0]}; chown -R ${HOST_UID}:${HOST_UID} /project 2>/dev/null; exit $ST'
```

`<ARGS>` is e.g. `test --tests "org.rustcamel.cxf.SecurityProfileTest"`.
The full suite command is `<ARGS>` = `test`. Format check: `<ARGS>` = `spotlessCheck` (Spotless runs google-java-format; no googleJavaFormatCheck task exists in Gradle 9.7.1).

Rust gates are N/A for this change (no `.rs` or root `Cargo.toml` diffs).

## Task 1: emit Timestamp before Signature with default coverage

**Files:**
- `bridges/cxf/src/main/java/org/rustcamel/cxf/SecurityProfile.java` (modified)
- `bridges/cxf/src/test/java/org/rustcamel/cxf/SecurityProfileTest.java` (modified)

**Steps:**
1. Jar-verify BEFORE coding (rc-0xze discipline). A fresh worktree has no Gradle cache yet and the container mounts the bridge at `/project`: first populate dependencies via the harness (`<ARGS>` = `dependencies` or a `test` run), then inside the container `find /project/.gradle-docker-cache -name "wss4j-ws-security-*.jar"`, and `javap -p -c -cp <jar> org.apache.wss4j.dom.handler.WSHandlerConstants` (same for `org.apache.wss4j.common.ConfigurationConstants`). Record in the commit body: (a) the value of `WSHandlerConstants.TIMESTAMP`, (b) the separator `ConfigurationConstants.SIGNATURE_PARTS` entries are split on, (c) how the parser resolves the bare keywords `Body` and `Timestamp` (which classes/methods: `RequestPart`/`WSSecurityUtil` equivalents in 4.0.x).
2. In `createOutInterceptor()`: when `containsAction(outActions, "Timestamp")` is true, insert the Timestamp action token into the `actions` list BEFORE the Signature token is added, inside the existing `if (hasText(keystorePath))` guard (smallest diff; Task 3 makes final behavior identical either way) (Timestamp must precede Signature so part resolution sees the materialized element — mirrors `WssSecurityProcessor` L78–83).
3. In the same method: when both Timestamp and Signature are active AND `signatureParts` has no text, set the default `SIGNATURE_PARTS` property to cover SOAP Body and the Timestamp element, using the jar-verified separator and keyword grammar (candidate form `Body;Timestamp`).
4. Extend the existing `LOG.info` to include the effective parts value.

**Tests** (add to `SecurityProfileTest`, following the file's existing interceptor-property extraction pattern):
- `timestampTokenYieldsOrderedActions`
  - setup: builder with keystore (existing test fixture keystore), out-actions "Signature Timestamp"
  - action: `profile.createOutInterceptor()`, extract the ACTION property
  - assert: action string contains Timestamp before Signature (exact string match on jar-verified order, e.g. "Timestamp Signature")
- `defaultPartsCoverBodyAndTimestamp`
  - setup: same profile, no SIGNATURE_PARTS knob
  - action: extract the SIGNATURE_PARTS property
  - assert: equals the jar-verified default form (e.g. "Body;Timestamp")
- `explicitPartsVerbatim`
  - setup: profile with SIGNATURE_PARTS "Body"
  - action: extract SIGNATURE_PARTS
  - assert: exactly "Body", no injected Timestamp entry
- `timestampFreeProfileUnchanged`
  - setup: out-actions "Signature", no parts knob
  - action: extract ACTION and check for SIGNATURE_PARTS key presence
  - assert: ACTION is "Signature" and no SIGNATURE_PARTS property is set

`command`: harness with `test --tests "org.rustcamel.cxf.SecurityProfileTest"`
`expected`: `timestampTokenYieldsOrderedActions` and `defaultPartsCoverBodyAndTimestamp` fail before Steps 2–3 (Timestamp token currently dropped; no default parts). `explicitPartsVerbatim` and `timestampFreeProfileUnchanged` pass BEFORE and AFTER (current behavior already applies explicit parts verbatim and emits no Timestamp) — they pin the regression fence.

**Acceptance:**
- Harness `test --tests "org.rustcamel.cxf.SecurityProfileTest"` exits 0 with the four new tests passing.
- Harness `spotlessCheck` exits 0.
- Commit body states the three jar-verification facts from Step 1.

- [x] 1.1

## Task 2: wire-level proof of covered Timestamp

**Files:**
- `bridges/cxf/src/main/java/org/rustcamel/cxf/SecurityProfile.java` (modified — PW_CALLBACK_REF production fix)
- `bridges/cxf/src/test/java/org/rustcamel/cxf/SecurityProfileWireTest.java` (new)

**Steps:**
1. Create `SecurityProfileWireTest` reusing the DOM-fixture patterns of `WssSecurityProcessorIntegrationTest` (SOAP envelope `Document`, keystore resources) to run the real `WSS4JOutInterceptor.handleMessage` in-process (no network, no server). Scaffolding approach: a direct `WSS4JOutInterceptor.handleMessage()` call does NOT produce signed output (it only queues its POST_PROTOCOL ending interceptor). Build a real `PhaseInterceptorChain` containing `SAAJOutInterceptor` and the profile's `WSS4JOutInterceptor`, back a `SoapMessage` with `new MessageImpl()` and `setContent(SOAPMessage.class, saajMessage)`, then call `chain.doIntercept(msg)`; read back the mutated SAAJ document. Copy the reference-collection helpers (`signedReferenceIds`, `assertCoversBodyAndTimestamp`) from `WssSecurityProcessorIntegrationTest`. Signer wiring (defaults will NOT match the fixture): the profile's default sig user is "clientkey" (`SecurityProfile.java` keystore-builder default) but the fixture keystore alias is `TestKeystoreHelper.KEY_ALIAS` ("alice") — build profiles with the two-arg `sigUser(TestKeystoreHelper.KEY_ALIAS, TestKeystoreHelper.KEYSTORE_PASSWORD)` (the Builder method is `sigUser(String username, String password)` at SecurityProfile.java:396 — no single-arg overload exists) AND add the outbound password callback the producer props currently omit: put `PW_CALLBACK_REF` = the profile's `createPasswordCallback()` (SecurityProfile.java:316 — `ProfilePasswordCallback` prefers `sigPassword` for SIGNATURE usage, falls back to keystore password) in `createOutInterceptor()` when Signature is active (production fix, not test-only).
2. Assert on the OUTPUT document, not on interceptor properties.

**Tests:**
- `signedMessageCarriesCoveredTimestamp`
  - setup: profile out-actions "Signature Timestamp", keystore, no SIGNATURE_PARTS
  - action: process a SOAP Body message through the outbound interceptor
  - assert: output XML contains a `wsu:Timestamp` element; the set of element IDs referenced by `ds:Reference` includes the Body element and the Timestamp element
- `explicitBodyOnlyPartsWire`
  - setup: same profile plus SIGNATURE_PARTS "Body"
  - action: process a message
  - assert: `ds:Reference` covers exactly the Body element; a `wsu:Timestamp` element exists but is NOT referenced
- `timestampFreeWireUnchanged`
  - setup: out-actions "Signature", no parts knob
  - action: process a message
  - assert: no `wsu:Timestamp` in output; references cover the Body only
- `sigPasswordHonoredOnWire`
  - setup: keystore whose key password DIFFERS from the keystore password; profile via `sigUser(TestKeystoreHelper.KEY_ALIAS, <distinct key password>)`
  - action: process a message through the chain
  - assert: signature present and verifiable (the callback supplied `sigPassword`; a keystore-password callback would fail to unlock the key)

`command`: harness with `test --tests "org.rustcamel.cxf.SecurityProfileWireTest"`
`expected`: red if Task 1's property-level grammar assumptions diverge from actual WSS4J wire behavior (separator/keyword mistakes surface here); green otherwise.

**Acceptance:**
- Harness `test --tests "org.rustcamel.cxf.SecurityProfileWireTest"` exits 0.
- Harness `spotlessCheck` exits 0.

- [x] 2.1

## Task 3: fail-loud validation and README

**Files:**
- `bridges/cxf/src/main/java/org/rustcamel/cxf/SecurityProfile.java` (modified)
- `bridges/cxf/src/test/java/org/rustcamel/cxf/SecurityProfileTest.java` (modified)
- `bridges/cxf/README.md` (modified)

**Steps:**
1. Add `private void validateOutboundActions()` to `Builder`, called from `build()` BEFORE `validateSignatureKnobs()`. It reads the RAW `securityActionsOut` field (not `resolveActionsOrDefault()`):
   - Composition rule: raw actions contain Timestamp but not Signature → throw `IllegalArgumentException` naming the Timestamp action and the missing Signature action.
   - Material rule (checked after composition): raw actions contain any of Signature/Encrypt/Timestamp and `keystorePath` has no text → throw `IllegalArgumentException` naming the missing keystore.
2. Reconcile two existing tests that the new validation would otherwise break (repo-wide scan found no others):
   - `SecurityProfileTest` L187 builds out-actions "Signature Encrypt" with NO keystore — the material rule now throws. Add the test keystore fixture to that profile.
   - `SecurityProfileTest` L282 expects the `SIGNATURE_DIGEST_ALGORITHM` knob-validation error, but its profile carries explicit outbound actions so the new checks would fire first with a different message. Remove the explicit actions from that profile — blank actions are raw-exempt and the knob path (which resolves the default action set) still produces the expected error.
3. Extend `SecurityProfileTest` with the validation tests below.
4. README: AMEND the existing `### Timestamp behavior` section (currently L104-106, which claims unconditionally "The timestamp is signed together with the SOAP Body" — false on the interceptor path once explicit SIGNATURE_PARTS exists). Rewrite it to document: the Timestamp action (emission + ordering), the Body+Timestamp coverage default when no SIGNATURE_PARTS is set, the verbatim contract when it is set (operator responsibility), and both fail-loud rules with their exact trigger conditions.

**Tests:**
- `timestampWithoutSignatureRejected`
  - setup: out-actions "Timestamp", keystore present
  - action: `build()`
  - assert: `IllegalArgumentException`, message contains "Timestamp" and "Signature"
- `signatureWithoutKeystoreRejected`
  - setup: out-actions "Signature", no keystore
  - action: `build()`
  - assert: `IllegalArgumentException`, message names the keystore
- `encryptWithoutKeystoreRejected`
  - setup: out-actions "Encrypt", no keystore
  - action: `build()`
  - assert: `IllegalArgumentException`, message names the keystore
- `composedWithoutKeystorePrecedence`
  - setup: out-actions "Signature Timestamp", no keystore
  - action: `build()`
  - assert: `IllegalArgumentException` names the keystore (composition is valid, material check fires)
- `timestampWithoutKeystoreCompositionFirst`
  - setup: out-actions "Timestamp", NO keystore (both rules violated simultaneously)
  - action: `build()`
  - assert: `IllegalArgumentException` names Timestamp and the missing Signature action (composition rule fires), NOT the keystore
- `blankActionsUnaffected`
  - setup: explicitly blank out-actions `.actionsOut("   ")` (whitespace), no keystore
  - action: `build()` then `hasSecurity()`/interceptor factory calls
  - assert: profile builds; `createOutInterceptor()` returns null; no exception

`command`: harness with `test --tests "org.rustcamel.cxf.SecurityProfileTest"`
`expected`: the four rejection tests fail before Step 1 (profiles currently build silently); `blankActionsUnaffected` passes before and after.

**Acceptance:**
- Harness `test` (FULL suite) exits 0 — all pre-existing 117 plus every new test from Tasks 1–3.
- Harness `spotlessCheck` exits 0.
- README section present with the four documented facts.

- [x] 3.1
