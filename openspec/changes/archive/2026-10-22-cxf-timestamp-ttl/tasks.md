# Tasks: cxf-timestamp-ttl

Per-profile WS-Security Timestamp freshness window (`timestamp_ttl_seconds`, default 300;
`timestamp_future_tolerance_seconds`, default 60) for the consumer inbound path. The design
is fixed by the blessed proposal/design/specs. Do not touch the replay caches, the
required-actions check, `verifyTimestampSignatureCoverage`, `createInInterceptor()`, or any
SOAP-header allow-list code (parallel mission 375 edits the same files; keep every edit
additive: new fields, new methods, new tests, new doc sections).

## Verification guardrails

**Rust containment.** Every Rust `cargo` command (test, check, clippy, fmt, xtask) runs in
the worktree `/home/shared/rust-camel-worktrees/378-cxf-timestamp-ttl`, never in the main
checkout, with `TMPDIR=/home/shared/tmp`. Tests run inside a fresh user scope:

```bash
bash -c '
set -u
if [ "$(pgrep -c compiled_artifa || true)" -ne 0 ]; then echo "precondition: stray compiled_artifa"; exit 2; fi
UNIT="rc378-$(date +%s%N)"
TMPDIR=/home/shared/tmp systemd-run --user --scope --collect --unit="$UNIT" \
  -p MemoryMax=12G -p MemorySwapMax=2G -p TasksMax=1024 -p OOMPolicy=kill \
  -- env TMPDIR=/home/shared/tmp CARGO_BUILD_JOBS=6 \
  cargo test -p camel-component-cxf
RC=$?
systemctl --user stop "$UNIT" 2>/dev/null || true
if [ "$(pgrep -c compiled_artifa || true)" -ne 0 ]; then echo "postcondition: stray compiled_artifa"; exit 3; fi
exit $RC
'
```

The block exits non-zero when the precondition fails, when `systemd-run` or the command
fails, or when the postcondition fails. Replace only the final `cargo ...` line with the
command a task names. Run it with the tool `workdir` set to the worktree root.

**Java verification.** Set the tool `workdir` to
`/home/shared/rust-camel-worktrees/378-cxf-timestamp-ttl/bridges/cxf` (no shell `cd`).
Targeted run: `TMPDIR=/home/shared/tmp ./gradlew --no-daemon test --tests '<FQCN>'`.
Every Java task ends with `TMPDIR=/home/shared/tmp ./gradlew --no-daemon spotlessApply`
followed by the mandatory `TMPDIR=/home/shared/tmp ./gradlew --no-daemon spotlessCheck test`.

**TDD.** Write the named tests first, run them, observe the stated red (compile failure on the
absent symbol counts as red for new-symbol tests), then implement until green.

## Java bridge

### Task 1: Profile fields, builder validation, env parsing, and startup WARN

**Files:**
- `bridges/cxf/src/main/java/org/rustcamel/cxf/SecurityProfile.java` (modified)
- `bridges/cxf/src/main/java/org/rustcamel/cxf/SecurityProfileStore.java` (modified)
- `bridges/cxf/src/test/java/org/rustcamel/cxf/SecurityProfileTest.java` (modified)
- `bridges/cxf/src/test/java/org/rustcamel/cxf/SecurityProfileStoreTest.java` (modified)

**Steps:**
1. In `SecurityProfile`, add `public static final int DEFAULT_TIMESTAMP_TTL_SECONDS = 300;`
   and `public static final int DEFAULT_TIMESTAMP_FUTURE_TOLERANCE_SECONDS = 60;` next to the
   other public constants.
2. Add `private final Integer timestampTtlSeconds;` and
   `private final Integer timestampFutureToleranceSeconds;` (comment
   `// --- Inbound Timestamp freshness window (null = WSS4J default) ---`) after
   `allowExpiredPinnedCertificate`; assign both in the private constructor from the builder.
3. Add accessors returning the effective `int`:
   `public int timestampTtlSeconds()` → configured value or `DEFAULT_TIMESTAMP_TTL_SECONDS`;
   `public int timestampFutureToleranceSeconds()` → configured value or
   `DEFAULT_TIMESTAMP_FUTURE_TOLERANCE_SECONDS`. Javadoc: consumer inbound path only.
4. In `Builder`, add fields `private Integer timestampTtlSeconds;` and
   `private Integer timestampFutureToleranceSeconds;` and setters
   `public Builder timestampTtlSeconds(Integer v)` and
   `public Builder timestampFutureToleranceSeconds(Integer v)` (null = default).
5. Add `private void validateTimestampWindow()` to `Builder` and call it in `build()` directly
   after `validateExpiredPinnedCertificate()`. For each knob with a non-null value, with
   `envVar = "CXF_PROFILE_" + name.toUpperCase(Locale.ROOT) + "_TIMESTAMP_TTL_SECONDS"` or
   `"..._TIMESTAMP_FUTURE_TOLERANCE_SECONDS"`:
   - value `<= 0` → `throw new IllegalArgumentException(envVar + " must be a positive integer number of seconds, but was " + v)`;
   - `!hasText(truststorePath) && !hasText(keystorePath)` →
     `throw new IllegalArgumentException(envVar + " is set but the profile has no truststore or keystore; the timestamp window would be silently inert without verification material (cxf.truststore.path or cxf.keystore.path)")`.
6. In `SecurityProfileStore`, add
   `static Integer parseTimestampSeconds(String envVar, String raw)`:
   - `raw == null || raw.isBlank()` → return `null`;
   - `long v = Long.parseUnsignedLong(raw.trim())`; on `NumberFormatException` and when
     `v == 0` → `throw new IllegalStateException(envVar + " must be a positive integer number of seconds when set, but was '" + raw + "'")`;
   - `Long.compareUnsigned(v, Integer.MAX_VALUE) > 0` →
     `throw new IllegalStateException(envVar + " must be at most 2147483647 seconds (WSS4J int limit), but was '" + raw + "'")`
     (owner ruling 2026-10-21: reject, never clamp);
   - return `(int) v`.
   Javadoc: accepts 1 to 2147483647 (the WSS4J `int` limit, about 68 years); larger values
   fail startup.
7. In the store constructor, next to the expired-pinned-cert parsing, parse
   `prefix + "TIMESTAMP_TTL_SECONDS"` and `prefix + "TIMESTAMP_FUTURE_TOLERANCE_SECONDS"` with
   `parseTimestampSeconds`, and pass them to `.timestampTtlSeconds(..)` and
   `.timestampFutureToleranceSeconds(..)` on the builder chain (before `.faultStatus`).
8. Add `static Optional<String> timestampWindowWarning(SecurityProfile profile)`: empty when
   `profile.timestampTtlSeconds() <= DEFAULT_TIMESTAMP_TTL_SECONDS` and
   `profile.timestampFutureToleranceSeconds() <= DEFAULT_TIMESTAMP_FUTURE_TOLERANCE_SECONDS`;
   otherwise exactly:
   `"WS-Security profile '" + name + "' widens the inbound Timestamp window (TIMESTAMP_TTL_SECONDS=" + ttl + ", TIMESTAMP_FUTURE_TOLERANCE_SECONDS=" + future + "; defaults 300/60); a captured signed message stays acceptable for longer while replay retention stays at most 3600 s; accepted risk, see ADR-0088"`.
   Log it in the constructor with `timestampWindowWarning(profile).ifPresent(LOG::warning);`
   directly after the `expiredPinnedCertWarning` line.

**Tests:**
- `SecurityProfileTest.timestamp_window_defaults_to_300_and_60`: builder with truststore only →
  `build()` → `timestampTtlSeconds() == 300`, `timestampFutureToleranceSeconds() == 60`.
- `SecurityProfileTest.timestamp_window_rejects_zero_and_negative` (parameterized over both
  knobs × values `0`, `-1`): builder with truststore → `build()` → `IllegalArgumentException`
  whose message contains the knob's `CXF_PROFILE_<NAME>_TIMESTAMP_...` name and
  `positive integer`.
- `SecurityProfileTest.timestamp_window_rejects_inert_profile` (both knobs): builder with no
  truststore and no keystore, knob `900` → `IllegalArgumentException` containing
  `no truststore or keystore`.
- `SecurityProfileTest.timestamp_window_accepts_int_max`: both knobs `Integer.MAX_VALUE` with a
  truststore → accessors return `Integer.MAX_VALUE`.
- `SecurityProfileStoreTest.store_parses_timestamp_window_env`: env `CXF_PROFILES=win`,
  keystore path/password (existing `ksFile`), `..._TIMESTAMP_TTL_SECONDS=900`,
  `..._TIMESTAMP_FUTURE_TOLERANCE_SECONDS=120` → profile accessors return 900 and 120.
- `SecurityProfileStoreTest.store_defaults_timestamp_window`: same env without the two vars →
  300 and 60.
- `SecurityProfileStoreTest.store_rejects_invalid_timestamp_window` (parameterized over both
  variables × `0`, `-5`, `abc`): `createStore(env)` → `IllegalStateException` whose message
  contains the full variable name.
- `SecurityProfileStoreTest.store_rejects_timestamp_window_above_int_max` (parameterized over
  both variables × `2147483648`, `18446744073709551615`): `createStore(env)` →
  `IllegalStateException` whose message contains the full variable name and `2147483647`.
- `SecurityProfileStoreTest.store_accepts_timestamp_window_at_int_max` (parameterized over
  both variables): value `2147483647` → effective accessor value is `2147483647`.
- `SecurityProfileStoreTest.timestamp_window_warning_decision`: `timestampWindowWarning` is
  empty for defaults, for TTL `120`, and for TTL `300`/future `60`. For TTL `900` alone the
  message contains `TIMESTAMP_TTL_SECONDS=900` and `TIMESTAMP_FUTURE_TOLERANCE_SECONDS=60`;
  for future tolerance `120` alone it contains `TIMESTAMP_TTL_SECONDS=300` and
  `TIMESTAMP_FUTURE_TOLERANCE_SECONDS=120`. Every present message contains the profile name,
  `ADR-0088`, and `3600 s`.
- `SecurityProfileStoreTest.store_logs_timestamp_window_warning` (parameterized): reuse the
  existing `Handler`/`LogRecord` capture pattern of the SHA-1 WARN test in this class; profile
  `win` with keystore; cases (TTL env, future env, expected WARNING count): (`900`, unset, 1),
  (unset, `120`, 1), (`900`, `120`, 1), (unset, unset, 0), (`120`, unset, 0). Count WARNING
  records containing `widens the inbound Timestamp window`; each counted record contains
  `'win'` and `ADR-0088`.

Red before implementation: compile failure on the absent builder setters/accessors/store
methods.

**Acceptance:**
- `./gradlew --no-daemon test --tests 'org.rustcamel.cxf.SecurityProfileTest' --tests 'org.rustcamel.cxf.SecurityProfileStoreTest'` passes.
- `./gradlew --no-daemon spotlessCheck test` passes.
- `createInInterceptor()` and every SOAP-header allow-list path are unchanged
  (`git diff` shows only additions in those regions).

- [x] 1

### Task 2: Wire the window into the consumer RequestData and prove behaviour

**Files:**
- `bridges/cxf/src/main/java/org/rustcamel/cxf/WssSecurityProcessor.java` (modified)
- `bridges/cxf/src/test/java/org/rustcamel/cxf/WssSecurityProcessorTimestampWindowTest.java` (new)
- `bridges/cxf/src/test/java/org/rustcamel/cxf/SecurityProfileTest.java` (modified)

**Steps:**
1. In `WssSecurityProcessor.processInbound`, directly after
   `requestData.setNonceReplayCache(replayCache);`, add
   `requestData.setTimeStampTTL(profile.timestampTtlSeconds());` and
   `requestData.setTimeStampFutureTTL(profile.timestampFutureToleranceSeconds());` with a
   one-line comment: per-profile freshness window; defaults equal the WSS4J defaults.
2. Create `WssSecurityProcessorTimestampWindowTest` (package `org.rustcamel.cxf`).
   `@BeforeAll` creates `keystorePath = TestKeystoreHelper.createTestKeystore()`; `@AfterAll`
   deletes it. Helper `profile(Integer ttl, Integer future)` builds
   `SecurityProfile.builder("window")` with `.keystore(ks, "changeit")`,
   `.truststore(ks, "changeit")`, `.sigUser("alice", "changeit")`,
   `.actionsIn("Timestamp Signature")`, `.timestampTtlSeconds(ttl)`,
   `.timestampFutureToleranceSeconds(future)`. Helper
   `signedEnvelope(Instant created, int expiresAfterCreatedSeconds)`: parse the SOAP 1.1
   envelope used by `WssSecurityProcessorTimestampCoverageTest` with
   `SoapEnvelopeHelper.configureSecure(DocumentBuilderFactory.newInstance())`
   (namespace-aware), `WSSecHeader.insertSecurityHeader()`, `WSSecTimestamp` with
   `setWsTimeSource(() -> created)` and `setTimeToLive(expiresAfterCreatedSeconds)` then
   `build()`, then `WSSecSignature` with `setUserInfo("alice", "changeit")`,
   `setKeyIdentifierType(WSConstants.BST_DIRECT_REFERENCE)`, parts Body
   (envelope NS, `"Content"`) and `Timestamp` (`WSConstants.WSU_NS`, `""`), RSA-SHA256
   signature and SHA-256 digest, `build(profile(null, null).getSignatureCrypto())`; return
   `SoapEnvelopeHelper.sourceToString(new DOMSource(doc))`.
   Every default-versus-raised comparison uses a NEW `WssSecurityProcessor` per call so the
   replay cache never couples them.
3. In `SecurityProfileTest`, add the producer non-coverage test below.

**Tests:**
- `oldCreated_rejectedByDefault_acceptedWithRaisedTtl`: envelope `Created = now - 600 s`,
  `Expires` after 3600 s → `new WssSecurityProcessor(profile(null, null)).processInbound(xml)`
  throws `WSSecurityException` whose message contains `out of range`; then
  `new WssSecurityProcessor(profile(900, null)).processInbound(xml)` does not throw.
- `futureCreated_rejectedByDefault_acceptedWithRaisedTolerance`: `Created = now + 120 s`,
  `Expires` after 600 s → default profile throws with `out of range`; `profile(null, 300)`
  accepts.
- `fixtureWithMultiYearWindow_accepted`: `Created = now - 9 * 365 days`, `Expires` after
  `315_360_000` s → `profile(315_360_000, null)` accepts.
- `expiredExpires_stillRejected_withRaisedTtl`: `Created = now - 600 s`, `Expires` after
  300 s → `profile(900, null)` throws with message containing `expired`.
- `replay_stillRejected_withRaisedTtl`: `Created = now - 600 s`, `Expires` after 3600 s; ONE
  processor for `profile(900, null)` → first `processInbound` succeeds; second call with the
  identical string throws `WSSecurityException` whose message contains `replay`.
- `SecurityProfileTest.producer_in_interceptor_ignores_timestamp_window`: profile with
  truststore, `actionsIn("Timestamp Signature")`, TTL 900, future 300 →
  `(AbstractWSS4JInterceptor) profile.createInInterceptor()` → `getProperties()` contains
  neither `ConfigurationConstants.TTL_TIMESTAMP` nor
  `ConfigurationConstants.TTL_FUTURE_TIMESTAMP`.

Red before Step 1: `oldCreated_...` and `futureCreated_...` fail on the raised-window leg
(WSS4J defaults still apply); `fixtureWithMultiYearWindow_accepted` fails. The remaining
tests pass before and after (they pin unchanged invariants).

**Acceptance:**
- `./gradlew --no-daemon test --tests 'org.rustcamel.cxf.WssSecurityProcessorTimestampWindowTest' --tests 'org.rustcamel.cxf.SecurityProfileTest'` passes.
- `./gradlew --no-daemon spotlessCheck test` passes (whole suite).
- The diff to `WssSecurityProcessor.java` is exactly the two setter lines and the comment.

- [x] 2

## Rust plumbing

### Task 3: TOML fields and fail-closed validation in camel-cxf

**Files:**
- `crates/components/camel-cxf/src/config.rs` (modified)

**Steps:**
1. Add to `CxfSecurityFields`, after `allow_expired_pinned_certificate`:
   ```rust
   /// Consumer inbound Timestamp TTL in seconds (`Created` may be at most this old).
    /// `None` keeps the bridge default (300). Accepted range: 1..=2147483647.
   #[serde(default)]
   pub timestamp_ttl_seconds: Option<u64>,
   /// Consumer inbound Timestamp future tolerance in seconds (`Created` may be at
    /// most this far ahead). `None` keeps the bridge default (60). Range: 1..=2147483647.
   #[serde(default)]
   pub timestamp_future_tolerance_seconds: Option<u64>,
   ```
2. Add both fields to the manual `Debug` impl (plain values) after
   `allow_expired_pinned_certificate`.
3. Add `fn validate_timestamp_window(&self, profile: &str) -> Result<(), camel_component_api::CamelError>`
   to `impl CxfSecurityFields`. For each `(key, value)` in
   `[("timestamp_ttl_seconds", self.timestamp_ttl_seconds), ("timestamp_future_tolerance_seconds", self.timestamp_future_tolerance_seconds)]`
   with `Some(v)`:
   - `v == 0` → `Err(CamelError::Config(format!("cxf profile '{profile}': security.{key} must be a positive number of seconds; 0 is not allowed")))`;
   - `v > i32::MAX as u64` → `Err(CamelError::Config(format!("cxf profile {profile}: security.{key} must be at most 2147483647 seconds (WSS4J int limit), got {v}")))`
     (owner ruling 2026-10-21: reject, never clamp);
   - no non-blank `keystore_path` and no `truststore_path` →
     `Err(CamelError::Config(format!("cxf profile '{profile}': security.{key} is set but the profile has no inbound verification material (neither security.keystore_path nor security.truststore_path); the timestamp window would be silently inert — configure a truststore or keystore, or remove the option")))`.
4. In `CxfPoolConfig::validate`, call
   `profile.security.validate_timestamp_window(&profile.name)?;` directly after
   `validate_allowed_algorithms`.

**Tests:** (in the existing `#[cfg(test)] mod tests` of `config.rs`, new section comment
`// ── Timestamp freshness window ──`)
- `timestamp_window_defaults_unset`: TOML profile with truststore only → both fields `None`;
  `cfg.validate()` is `Ok`.
- `timestamp_window_parses_from_toml`: `timestamp_ttl_seconds = 900`,
  `timestamp_future_tolerance_seconds = 120`, truststore → fields `Some(900)`/`Some(120)`;
  `validate()` `Ok`; `format!("{:?}", security)` contains `timestamp_ttl_seconds: Some(900)`.
- `timestamp_window_zero_rejected`: for each key, TOML with `<key> = 0` and a truststore →
  `validate()` returns `Err(CamelError::Config(msg))` with `msg` containing `'partner'`,
  `security.<key>`, and `positive`.
- `timestamp_window_int_max_accepted`: both keys `= 2147483647` → parse gives
  `Some(2147483647)`; `validate()` `Ok`.
- `timestamp_window_above_int_max_rejected`: for each key and each value `2147483648` and
  `u64::MAX` (directly constructed config, truststore set) → `validate()` returns
  `Err(CamelError::Config(msg))` with `msg` containing `security.<key>` and `2147483647`.
- `timestamp_window_without_material_rejected`: for each key, `<key> = 900` with no
  keystore/truststore → `Err(CamelError::Config(msg))`, `msg` contains
  `no inbound verification material`.

Red: compile failure on the absent fields/method.

**Acceptance:**
- Contained `cargo test -p camel-component-cxf --lib config` passes.
- `cargo clippy -p camel-component-cxf --all-targets -- -D warnings` exits 0 (may need Task 4
  for the crate to compile if `pool.rs` struct literals fail; if so run clippy after Task 4).

- [x] 3

### Task 4: Env emission in camel-bridge and pool mapping

**Files:**
- `crates/services/camel-bridge/src/process.rs` (modified)
- `crates/components/camel-cxf/src/pool.rs` (modified)
- `crates/components/camel-cxf/src/pool_env_test.rs` (modified)
- `crates/camel-test/tests/cxf_test.rs` (modified)

**Steps:**
1. Add to `CxfProfileEnvVars`, after `allow_expired_pinned_certificate`:
   `pub timestamp_ttl_seconds: Option<u64>,` (doc: emitted as
   `CXF_PROFILE_<NAME>_TIMESTAMP_TTL_SECONDS` only when set) and
   `pub timestamp_future_tolerance_seconds: Option<u64>,` (doc: emitted as
   `CXF_PROFILE_<NAME>_TIMESTAMP_FUTURE_TOLERANCE_SECONDS` only when set).
2. In `to_env_vars`, after the `ALLOW_EXPIRED_PINNED_CERTIFICATE` block, push
   `(format!("{}TIMESTAMP_TTL_SECONDS", prefix), v.to_string())` and
   `(format!("{}TIMESTAMP_FUTURE_TOLERANCE_SECONDS", prefix), v.to_string())` for `Some(v)`.
3. Add `timestamp_ttl_seconds: None, timestamp_future_tolerance_seconds: None,` after
   `allow_expired_pinned_certificate: ...` in every other `CxfProfileEnvVars { .. }` literal:
   `process.rs` tests (lines near 1139, 1164, 1271, 1377, 1590), `pool_env_test.rs`
   `secret_profiles`, and `crates/camel-test/tests/cxf_test.rs` (near lines 458, 756, 1345).
   Find them all with `rg -n "allow_expired_pinned_certificate:" crates`.
4. In `pool::profile_env_vars_for`, map
   `timestamp_ttl_seconds: p.security.timestamp_ttl_seconds,` and
   `timestamp_future_tolerance_seconds: p.security.timestamp_future_tolerance_seconds,`.
5. In `process.rs` test `profile_env_vars_to_env_vars_includes_all_fields`, set the two
   fields to `Some(900)` and `Some(120)`, update the count comment to
   `+ 2 timestamp window = 18` and the assertion to `assert_eq!(env.len(), 18);`, and assert
   both keys carry `"900"` and `"120"`.

**Tests:**
- `process.rs::profile_env_vars_to_env_vars_includes_all_fields` (updated as Step 5).
- `process.rs::profile_env_vars_omits_timestamp_window_when_unset`: literal with both `None`
  → no key ends with `TIMESTAMP_TTL_SECONDS` or `TIMESTAMP_FUTURE_TOLERANCE_SECONDS`.
- `process.rs::profile_env_vars_emits_u64_max_timestamp_window_unchanged`: both
  `Some(u64::MAX)` → both env values equal `"18446744073709551615"`.
- `pool_env_test.rs::pool_env_carries_timestamp_window_from_profile`: `CxfProfileConfig` with
  `CxfSecurityFields { truststore_path: Some(..), truststore_password: Some(..), timestamp_ttl_seconds: Some(900), timestamp_future_tolerance_seconds: Some(120), ..Default::default() }`
  → `profile_env_vars_for` → `bridge_config_with_env_trace("slot-ts", PathBuf::from("/tmp/cxf-bridge"), &profiles, None, 15_000, None)`
  → `env_map(&config)` has `CXF_PROFILE_<NAME>_TIMESTAMP_TTL_SECONDS = "900"` and
  `..._TIMESTAMP_FUTURE_TOLERANCE_SECONDS = "120"`; a default profile emits neither.
- `pool_env_test.rs::pool_env_carries_int_max_timestamp_window_from_toml`: parse a TOML
  `CxfPoolConfig` (one profile `huge`, truststore path/password, both keys
  `= 2147483647`) with `toml::from_str`, assert `cfg.validate().is_ok()`, then
  `profile_env_vars_for(&cfg.profiles)` → `bridge_config_with_env_trace("slot-huge", ...)` →
  `env_map` has both `CXF_PROFILE_HUGE_TIMESTAMP_TTL_SECONDS` and
  `CXF_PROFILE_HUGE_TIMESTAMP_FUTURE_TOLERANCE_SECONDS` equal to `"2147483647"`.

Red: compile failure on the absent fields.

**Acceptance:**
- Contained `cargo test -p camel-bridge --lib` passes.
- Contained `cargo test -p camel-component-cxf` passes.
- Contained `cargo check -p camel-test --features integration-tests-cxf --test cxf_test`
  passes (compiles the updated `cxf_test.rs` literals; runs no Docker test).
- `cargo clippy -p camel-bridge -p camel-component-cxf --all-targets -- -D warnings` exits 0;
  `cargo fmt --check --all` exits 0.

- [x] 4

## Documentation

### Task 5: ADR-0088, ws-soap.md, bridge README, CONTEXT files

**Files:**
- `docs/adr/0088-cxf-timestamp-window.md` (new)
- `docs/src/components/ws-soap.md` (modified)
- `bridges/cxf/README.md` (modified)
- `crates/components/camel-cxf/CONTEXT.md` (modified)
- `CONTEXT-MAP.md` (modified)

**Steps:**
1. Write ADR-0088 with the section layout of ADR-0087 (Status
   `Accepted (2026-10-21; bd rc-a6dyc)`, Source, Cross-refs ADR-0032/0033/0085/0087;
   Context; Decision; Accepted risks; Rejected alternatives; Consequences; References).
   Content: partner camel-112 multi-year fixtures and drifting peer clocks; the two knobs,
   defaults 300/60, validation (0, inert, garbage, above 2147483647); consumer-only, producer
   unchanged; WARN text summary. Upper bound (owner ruling 2026-10-21):
   `RequestData.setTimeStampTTL(int)` / `setTimeStampFutureTTL(int)` take `int`, so values
   are accepted from 1 to 2147483647 s (about 68 years) and larger values are rejected at
   startup; no silent clamp. Rejected alternatives include the custom long-arithmetic
   `Timestamp` validator (re-implements WSS4J freshness/BSP checks for a range nobody needs)
   and silent clamping (a silently clamped security window hides the effective value). Replay facts
   exactly as `design.md` states them: one shared `MemoryReplayCache` per profile; key =
   `Created` + hash of the signature value + hash of the verification key encoding;
   retention = `Expires` when in the future and at most 3600 s away, else 300 s; a
   multi-year fixture is remembered for 300 s; a larger TTL widens acceptance, never
   retention. Accepted risk: a captured signed message is replayable after its cache entry
   expires while it remains inside the configured window.
2. In `ws-soap.md`, add `### Timestamp freshness window (consumer)` directly before
   `### Producer transport TLS`: a table `| Option | Environment variable | Default |` with
   both rows; the validation rules; the maximum 2147483647 s (larger values rejected at
   startup); the WARN; consumer-only scope; the replay
   facts (one paragraph); a TOML example with `timestamp_ttl_seconds = 315360000` in
   `[components.cxf.profiles.security]`. English only, no bridge version numbers.
3. In `bridges/cxf/README.md`, add two env-table rows after the `ALLOWED_DIGEST_ALGORITHMS`
   row: `CXF_PROFILE_<NAME>_TIMESTAMP_TTL_SECONDS` default `300` and
   `CXF_PROFILE_<NAME>_TIMESTAMP_FUTURE_TOLERANCE_SECONDS` default `60`, each with a
   one-sentence description (consumer inbound only; 1 to 2147483647, larger values rejected
   at startup; see ADR-0088).
4. In `crates/components/camel-cxf/CONTEXT.md`, after the `fault_status` paragraph near
   line 130, add a short paragraph describing the two optional knobs, `CamelError::Config`
   on 0 or without verification material, and the env names.
5. In `CONTEXT-MAP.md`, add an ADR-list entry for 0088 directly after the 0087 entry, same
   style (one line, ending `(camel-component-cxf + bridges/cxf)`).

**Tests:**
- `mdbook test` (workdir `docs`): passes.
- `cargo xtask lint-bridge-versions` exits 0 (no semver in docs).
- `cargo xtask lint-context-citations` exits 0.

**Acceptance:**
- The three commands above exit 0 (xtask commands contained per guardrails).
- `rg -n "3600" docs/adr/0088-cxf-timestamp-window.md` shows retention stated as "at most
  3600 s", never as extended by the TTL.

- [x] 5

## Verification

### Task 6: Run the blessed acceptance gates

**Files:**
- none modified (verification only; fix failures in the owning task's files and re-run)

**Steps:**
Run each command separately (Rust/xtask commands inside the containment block, workdir =
worktree root; Java with workdir `bridges/cxf`; mdbook with workdir `docs`) and record its
exit code:
1. `cargo fmt --check --all`
2. `cargo clippy -p camel-component-cxf -p camel-bridge --all-targets -- -D warnings`
3. `cargo clippy --workspace --all-features --exclude camel-cli --exclude camel-component-kafka --exclude security-keycloak --exclude security-wasm-policy -- -D warnings`
4. `cargo test -p camel-component-cxf` and `cargo test -p camel-bridge`
5. `cargo check -p camel-test --features integration-tests-cxf --test cxf_test`
6. `cargo test -p camel-cli --test feature_profiles` (golden unchanged: no new crate or
   feature; a golden diff is a failure to investigate, not to regenerate)
7. Every `cargo xtask lint-*` gate listed in `AGENTS.md ## QUALITY GATES` except
   `lint-commits` (remote fetch; CI-owned), including `lint-bridge-versions`
8. `cargo xtask schema --check`
9. `mdbook test` (workdir `docs`)
10. `TMPDIR=/home/shared/tmp ./gradlew --no-daemon spotlessCheck test` (workdir `bridges/cxf`)
11. `openspec validate cxf-timestamp-ttl --type change --json` (workdir worktree root)

**Tests:**
- The commands above are the tests; each must exit 0.

**Acceptance:**
- All 11 steps exit 0, or a failure is shown to exist unchanged on `main` and is reported
  with its bd follow-up id.
- `bridges/versions.toml` is unchanged (`git diff main -- bridges/versions.toml` is empty).

- [x] 6

## Self-review coverage map

| Spec scenario | Task |
| --- | --- |
| TOML values reach the bridge environment | 3, 4 |
| Unset values keep the defaults | 1, 3, 4 |
| Maximum value accepted | 1, 3, 4 |
| Value above the maximum rejected | 1, 3 |
| Zero rejected in Rust | 3 |
| Zero or garbage rejected in the bridge | 1 |
| Inert value rejected | 1, 3 |
| WARN above default | 1 |
| No WARN at or below default | 1 |
| Old Created accepted only with a raised TTL | 2 |
| Future Created accepted only with a raised tolerance | 2 |
| Fixture with a multi-year window | 2 |
| Expired Expires still rejected with a raised TTL | 2 |
| Replay still rejected with a raised TTL | 2 |
| Producer path unchanged | 2 |
