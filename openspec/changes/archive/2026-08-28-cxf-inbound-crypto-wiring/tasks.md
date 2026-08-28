# Tasks: cxf-inbound-crypto-wiring

Single-phase change. All Java commands run through the container harness
(no JDK on host). From the repo root, with
`BRIDGE=.worktrees/cxf-inbound-crypto-wiring/bridges/cxf`:

```
docker run --rm --user=0:0 --volume="$PWD/$BRIDGE":/project:z --workdir=/project \
  --env=GRADLE_USER_HOME=/project/.gradle-docker-cache --tmpfs=/tmp:rw,exec \
  --entrypoint bash quay.io/quarkus/ubi9-quarkus-graalvmce-builder-image:jdk-25 \
  -c 'HOST_UID=$(stat -c "%u" /project); java -cp gradle/wrapper/gradle-wrapper.jar org.gradle.wrapper.GradleWrapperMain <ARGS> --no-daemon; ST=${PIPESTATUS[0]}; chown -R ${HOST_UID}:${HOST_UID} /project 2>/dev/null; exit $ST'
```

`<ARGS>` is e.g. `test --tests "org.rustcamel.cxf.SecurityProfileTest"`.
Full suite: `<ARGS>` = `test`. Format gate: `<ARGS>` = `spotlessCheck`.
Rust gates are N/A for this change (no `.rs` or root `Cargo.toml` diffs).

## Task 1: inbound crypto ref-id wiring with property-shape tests

**Files:**
- `bridges/cxf/src/main/java/org/rustcamel/cxf/SecurityProfile.java` (modified)
- `bridges/cxf/src/test/java/org/rustcamel/cxf/SecurityProfileTest.java` (modified)

**Steps:**
1. Add constant `private static final String DEC_CRYPTO_REF_ID = "decCryptoProperties";` beside `ENC_CRYPTO_REF_ID` (~L50).
2. In `createInInterceptor()` Signature branch (~L304-306): replace the direct-Properties put with `props.put(ConfigurationConstants.SIG_PROP_REF_ID, SIG_CRYPTO_REF_ID); props.put(SIG_CRYPTO_REF_ID, createCryptoProperties(truststorePath, truststorePassword));`
3. In the Encrypt branch (~L310-312): same shape with `DEC_PROP_REF_ID` / `DEC_CRYPTO_REF_ID` and keystore crypto. Leave `PW_CALLBACK_REF` unchanged.
4. Extend the inbound interceptor's `LOG.info` to include the ref-ids in use.

**Tests** (add to `SecurityProfileTest`, existing extraction pattern):
- `inboundSigCryptoUsesRefIdShape`
  - setup: truststore-only profile with in-actions "Signature"
  - action: `createInInterceptor()`, extract properties
  - assert: `SIG_PROP_REF_ID` value is the String `"sigCryptoProperties"`; the props map contains that key with a `Properties` value
- `inboundDecCryptoUsesRefIdShape`
  - setup: profile with keystore + in-actions "Encrypt"
  - action: extract properties
  - assert: `DEC_PROP_REF_ID` value is the String `"decCryptoProperties"`; the props map contains that key with a `Properties` value

`command`: harness `test --tests "org.rustcamel.cxf.SecurityProfileTest"`
`expected`: both fail before Steps 2–3 (current values are `Properties` instances, not Strings).

**Acceptance:**
- Harness `test --tests "org.rustcamel.cxf.SecurityProfileTest"` exits 0.
- Harness `spotlessCheck` exits 0.

- [x] 1.1

## Task 2: inbound wire proof and README

**Files:**
- `bridges/cxf/src/test/java/org/rustcamel/cxf/SecurityProfileInWireTest.java` (new)
- `bridges/cxf/README.md` (modified)

**Steps:**
1. Create `SecurityProfileInWireTest` copying the chain scaffolding of `SecurityProfileWireTest` (real `PhaseInterceptorChain`, `setInterceptorChain` before `doIntercept`, SAAJ content, assertions on the output DOM). Two chains per case: produce a secured document with the producer's OUTBOUND interceptor chain (verified working), then process it through the profile's `WSS4JInInterceptor` on an IN-phase chain — `PhaseInterceptorChain(PhaseManagerImpl.getInPhases())` with `SAAJInInterceptor` before the WSS4J in-interceptor.
2. Fixture reuse: `TestKeystoreHelper.createTestKeystore()` (default same-password form) for keystores. Truststore: NO dedicated fixture exists — the verifier profile uses `.truststore(signerKeystorePath, KEYSTORE_PASSWORD)` (the signer's own keystore doubles as truststore; proven pattern at `SecurityProfileWireTest:139`).
3. README: inbound security paragraph — actions.in gates verification/decryption; state that inbound Signature verification (truststore) and Encrypt decryption (keystore) are functional; reference the env-var rows.

**Tests:**
- `signedResponseVerifiesInbound`
  - setup: signer profile (keystore, out-actions "Signature", `.sigUser(TestKeystoreHelper.KEY_ALIAS, TestKeystoreHelper.KEYSTORE_PASSWORD)` — default sigUsername "clientkey" would fail key lookup) and verifier profile (`.truststore(signerKeystorePath, KEYSTORE_PASSWORD)`, in-actions "Signature")
  - action: sign a SOAP document through the outbound chain; process the result through the verifier's in-interceptor chain
  - assert: processing completes without exception; Body content survives
- `encryptedResponseDecryptsInbound`
  - setup: encryptor profile (keystore, out-actions "Encrypt", `.encUser(TestKeystoreHelper.KEY_ALIAS)`) and decryptor profile (keystore, in-actions "Encrypt") — decryptor keystore MUST be the default `createTestKeystore()` (storePass == keyPass): the inbound `PW_CALLBACK_REF` supplies the store password only, so a split-password decryptor key would spuriously fail (callback semantics are a fenced non-goal)
  - action: encrypt a SOAP document through the outbound chain; process through the decryptor's in-interceptor chain
  - assert: processing completes without exception; original content round-trips

`command`: harness `test --tests "org.rustcamel.cxf.SecurityProfileInWireTest"`
`expected`: both fail before Task 1's fix with `ClassCastException` rooted in `WSHandler.getString` (surface as processing error, not clean assert failure — the CCE signature IS the red proof); green after.

**Acceptance:**
- Harness `test` (FULL suite) exits 0 — 131 + Task 1's 2 + these 2 = 135.
- Harness `spotlessCheck` exits 0.
- README paragraph present.

- [x] 2.1
