# Tasks: cxf-pinned-expired-cert

Consumer-only, per-profile opt-in that accepts an expired certificate only when its full
DER is pinned in the profile's keystore or truststore. The design is fixed by the blessed
proposal/design/specs; these tasks implement it without widening any other validity gate.

## Verification guardrails

**Rust containment header — every Rust test command below uses this wrapper.** Run it from
the feature worktree `/home/shared/rust-camel-worktrees/374-cxf-pinned-expired-cert`.

Before the battery:

```bash
test "$(pgrep -c compiled_artifa || true)" -eq 0
```

Run the battery in a fresh user scope (this example uses the first task's full crate test;
replace only the final `cargo test` command with the command named in the task):

```bash
UNIT="rc374-$(date +%s)"
TMPDIR=/home/shared/tmp systemd-run --user --scope --collect --unit="$UNIT" \
  -p MemoryMax=12G -p MemorySwapMax=2G -p TasksMax=1024 -p OOMPolicy=kill \
  -- env TMPDIR=/home/shared/tmp CARGO_BUILD_JOBS=6 cargo test -p camel-component-cxf
systemctl --user stop "$UNIT" 2>/dev/null || true
```

After the battery:

```bash
test "$(pgrep -c compiled_artifa || true)" -eq 0
```

Never run a Rust test outside the scope, never run it in the main checkout, and never point
`TMPDIR` at `/tmp`. Rust `cargo check`, `fmt`, and `clippy` also run only in this worktree
with `TMPDIR=/home/shared/tmp`.

**Java verification.** Set the tool `workdir` to `bridges/cxf` for every Gradle command;
do not use a shell `cd` in any command snippet. Every Java task ends with the mandatory full
suite and formatting gate:

```bash
TMPDIR=/home/shared/tmp ./gradlew --no-daemon spotlessCheck test
```

**Acceptance-test red protocol for Tasks 5 and 6.** These test-only acceptance tests are
added after Tasks 2 and 3, so a baseline compile failure caused by the absent Builder method
is not a meaningful red. Red-before-fix is owned by Task 2 (new class absent) and Task 3
(builder/env absent) in TDD order. If a red run is still required for a Task 5 or 6 test,
snapshot the file first, apply an unblessed code-only mutation, run the named test, observe
the expected acceptance failure, then reverse the exact mutation or restore the PRE-MUTATION
snapshot. Use these commands from the worktree root:

```bash
SECURITY_PROFILE=bridges/cxf/src/main/java/org/rustcamel/cxf/SecurityProfile.java
PRE_SHA=$(sha256sum "$SECURITY_PROFILE" | awk '{print $1}')
cp "$SECURITY_PROFILE" /home/shared/tmp/rc374-SecurityProfile.java.pre-mutation
```

The mutation is: in `SecurityProfile.getVerificationCrypto()`, use
`CryptoFactory.getInstance(createCryptoProperties(path, pass))` even when the flag is true.
After the red run, reverse that exact edit, or restore the snapshot with
`cp /home/shared/tmp/rc374-SecurityProfile.java.pre-mutation "$SECURITY_PROFILE"`, then run:

```bash
POST_SHA=$(sha256sum "$SECURITY_PROFILE" | awk '{print $1}')
test "$PRE_SHA" = "$POST_SHA"
```

Never use `git checkout`, `git restore`, or `git reset` to undo the mutation; those can
restore baseline content instead of the pre-mutation working tree. Confirm the sha256 values
match and never commit the mutation. Task 1's baseline characterization test is not a red
test: it must pass before Task 2 exists and must stay green after Task 3 default-off.

## Java bridge fixtures

### Task 1: Generate expired, future, same-DN, SHA-1, and chain test material

**Files:**
- `bridges/cxf/src/test/java/org/rustcamel/cxf/TestKeystoreHelper.java` (modified)
- `bridges/cxf/src/test/java/org/rustcamel/cxf/ExpiredSignedEnvelopeFactory.java` (new)
- `bridges/cxf/src/test/java/org/rustcamel/cxf/ExpiredCertificateFixturesTest.java` (new)
- `bridges/cxf/src/test/java/org/rustcamel/cxf/ExpiredPinnedCertBaselineCharacterizationTest.java` (new)

**Steps:**
1. Extend `TestKeystoreHelper` with these package-private static methods, all using the JDK
   `keytool` resolved from `System.getProperty("java.home")` and the existing
   `KEYSTORE_PASSWORD`/`KEY_ALIAS` conventions:
   - `Path createExpiredTestKeystore()`: RSA 2048 self-signed, `DEFAULT_DNAME`,
     `-startdate -400d -validity 30`.
   - `Path createFutureTestKeystore()`: RSA 2048 self-signed, `DEFAULT_DNAME`,
     `-startdate +10d -validity 365`.
   - `Path createExpiredSameDnDifferentKeyKeystore()`: fresh RSA 2048 keypair with the exact
     `DEFAULT_DNAME`, `-startdate -400d -validity 30`.
   - `Path createExpiredSha1TestKeystore()`: RSA 2048 self-signed with `-sigalg SHA1withRSA`,
     `-startdate -400d -validity 30`.
   - `Path createExpiredChainKeystore(Path dir)`: CA aliased `ca` (`-ext bc:c`,
     `-validity 3650`) and leaf aliased `leaf` with `-startdate -400d -validity 30`, signed by
     the CA through `-certreq` plus `-gencert -startdate -400d -validity 30`; import the CA
     certificate first and the signed leaf reply with `-trustcacerts` so the key entry chain
     is leaf then CA.
   - `Path createTruststoreWithCertificate(Path sourceKeystore, String sourceAlias)`:
     create a fresh JKS truststore containing the exported certificate under alias `pinned`.
   - `X509Certificate certificateFrom(Path keystore, String alias)`.
   - `X509Certificate[] certificateChainFrom(Path keystore, String alias)`.
2. Add `ExpiredSignedEnvelopeFactory` with one package-private static method:
   `String signSoap11(String soapXml, Path keystore, String alias, String signatureAlgorithm,
   String digestAlgorithm, boolean includeTimestamp, boolean includeCertificateChain)`.
   Build a SOAP 1.1 envelope fixture with `WSSecHeader`; when `includeTimestamp` is true add
    `WSSecTimestamp` and a Timestamp reference; always add the Body Content reference. Use
    `WSConstants.BST_DIRECT_REFERENCE` in both modes and set
    `setUseSingleCertificate(!includeCertificateChain)` so chain mode emits a full PKIPath
    BinarySecurityToken rather than an X509 key identifier containing only the leaf. After
    `prepare`, prepend the BST in both modes. Set the supplied signature and digest algorithms
    and exclusive c14n; serialize with
   `SoapEnvelopeHelper.sourceToString`.
3. Add `ExpiredCertificateFixturesTest` using `@TempDir` for chain output and JUnit
   `@AfterAll` deletion for the temporary stores.
4. Add `ExpiredPinnedCertBaselineCharacterizationTest` using only pre-existing production
   APIs: `SecurityProfile.builder(profileName)` with the expired keystore as both `keystore` and
   `truststore`, `actionsIn("Signature")`, and no new flag; sign with
   `ExpiredSignedEnvelopeFactory.signSoap11`; verify with
   `new WssSecurityProcessor(profile).processInbound(signed)`. It must reproduce the shipped
   expiry rejection before `PinnedTrustMerlin` exists and must not reference
   `allowExpiredPinnedCertificate` or `PinnedTrustMerlin`.
5. Do not add dependencies to `build.gradle.kts`; all generation is JDK `keytool` and existing
   WSS4J test APIs.

**Tests:** (executable spec — name, arrange, act, assert)
- `expiredFixture_isExpiredAndRsa2048`: `createExpiredTestKeystore` → `certificateFrom(path,
  KEY_ALIAS)` → `checkValidity()` throws `CertificateExpiredException`, key algorithm is
  `RSA`, modulus bit length is 2048. Command: `TMPDIR=/home/shared/tmp ./gradlew --no-daemon
  test --tests 'org.rustcamel.cxf.ExpiredCertificateFixturesTest'`. Expected: FAIL before
  implementation (compile error: fixture methods absent); PASS after.
- `futureFixture_isNotYetValid`: `createFutureTestKeystore` → `checkValidity()` throws
  `CertificateNotYetValidException`. Same command; expected FAIL before, PASS after.
- `sameDnDifferentKeyFixture_hasSameSubjectAndDifferentDer`: generate the default and same-DN
  stores → compare subject principals and DER encodings → subjects equal, encodings differ.
  Same command; expected FAIL before, PASS after.
- `sha1Fixture_isSha1SignedAndExpired`: `createExpiredSha1TestKeystore` → certificate
  signature algorithm contains `SHA1` (case-insensitive) and `checkValidity()` throws
  `CertificateExpiredException`. Same command; expected FAIL before, PASS after.
- `chainFixture_hasTwoCertificatesAndExpiredLeaf`: `createExpiredChainKeystore` →
  `certificateChainFrom(store, "leaf")` length 2, leaf `checkValidity()` throws
  `CertificateExpiredException`, leaf issuer equals the CA subject. Same command; expected
  FAIL before, PASS after.
- `truststoreFixture_containsPinnedCertificate`: `createTruststoreWithCertificate(expired,
  KEY_ALIAS)` → load the target JKS → certificate DER equals the source certificate DER.
  Same command; expected FAIL before, PASS after.
- `signedEnvelopeFactory_embedsExpiredCertificate`: sign a plain SOAP 1.1 envelope with the
  expired store → parse the `BinarySecurityToken` → certificate DER equals the expired
  certificate DER. Same command; expected FAIL before, PASS after.
- `baselineExpiredPinnedCertIsRejectedByExistingProfile`: expired keystore used as both
  `keystore` and `truststore`, no opt-in flag → `processInbound` throws
  `WSSecurityException`; `getErrorCode()` is `FAILED_CHECK`; `getCause()` is
  `CertificateExpiredException`; `getMessage()` contains `The provided certificate is
  invalid`. Command: `TMPDIR=/home/shared/tmp ./gradlew --no-daemon test --tests
  'org.rustcamel.cxf.ExpiredPinnedCertBaselineCharacterizationTest'`. Expected: PASS
  immediately after Task 1 and remain PASS after Task 3 default-off wiring; this is the
  baseline characterization, not a red test.

**Acceptance:**
- `TMPDIR=/home/shared/tmp ./gradlew --no-daemon spotlessCheck test` exits 0.
- `git diff -- bridges/cxf/build.gradle.kts` is empty; no new dependency was added.
- `git status --short` shows no change to `NativeImageReflectionRegistrations.java`,
  `CxfClientManager.java`, or `build.gradle.kts`.
- `ExpiredPinnedCertBaselineCharacterizationTest` passes with no reference to the new flag or
  `PinnedTrustMerlin`.

- [x] 1

## Java bridge trust seam

### Task 2: Add `PinnedTrustMerlin` and the direct-Crypto rejection matrix

**Files:**
- `bridges/cxf/src/main/java/org/rustcamel/cxf/PinnedTrustMerlin.java` (new)
- `bridges/cxf/src/test/java/org/rustcamel/cxf/PinnedTrustMerlinTest.java` (new)

**Steps:**
1. Add package-private `final class PinnedTrustMerlin extends Merlin` with constructor
   `PinnedTrustMerlin(Properties cryptoProperties, boolean allowExpired)` that calls
   `super(cryptoProperties, PinnedTrustMerlin.class.getClassLoader(), null)` and maps a thrown
   `IOException` to `new WSSecurityException(WSSecurityException.ErrorCode.FAILURE, e)`.
2. Override the protected three-argument
   `void verifyTrust(X509Certificate[] certs, boolean enableRevocation,
   Collection<Pattern> subjectCertConstraints)`: call `super.verifyTrust(certs, enableRevocation, subjectCertConstraints)`; catch only
   `WSSecurityException`; rethrow immediately unless all of the following hold:
   `allowExpired` is true, `enableRevocation` is false, `certs` is non-null with length 1,
   `caught.getCause() instanceof CertificateExpiredException` is true using ONLY the direct
   cause (never walk the cause chain), and `certificatesEqual(certs[0], entryCert)` is true
   for an entry in `getKeyStore()` or `getTrustStore()`. Compare DER with
   `Arrays.equals(certificate.getEncoded(), entry.getEncoded())`; any `KeyStoreException` or
   `CertificateEncodingException` while scanning rethrows the original
   `WSSecurityException` object unchanged.
3. Scan every alias in both stores through `getCertificate(alias)`; tolerate a null
   `getTrustStore()` and null entries. Do not add a global flag, a system property, or state
   outside the instance.
4. In `PinnedTrustMerlinTest`, construct with empty `Properties` and set the stores directly
   with the inherited public `setKeyStore(KeyStore)` and `setTrustStore(KeyStore)` methods so
   the two-store scan is exercised without changing production crypto loading. Load JKS files
   with `KeyStore.getInstance("JKS")` and `TestKeystoreHelper.KEYSTORE_PASSWORD`. Use
   `Collections.emptyList()` for constraints.
5. Do not edit `NativeImageReflectionRegistrations.java`: the class is constructed with `new`,
   never by name.

**Tests:** (executable spec — name, arrange, act, assert)
- `expiredPinnedCertAcceptedWhenPinnedInKeystore`: expired store pinned as the keystore,
  `allowExpired=true` → `verifyTrust(new X509Certificate[]{expired}, false, empty)` returns
  without throwing. Command: `TMPDIR=/home/shared/tmp ./gradlew --no-daemon test --tests
  'org.rustcamel.cxf.PinnedTrustMerlinTest'`. Expected: FAIL before implementation (compile
  error: `PinnedTrustMerlin` absent); PASS after.
- `expiredPinnedCertAcceptedWhenPinnedInTruststore`: `setKeyStore(valid)` and
  `setTrustStore(expiredTruststore)` directly, `allowExpired=true` → `verifyTrust` with the
  expired certificate returns without throwing, proving the truststore scan. Same command;
  expected FAIL before, PASS after.
- `notYetValidCertRejectedByMerlin`: future store set as the keystore, `allowExpired=true` →
  `verifyTrust` throws `WSSecurityException`; `getErrorCode()` is `FAILED_CHECK`;
  `getCause()` is `CertificateNotYetValidException`; `getMessage()` contains
  `The provided certificate is invalid`. Same command; expected FAIL before, PASS after.
- `chainWithExpiredLeafRejectedByMerlin`: truststore contains the CA, `allowExpired=true` →
  `verifyTrust(new X509Certificate[]{expiredLeaf, ca}, false, empty)` throws
  `WSSecurityException`. Same command; expected FAIL before, PASS after.
- `pinnedIssuerDoesNotAuthorizeExpiredLeafByMerlin`: truststore contains only the CA,
  `allowExpired=true` → `verifyTrust(new X509Certificate[]{expiredLeaf}, false, empty)`
  throws `WSSecurityException`. Same command; expected FAIL before, PASS after.
- `revocation_enabled_disables_the_opt_in` (T7): expired certificate pinned, `allowExpired=true`
  → `verifyTrust(new X509Certificate[]{expired}, true, empty)` throws `WSSecurityException`,
  while the same call with `false` returns without throwing. Same command; expected FAIL
  before, PASS after.
- `unrelated_failure_is_rethrown_unchanged`: `allowExpired=true`, present the valid unpinned
  default certificate with an unrelated valid CA truststore → `verifyTrust` throws
  `WSSecurityException`; configure a plain `Merlin` with the same stores and certificates and
  assert the pinned exception has the same `getErrorCode()` and `getMessage()` as the stock
  one, proving a non-expiry failure was rethrown rather than swallowed. Same command;
  expected FAIL before, PASS after.

**Master-requested guard coverage correction:** The PKIX rejection tests above do not isolate
the acceptance predicate's guards. Keep them, and exercise the existing private predicate from
tests through reflection, without adding a production seam. Supply a controlled
`WSSecurityException` with a direct `CertificateExpiredException` cause. Each negative test must
also assert its otherwise-identical positive control.

- `acceptancePredicate_acceptsSinglePinnedExpiredCertificate`: opt-in on, one exact pinned
  expired certificate and revocation off; assert `true`.
- `acceptancePredicate_rejectsDifferentDer`: keep the same DN but present a different DER;
  assert `false` while the exact pinned certificate returns `true`.
- `acceptancePredicate_rejectsMultipleCertificates`: present `[pinned, CA]`; assert `false`
  while `[pinned]` returns `true`.
- `acceptancePredicate_rejectsRevocationEnabled`: enable revocation; assert `false` while the
  same call with revocation off returns `true`.
- `acceptancePredicate_rejectsOptInDisabled`: turn the opt-in off; assert `false` while the
  otherwise-identical enabled instance returns `true`.
- `acceptancePredicate_rejectsNonExpiryCause`: replace the direct expiry cause with
  `CertificateNotYetValidException`; assert `false` while direct expiry returns `true`.
- `acceptancePredicate_rejectsNestedExpiryCause`: wrap expiry below another direct cause;
  assert `false` while direct expiry returns `true`.

Command: `./gradlew --no-daemon test --tests 'org.rustcamel.cxf.PinnedTrustMerlinTest'`, with
the Java environment above. Expected: restored production passes all 14 tests. Removing each
of the pin-match, certificate-count and revocation guards individually must fail its matching
new test. Record the old seven-test suite surviving each mutant, each new test killing its
mutant, and byte-identical production restoration after every mutation.

**Acceptance:**
- `TMPDIR=/home/shared/tmp ./gradlew --no-daemon spotlessCheck test` exits 0.
- `rg -n "PinnedTrustMerlin" bridges/cxf/src/main/java/org/rustcamel/cxf/NativeImageReflectionRegistrations.java` exits 1 (no reflection registration).
- `git status --short` shows no change to `CxfClientManager.java` or `build.gradle.kts`.
- `PinnedTrustMerlinTest` is green on both the keystore-pinned and truststore-pinned cases.

- [x] 2

### Task 3: Wire the opt-in through profile config, startup validation, and WARN

**Files:**
- `bridges/cxf/src/main/java/org/rustcamel/cxf/SecurityProfile.java` (modified)
- `bridges/cxf/src/main/java/org/rustcamel/cxf/SecurityProfileStore.java` (modified)
- `bridges/cxf/src/test/java/org/rustcamel/cxf/SecurityProfileTest.java` (modified)
- `bridges/cxf/src/test/java/org/rustcamel/cxf/SecurityProfileStoreTest.java` (modified)

**Steps:**
1. In `SecurityProfile`, add `private final boolean allowExpiredPinnedCertificate`, copy it in
   the private constructor from the Builder, and expose
   `public boolean allowExpiredPinnedCertificate()`.
2. Add `Builder.allowExpiredPinnedCertificate(boolean)` and
   `private boolean allowExpiredPinnedCertificate`; call a new
   `validateExpiredPinnedCertificate()` from `build()` after `validateInboundActions()`.
   When the flag is false or unset the method returns. When true:
   - compute `envVar = "CXF_PROFILE_" + name.toUpperCase(Locale.ROOT) +
     "_ALLOW_EXPIRED_PINNED_CERTIFICATE"`;
   - resolve inbound actions with `resolveActionsOrDefault(securityActionsIn)` and throw
     `IllegalArgumentException` naming `envVar` when the resolved list lacks `Signature`;
   - throw `IllegalArgumentException` naming `envVar` when neither `truststorePath` nor
     `keystorePath` has text. Blank actions therefore pass through
     `resolveActionsOrDefault` as `Signature`; an explicit `Encrypt` in-action without
     `Signature` fails.
3. In `getVerificationCrypto()`, preserve the existing truststore-preferred selection
   verbatim: `String path = hasText(truststorePath) ? truststorePath : keystorePath;` and
   `String pass = hasText(truststorePassword) ? truststorePassword : keystorePassword;`.
   Keep the existing lazy double-checked block. When `allowExpiredPinnedCertificate` is
   false, keep `CryptoFactory.getInstance(createCryptoProperties(path, pass))` unchanged.
   When true, change only the implementation to
   `verificationCrypto = new PinnedTrustMerlin(createCryptoProperties(path, pass), true)`.
   Do not add `Merlin.TRUSTSTORE_*` properties and do not load the configured signing
   keystore as an additional verification store; the verification material remains exactly
   the file the existing code selected.
4. In `SecurityProfileStore`, read
   `CXF_PROFILE_<NAME>_ALLOW_EXPIRED_PINNED_CERTIFICATE` with a helper that returns false for
   null/blank, true for `true`, false for `false`, and throws `IllegalStateException` naming
   the variable for any other value; pass the result to the Builder. Add
   `static Optional<String> expiredPinnedCertWarning(SecurityProfile profile)` that returns
   empty unless the profile opted in, else a WARN string naming the profile,
   `ALLOW_EXPIRED_PINNED_CERTIFICATE`, and the accepted risk in ADR-0087. Call it beside
   `sha1AllowedWarning(profile)` in the profile-load loop.
5. Do not add a revocation profile setting, a producer-path setting, or a global switch.

**Tests:** (executable spec — name, arrange, act, assert)
- `startup_rejects_flag_without_signature_action` (T8): builder with `keystore(expired,
  password)`, `actionsIn("Encrypt")`, `allowExpiredPinnedCertificate(true)` → `build()`
  throws `IllegalArgumentException` whose message contains
  `CXF_PROFILE_EXPIRED_ALLOW_EXPIRED_PINNED_CERTIFICATE`. Command:
  `TMPDIR=/home/shared/tmp ./gradlew --no-daemon test --tests
  'org.rustcamel.cxf.SecurityProfileTest'`. Expected: FAIL before implementation (compile
  error: builder method absent); PASS after.
- `startup_rejects_flag_without_verification_material`: builder with blank actions, no
  keystore, no truststore, flag true → `build()` throws `IllegalArgumentException` naming the
  variable and saying a truststore or keystore is required. Same command; expected FAIL
  before, PASS after.
- `startup_warns_about_accepted_risk`: valid opted-in profile with a truststore →
  `SecurityProfileStore.expiredPinnedCertWarning(profile)` is present, contains the profile
  name and `ADR-0087`; the same call on a default-off profile is empty. Same command;
  expected FAIL before, PASS after.
- `default_inbound_signature_action_permits_the_opt_in`: opted-in profiles with blank
  `actionsIn` and (a) a truststore, (b) a keystore only → both build successfully and
  `resolveActionsIn()` equals `Signature`. Same command; expected FAIL before, PASS after.
- `store_parses_allow_expired_pinned_certificate_env`: store subclass maps
  `CXF_PROFILES=opted`, an existing fake keystore path/password, and
  `CXF_PROFILE_OPTED_ALLOW_EXPIRED_PINNED_CERTIFICATE=true` → stored profile
  `allowExpiredPinnedCertificate()` is true. Command: `TMPDIR=/home/shared/tmp
  ./gradlew --no-daemon test --tests 'org.rustcamel.cxf.SecurityProfileStoreTest'`.
  Expected: FAIL before, PASS after.
- `store_defaults_allow_expired_pinned_certificate_false`: same env without the variable →
  accessor false. Same command; expected FAIL before, PASS after.
- `store_rejects_non_boolean_allow_expired_pinned_certificate`: env value `yes` →
  `SecurityProfileStore` construction throws `IllegalStateException` naming the variable.
  Same command; expected FAIL before, PASS after.
- `default_off_keeps_stock_merlin_behavior`: valid profile without the flag →
  `getVerificationCrypto()` is not an instance of `PinnedTrustMerlin`. Same command;
  expected FAIL before, PASS after.
- `opt_in_builds_pinned_trust_merlin`: valid profile with the flag and a keystore →
  `getVerificationCrypto()` is an instance of `PinnedTrustMerlin`. Same command; expected
  FAIL before, PASS after.
- `opt_in_truststore_does_not_trust_signing_only_certificate`: profile with a valid signing
  keystore and a separate expired-certificate truststore → a flag-off stock profile over the
  same keystore+truststore is not `PinnedTrustMerlin`, and both the stock and opted-in
  `getVerificationCrypto()` reject `verifyTrust(new X509Certificate[]{validSigningCert}, false,
  Collections.emptyList(), Collections.emptyList())` with a `WSSecurityException` whose
  `ErrorCode` and message are equal. The valid signing-only certificate must remain untrusted
  because the opt-in adds no signing-keystore verification material. Same command; expected
  FAIL before, PASS after.

**Acceptance:**
- `TMPDIR=/home/shared/tmp ./gradlew --no-daemon spotlessCheck test` exits 0.
- `rg -n "allowExpiredPinnedCertificate" bridges/cxf/src/main/java/org/rustcamel/cxf/SecurityProfile.java bridges/cxf/src/main/java/org/rustcamel/cxf/SecurityProfileStore.java` shows both the Builder/accessor and the store parsing/warning.
- `rg -n "TRUSTSTORE_FILE|TRUSTSTORE_PASSWORD" bridges/cxf/src/main/java/org/rustcamel/cxf/SecurityProfile.java` exits 1: the opt-in adds no truststore properties and does not load the signing keystore additionally.
- `opt_in_truststore_does_not_trust_signing_only_certificate` is green.
- No new revocation, producer, or global configuration symbol exists.

- [x] 3

## Rust plumbing

### Task 4: Carry `allow_expired_pinned_certificate` from TOML to the bridge environment

**Files:**
- `crates/components/camel-cxf/src/config.rs` (modified)
- `crates/components/camel-cxf/src/component.rs` (modified)
- `crates/components/camel-cxf/src/pool.rs` (modified)
- `crates/components/camel-cxf/src/pool_env_test.rs` (modified)
- `crates/components/camel-cxf/tests/consumer_signer_test.rs` (modified)
- `crates/services/camel-bridge/src/process.rs` (modified)
- `crates/camel-test/tests/cxf_test.rs` (modified)
- `crates/camel-test/tests/cxf_test/signer.rs` (modified)
- `crates/camel-test/tests/cxf_test/tls.rs` (modified)
- `crates/camel-test/tests/cxf_test/wss_e2e.rs` (modified)

**Steps:**
1. In `CxfSecurityFields`, add
   `#[serde(default)] pub allow_expired_pinned_certificate: bool` after
   `allowed_digest_algorithms`, and add `.field("allow_expired_pinned_certificate",
   &self.allow_expired_pinned_certificate)` to the manual `Debug` impl (a boolean is not
   redacted). Do not add Rust-side startup validation: the Java bridge remains the
   authoritative fail-closed validator per the blessed design.
2. In `pool.rs::profile_env_vars_for`, map
   `allow_expired_pinned_certificate: p.security.allow_expired_pinned_certificate`.
3. In `camel-bridge`'s `CxfProfileEnvVars`, add
   `pub allow_expired_pinned_certificate: bool`; in `to_env_vars`, push
   `(format!("{}ALLOW_EXPIRED_PINNED_CERTIFICATE", prefix), "true".to_string())` only when
   the boolean is true. No env var is emitted for false, preserving the default.
4. Add `allow_expired_pinned_certificate: false` (or true where the test asserts emission)
   to every `CxfSecurityFields` and `CxfProfileEnvVars` struct literal in the files listed
   above. Use `rg -n "CxfSecurityFields \\{|CxfProfileEnvVars \\{" crates` before and after
   to confirm no literal was missed.
5. Add the Rust tests listed below; update the existing
   `profile_env_vars_to_env_vars_includes_all_fields` expectation to 16 vars and assert the
   new key value is `true`.
6. Run `cargo fmt --check --all` and
   `cargo clippy -p camel-component-cxf -p camel-bridge --all-targets -- -D warnings` in the
   worktree with `TMPDIR=/home/shared/tmp`.

**Tests:** (executable spec — name, arrange, act, assert)
All commands use the containment header above.
- `allow_expired_pinned_certificate_defaults_false`: parse TOML `[profiles.security]` with a
  truststore but no new key → field is false. Command: containment header plus
  `cargo test -p camel-component-cxf config::tests::allow_expired_pinned_certificate_defaults_false -- --exact`.
  Expected: FAIL before implementation (field absent, compile error); PASS after.
- `allow_expired_pinned_certificate_parses_true`: parse TOML with the field true → field is
  true and `format!("{:?}", security)` contains `allow_expired_pinned_certificate: true`.
  Command: containment header plus `cargo test -p camel-component-cxf
  config::tests::allow_expired_pinned_certificate_parses_true -- --exact`. Expected: FAIL
  before, PASS after.
- `pool_env_emits_allow_expired_pinned_certificate_only_when_true`: build two profiles, one
  true and one false, through `profile_env_vars_for` and
  `bridge_config_with_env_trace` → the true profile has
  `CXF_PROFILE_<NAME>_ALLOW_EXPIRED_PINNED_CERTIFICATE=true`, the false profile has no such
  key. Command: containment header plus `cargo test -p camel-component-cxf
  pool_env_test::pool_env_emits_allow_expired_pinned_certificate_only_when_true -- --exact`.
  Expected: FAIL before, PASS after.
- `profile_env_vars_to_env_vars_includes_all_fields`: existing test updated to set the flag
  true → env key list contains the new key and its value is `true`; total count is 16.
  Command: containment header plus `cargo test -p camel-bridge
  process::tests::profile_env_vars_to_env_vars_includes_all_fields -- --exact`. Expected:
  FAIL before (compile error: struct field absent); PASS after with output containing
  `test result: ok. 1 passed; 0 failed`.
- `profile_env_vars_omits_allow_expired_pinned_certificate_when_false`: flag false →
  `to_env_vars()` contains no key ending in `ALLOW_EXPIRED_PINNED_CERTIFICATE`. Command:
  containment header plus `cargo test -p camel-bridge
  process::tests::profile_env_vars_omits_allow_expired_pinned_certificate_when_false -- --exact`.
  Expected: FAIL before, PASS after with output containing
  `test result: ok. 1 passed; 0 failed`.
- Full batteries: containment header plus `cargo test -p camel-component-cxf`, then
  `cargo test -p camel-bridge`, then
  `cargo test -p camel-cli --test feature_profiles`. Expected: all three exit 0 and each
  prints `test result: ok.` with a non-zero passed count; record the exact passed/failed
  counts. The CLI golden is not regenerated because no crate/feature edge changes; if the
  golden genuinely drifts, report the diff before changing it.

**Acceptance:**
- `cargo fmt --check --all` exits 0.
- `cargo clippy -p camel-component-cxf -p camel-bridge --all-targets -- -D warnings` exits 0.
- `cargo check -p camel-test --tests` exits 0 (all workspace struct literals updated).
- The three full batteries above exit 0 under containment, each reports a non-zero passed
  count, and `pgrep -c compiled_artifa` is 0 before and after each. Each targeted `--exact`
  test reports `1 passed; 0 failed` (a zero-test run is a failure, not a pass).
- `rg -n "ALLOW_EXPIRED_PINNED_CERTIFICATE" crates` shows exactly one emission site in
  `camel-bridge` and no accidental emission for false.

- [x] 4

## Java behaviour matrix

### Task 5: Prove consumer acceptance, default rejection, and zero gRPC dispatch

**Files:**
- `bridges/cxf/src/test/java/org/rustcamel/cxf/ExpiredPinnedCertAcceptanceTest.java` (new)
- `bridges/cxf/src/test/java/org/rustcamel/cxf/SoapEndpointPublisherTest.java` (modified)

**Steps:**
1. Add `ExpiredPinnedCertAcceptanceTest` using the fixtures from Task 1 and the profile
   wiring from Task 3. For engine-level tests build a `SecurityProfile` and call
   `new WssSecurityProcessor(profile).processInbound(signedXml)`; use
   `ExpiredSignedEnvelopeFactory.signSoap11` with RSA-SHA256 and SHA-256 so the algorithm
   gate is never the rejection cause. Assert signer fingerprints by comparing the SHA-256 of
   the DER certificate with `verified.signer().get().getFingerprintSha256()`.
2. Derive engine profiles in test helpers:
   - expired profile with the expired certificate pinned in the verification truststore
     (`keystore(expiredKs)`, `truststore(expiredTruststore)`,
     `allowExpiredPinnedCertificate(true)`, blank `actionsIn`); the verification Crypto still
     uses the truststore-preferred file only;
   - not-pinned profile pinning the expired certificate while signing with the same-DN
     different-key store.
3. Add two listener-boundary tests to `SoapEndpointPublisherTest` using the existing
   `driveRequestOutcome(requestXml, contentType, profile, completedOk())` helper and the
   existing `envelope11(ECHO_ELEMENT)` body. Configure the opt-in profile through
   `profileStore.getProfile(TEST_PROFILE_NAME)` exactly as the existing
   `emittedConsumerRequestCarriesSigner` test does. For T9 capture
   `ArgumentCaptor<ConsumerRequest>` on `cxfServerManager.handleSoapRequest` and assert the
   `ConsumerRequest` signer fingerprint. For T2 assert the fault response and
   `verify(cxfServerManager, never()).handleSoapRequest(any())`.
4. Keep the engine-level default-off assertion as a separate test so FAILED_CHECK, direct
   cause, and rendered message are observable without the listener wrapping the exception.
5. Do not add a producer opt-in, a revocation setting, or a route-dispatch bypass.

**Tests:** (executable spec — name, arrange, act, assert)
- `expired_pinned_cert_accepted_when_opted_in` (T1): expired self-signed certificate pinned in
  the verification truststore while a separate expired signing keystore is configured,
  profile opt-in on, blank `actionsIn` → engine
  `processInbound` returns without throwing, signer is present, and its fingerprint equals the
  expired certificate's SHA-256 DER fingerprint. Command: `TMPDIR=/home/shared/tmp
  ./gradlew --no-daemon test --tests
  'org.rustcamel.cxf.ExpiredPinnedCertAcceptanceTest'`. Expected: PASS at task completion;
  red protocol in the verification guardrails applies.
- `default_off_engine_rejection_is_rendered`: same expired certificate and stores, flag
  unset → engine `processInbound` throws `WSSecurityException`; `getErrorCode()` is
  `FAILED_CHECK`; `getCause()` is `CertificateExpiredException`; `getMessage()` contains
  `The provided certificate is invalid`. Same command; expected PASS at task completion
  (red protocol).
- `expired_cert_rejected_when_not_pinned` (T3): profile opt-in on and pinned expired
  certificate A; envelope signed with the same-DN different-key expired certificate B →
  engine `processInbound` throws `WSSecurityException`. Same command; expected PASS at task
  completion (red protocol).
- `expired_pinned_cert_rejected_by_default` (T2): listener-boundary test in
  `SoapEndpointPublisherTest`; default-off profile with the expired certificate pinned;
  drive the signed envelope with `driveRequestOutcome(signed, "text/xml", profile,
  completedOk())` → SOAP 1.1 status 500 with a Sender fault whose body contains
  `The provided certificate is invalid`, and
  `verify(cxfServerManager, never()).handleSoapRequest(any())` proves zero gRPC dispatch.
  Command: `TMPDIR=/home/shared/tmp ./gradlew --no-daemon test --tests
  'org.rustcamel.cxf.SoapEndpointPublisherTest'`. Expected: PASS at task completion (red
  protocol).
- `expired_pinned_cert_e2e_reaches_route_with_signer` (T9): listener-boundary test in
  `SoapEndpointPublisherTest`; opt-in profile with the expired certificate pinned, drive the
  signed envelope through `driveRequestOutcome` → capture the dispatched
  `ConsumerRequest`, assert `hasSigner()` is true, and assert
  `getSigner().getFingerprintSha256()` equals the expired certificate's SHA-256 DER
  fingerprint; `verify(cxfServerManager).handleSoapRequest(requestCaptor.capture())` shows
  exactly one dispatch. Same command; expected PASS at task completion (red protocol).

**Acceptance:**
- `TMPDIR=/home/shared/tmp ./gradlew --no-daemon spotlessCheck test` exits 0 (full Java
  suite, including all pre-existing tests).
- T1, T2, T3, T9, and `default_off_engine_rejection_is_rendered` are green.
- T2 asserts zero `handleSoapRequest` calls; T9 asserts one call and a non-empty signer
  fingerprint on the captured `ConsumerRequest`.
- `rg -n "allowExpiredPinnedCertificate" bridges/cxf/src/main/java/org/rustcamel/cxf` appears
  only in `SecurityProfile`/`SecurityProfileStore` and the trust seam, never in the producer
  interceptor construction.
- No change to `NativeImageReflectionRegistrations.java`, `CxfClientManager.java`, or
  `build.gradle.kts`.

- [x] 5

### Task 6: Prove every non-expiry gate, SHA-1 composition, and producer non-coverage

**Files:**
- `bridges/cxf/src/test/java/org/rustcamel/cxf/ExpiredPinnedCertGateTest.java` (new)
- `bridges/cxf/src/test/java/org/rustcamel/cxf/SecurityProfileInInterceptorTest.java` (modified)

**Steps:**
1. Add `ExpiredPinnedCertGateTest` using the fixtures from Task 1 and the profile wiring from
   Task 3. Build profiles for the future, expired-chain, issuer-only, and partner SHA-1 cases;
   sign with RSA-SHA256 and SHA-256 except in the SHA-1 composition case, which signs with
   RSA-SHA1 and SHA-1. Call `new WssSecurityProcessor(profile).processInbound(signedXml)`.
2. Future profile pins the future store. Chain profile pins the CA in the truststore and
   signs with the two-certificate chain. Issuer-only profile pins the CA and signs with the
   expired leaf only. SHA-1 profile uses the expired SHA-1 store with both allow-lists set to
   the partner URIs.
3. Add one producer test to `SecurityProfileInInterceptorTest`: profile with truststore
   containing the expired certificate, `actionsIn("Signature")`, and
   `allowExpiredPinnedCertificate(true)`; call `createInInterceptor()` and
   `invokeInInterceptor(profile, signedExpiredResponse)` → `org.apache.cxf.interceptor.Fault` wrapping
   `WSSecurityException` with `ErrorCode.FAILED_CHECK`, direct cause
   `CertificateExpiredException`, and `getMessage()` containing
   `The provided certificate is invalid`. The producer path uses the stock `CryptoFactory`
   properties built by `createInInterceptor`, so the opt-in is not honored there.
4. Do not add a producer opt-in, a revocation setting, or a route-dispatch bypass.

**Tests:** (executable spec — name, arrange, act, assert)
- `not_yet_valid_cert_rejected` (T4): profile opt-in on and future certificate pinned →
  envelope signed with the future certificate → `processInbound` throws
  `WSSecurityException`; `getErrorCode()` is `FAILED_CHECK`; `getCause()` is
  `CertificateNotYetValidException`; `getMessage()` contains
  `The provided certificate is invalid`. Command: `TMPDIR=/home/shared/tmp ./gradlew --no-daemon
  test --tests 'org.rustcamel.cxf.ExpiredPinnedCertGateTest'`. Expected: PASS at task
  completion; red protocol in the verification guardrails applies.
- `chain_with_expired_leaf_rejected` (T5): profile opt-in on, CA truststore, envelope carries
  a two-certificate chain whose leaf is expired. Assert the direct STR references an
  `X509PKIPathv1` BinarySecurityToken, decode its two certificates and compare their DER to
  the expected leaf and CA. Observe the real Crypto's public four-argument `verifyTrust`
  call, assert that the same two certificates arrive before invoking the real method, then
  require `processInbound` to throw `WSSecurityException` and exactly one observed trust call.
  No exception-cause topology is asserted because path validation is provider-dependent.
  Same command; expected FAIL against the original leaf-only factory, PASS with the corrected
  PKIPath factory.
- `pinned_issuer_does_not_authorize_expired_leaf` (T6): profile opt-in on, only the issuer CA
  pinned, envelope presents the expired leaf only → `processInbound` throws
  `WSSecurityException`; no exception-cause topology is asserted because path validation is
  provider-dependent. Same command; expected PASS at task completion (red protocol).
- `sha1_and_expiry_opt_ins_compose` (T10): expired self-signed RSA-SHA1 certificate pinned,
  envelope signed with RSA-SHA1 plus SHA-1, profile with both the expiry opt-in and both
  allow-lists set → `processInbound` succeeds and the signer is present. Then the same
  certificate with expiry opt-in on but allow-lists at the secure defaults →
  `WSSecurityException` with `ErrorCode.INVALID_SECURITY`; `getCause()` is not
  `CertificateExpiredException`; `getMessage()` does not contain
  `The provided certificate is invalid`. WSS4J logs the rejected algorithm URI only in a
  WARN, so the test does not require the URI in the thrown message. Then the same certificate
  with allow-lists on but expiry opt-in off → `WSSecurityException` with
  `ErrorCode.FAILED_CHECK`, direct cause `CertificateExpiredException`, and `getMessage()`
  containing `The provided certificate is invalid`, distinguishable from the generic
  `INVALID_SECURITY` algorithm rejection. Same command; expected PASS at task completion
  (red protocol).
- `producer_profile_in_interceptor_rejects_expired_pinned_response`: producer profile opt-in
  on with the expired certificate in the truststore and `actionsIn("Signature")`; invoke the
  profile's `createInInterceptor()` on a response signed with the expired certificate →
  `org.apache.cxf.interceptor.Fault` wrapping `WSSecurityException` with `ErrorCode.FAILED_CHECK`, direct cause
  `CertificateExpiredException`, and `getMessage()` containing
  `The provided certificate is invalid`. Command: `TMPDIR=/home/shared/tmp ./gradlew --no-daemon
  test --tests 'org.rustcamel.cxf.SecurityProfileInInterceptorTest'`. Expected: PASS at task
  completion (red protocol).

**Acceptance:**
- `TMPDIR=/home/shared/tmp ./gradlew --no-daemon spotlessCheck test` exits 0 (full Java
  suite, including all pre-existing tests).
- T4, T5, T6, T10, and the producer test are green.
- `rg -n "allowExpiredPinnedCertificate" bridges/cxf/src/main/java/org/rustcamel/cxf` appears
  only in `SecurityProfile`/`SecurityProfileStore` and the trust seam, never in the producer
  interceptor construction.
- No change to `NativeImageReflectionRegistrations.java`, `CxfClientManager.java`, or
  `build.gradle.kts`.

- [ ] 6

## Documentation and schema

### Task 7: Record the ADR, document the consumer-only opt-in, and regenerate schemas

**Files:**
- `docs/adr/0087-cxf-expired-pinned-cert-opt-in.md` (new)
- `docs/src/components/ws-soap.md` (modified)
- `CONTEXT-MAP.md` (modified)

**Steps:**
1. Write ADR-0087 in English with status Accepted, source `rc-x7wys`, and cross-references to
   ADR-0033, ADR-0078, ADR-0085, and ADR-0083. Record: the per-profile opt-in, exact DER
   pinning in the keystore or truststore, single-certificate and revocation-disabled
   preconditions, consumer-only scope, the startup fail-closed checks and WARN, and the
   accepted risk that expiry as a key-lifetime bound (and expiry-tied rotation) is lost for
   this exact pinned key. State that not-yet-valid, chains longer than one, issuer-only pins,
   and revocation-enabled verification still reject. Add a distinctness paragraph: ADR-0083
   pins an artifact-signing key by `blake3:` fingerprint for artifact verification; ADR-0087
   pins an X.509 certificate by full DER equality inside the WS-Security Crypto store. They
   are unrelated trust surfaces. Record the error split: the algorithm URI is logged only in
   a WSS4J WARN, the thrown algorithm rejection is generic `INVALID_SECURITY`, and it is
   distinguishable from the `FAILED_CHECK` rejection whose rendered message is
   `The provided certificate is invalid` (`invalidCert`).
2. Add a subsection under `## CXF security` in `docs/src/components/ws-soap.md` titled
   `### Expired pinned certificate (consumer opt-in)`. Include the option
   `allow_expired_pinned_certificate`, the environment variable
   `CXF_PROFILE_<NAME>_ALLOW_EXPIRED_PINNED_CERTIFICATE`, the property label
   `cxf.security.allow.expired.pinned.certificate`, and default `false`. Document the exact
   acceptance conditions, the still-rejected cases, the consumer-only limit (the producer
   `ProfileInInterceptor` path always uses stock `CryptoFactory`), startup validation for
   resolved inbound actions without `Signature` or missing verification material, the WARN,
   and the distinguishable error messages: the Wire key `invalidCert` renders
   `The provided certificate is invalid` and is thrown with `FAILED_CHECK`; the
   signature-algorithm rejection is the generic `INVALID_SECURITY` error, with the rejected
   algorithm URI appearing only in the WSS4J WARN log and never required in the thrown
   message. State that the SHA-1 allow-list and this opt-in are
   independent and may both be enabled for the partner case. Include a TOML example under
   `[profiles.security]`.
3. Add the ADR-0087 row to the ADR list in `CONTEXT-MAP.md` immediately after the ADR-0084
   row, using a one-line English summary and a link to `./docs/adr/0087-cxf-expired-pinned-cert-opt-in.md`.
4. Regenerate the schema unconditionally with `cargo xtask schema`, even though
   `allow_expired_pinned_certificate` is runtime component configuration rather than a
   canonical route or component-metadata field and a zero diff is expected. Include any
   regenerated artifact the command reports, then verify with `cargo xtask schema --check`.
   Do not substitute `--check` for the mandatory regeneration.
5. Keep all prose English-only; do not touch the producer TLS or native-image sections.

**Tests:** (docs/schema task uses executable checks; there is no fake red assertion)
- `openspec_validate_change`: run `openspec validate cxf-pinned-expired-cert --type change`
  → exits 0. Expected: PASS after the docs edit (this validates the change artifacts, not
  new code).
- `schema_regeneration`: use the containment header with the tail `cargo xtask schema` →
  exits 0 and prints the generated artifact paths. Expected: PASS; record whether the
  working-tree diff is zero.
- `schema_check`: use the containment header with the tail `cargo xtask schema --check` →
  exits 0 and prints `OK: all schemas and TS types match.` Expected: PASS after the mandatory
  regeneration.
- `docs_guide_test`: set the tool `workdir` to `docs` and run `mdbook test` → exits 0.
  Expected: PASS.
- `context_citations`: run `cargo xtask lint-context-citations` → exits 0 after the
  `CONTEXT-MAP.md` row is added. Expected: PASS.
- `adr_and_docs_grep`: run `rg -n "ADR-0087|allow_expired_pinned_certificate|consumer" docs/adr/0087-cxf-expired-pinned-cert-opt-in.md docs/src/components/ws-soap.md CONTEXT-MAP.md`
  → lines exist in all three files. Expected: PASS.

**Acceptance:**
- ADR-0087 exists with an Accepted status and the distinctness paragraph from ADR-0083.
- `docs/src/components/ws-soap.md` states default `false`, the env var, the consumer-only
  scope, the still-rejected cases, and the two distinguishable errors, including the rendered
  `The provided certificate is invalid` text for the `invalidCert` key.
- `CONTEXT-MAP.md` contains the ADR-0087 row.
- `openspec validate cxf-pinned-expired-cert --type change` exits 0.
- `cargo xtask schema` exits 0 under containment (mandatory regeneration, even with zero
  diff), and the subsequent `cargo xtask schema --check` exits 0.
- `mdbook test` exits 0 with tool `workdir` set to `docs`.
- `git status --short` shows no change to `bridges/cxf/build.gradle.kts`, no version bump,
  and no release metadata.

- [x] 7

## Self-review coverage map

| Spec scenario | Owning task and test |
| --- | --- |
| Expired pinned cert accepted when opted in | Task 5 `expired_pinned_cert_accepted_when_opted_in`; Task 2 keystore and truststore direct scans |
| E2E signed envelope reaches the route | Task 5 `expired_pinned_cert_e2e_reaches_route_with_signer` |
| Expired pinned cert rejected by default | Task 5 `expired_pinned_cert_rejected_by_default`; Task 3 `default_off_keeps_stock_merlin_behavior` |
| Expired cert rejected when not pinned | Task 5 `expired_cert_rejected_when_not_pinned` |
| Not-yet-valid cert rejected | Task 6 `not_yet_valid_cert_rejected`; Task 2 `notYetValidCertRejectedByMerlin` |
| Chain with expired leaf rejected | Task 6 `chain_with_expired_leaf_rejected`; Task 2 `chainWithExpiredLeafRejectedByMerlin` |
| Pinned issuer does not authorize expired leaf | Task 6 `pinned_issuer_does_not_authorize_expired_leaf`; Task 2 `pinnedIssuerDoesNotAuthorizeExpiredLeafByMerlin` |
| Revocation enabled disables the opt-in | Task 2 `revocation_enabled_disables_the_opt_in` |
| Startup rejects flag without Signature action | Task 3 `startup_rejects_flag_without_signature_action` |
| Startup rejects flag without verification material | Task 3 `startup_rejects_flag_without_verification_material` |
| Startup warns about the accepted risk | Task 3 `startup_warns_about_accepted_risk` |
| Default inbound Signature action permits the opt-in | Task 3 `default_inbound_signature_action_permits_the_opt_in` |
| SHA-1 and expiry opt-ins compose | Task 6 `sha1_and_expiry_opt_ins_compose` |
| SHA-1 allow-list off rejects with the algorithm error | Task 6 `sha1_and_expiry_opt_ins_compose` (generic `INVALID_SECURITY`; URI only in WARN) |
| Expiry opt-in off rejects with invalidCert | Task 6 `sha1_and_expiry_opt_ins_compose` |
| Env var reaches the bridge profile | Task 4 config/pool/process tests; Task 3 `store_parses_allow_expired_pinned_certificate_env` |
| Default off keeps stock Merlin behavior | Task 3 `default_off_keeps_stock_merlin_behavior` |
| Producer path unaffected | Task 6 `producer_profile_in_interceptor_rejects_expired_pinned_response` |

**New-symbol consistency.** `PinnedTrustMerlin(Properties, boolean)` and its three-argument
`verifyTrust` override are introduced in Task 2 and used by Task 3. `allowExpiredPinnedCertificate`
(Builder and profile accessor) is introduced in Task 3 and used by Tasks 5 and 6 (and documented in Task 7).
`allow_expired_pinned_certificate` is introduced in Task 4 in `CxfSecurityFields` and
`CxfProfileEnvVars`, and consumed by the Task 3 Java env parser; the Java-side accessor is
`allowExpiredPinnedCertificate`. The fixture methods and
`ExpiredSignedEnvelopeFactory.signSoap11` are introduced in Task 1 and used by Tasks 5 and 6 (the direct-Crypto test in Task 2 uses only
`certificateFrom` and the chain helpers) with the exact names above.

**Placeholder scan.** The file contains no deferred-implementation markers, no ellipsis used
as content, and no placeholder tokens; every step names a concrete file, method, command, or
assertion.
