# Design: cxf-timestamp-ttl

## Approach

Facts the design rests on (WSS4J 4.0.1, verified with `javap`):

- `RequestData` has `setTimeStampTTL(int)` and `setTimeStampFutureTTL(int)`, defaults 300 and 60. `TimestampValidator` reads both from `RequestData` and calls `DateUtil.verifyCreated(created, ttl, futureTtl)`, which uses `Instant.minusSeconds`/`plusSeconds` on the `long` widening of the `int`. `Integer.MAX_VALUE` does not overflow.
- `TimestampValidator` rejects an expired `Expires` (strict mode, default) independently of the TTL. A raised TTL does not accept a message whose `Expires` has passed.
- The WSS4J API is `int` seconds. `Integer.MAX_VALUE` s is about 68 years.
- Replay cache (unchanged, documented only; verified with `javap -c` on `SignatureProcessor.testMessageReplay` and `MemoryReplayCache.add(String, Instant)`): one shared `MemoryReplayCache` per security profile (`WssSecurityProcessor` field, set as both the timestamp and the nonce replay cache). For a signed message whose signature follows a `Timestamp`, the replay key is the `Created` string plus `Arrays.hashCode` of the signature value bytes plus `Arrays.hashCode` of the verification key encoding. It does not use `wsu:Id`. A byte-identical resend is rejected with `A replay attack has been detected` while the entry is live. Entry lifetime: the `Expires` instant when it is in the future and at most `MAX_TTL` = 3600 s away; otherwise `now + DEFAULT_TTL` = 300 s (no `Expires`, past `Expires`, or `Expires` more than 3600 s away). Retention never exceeds one hour, and a multi-year fixture `Expires` gives 300 s.
- The order's summary ("key derives from Created", "lifetime = Expires clamped to 3600 s") is corrected here by the bytecode; ADR-0088 and `ws-soap.md` state the corrected facts.

**R1/R2 — Rust config.** `CxfSecurityFields` gains `timestamp_ttl_seconds: Option<u64>` and `timestamp_future_tolerance_seconds: Option<u64>` (`#[serde(default)]`, plain values in `Debug`). `CxfPoolConfig::validate` rejects `Some(0)` with `CamelError::Config` naming the profile and the key, and rejects a set value when the profile has neither `keystore_path` nor `truststore_path` (inert config, dead-config policy). `None` means the bridge default. It also rejects `Some(v)` with `v > i32::MAX as u64` (2147483647) with `CamelError::Config` naming the key and the maximum (owner ruling 2026-10-21: reject, never clamp silently; same practice as the repo's other int-limited settings such as `timeout_ms`).

**R1 — Env.** `CxfProfileEnvVars` gains the same two `Option<u64>` fields; `to_env_vars` emits `CXF_PROFILE_<NAME>_TIMESTAMP_TTL_SECONDS` and `CXF_PROFILE_<NAME>_TIMESTAMP_FUTURE_TOLERANCE_SECONDS` only when `Some`. `pool::profile_env_vars_for` copies them.

**R2 — Java parse.** `SecurityProfileStore.parseTimestampSeconds(envVar, raw)` returns `null` for absent or blank; parses with `Long.parseUnsignedLong(raw.trim())`; rejects `0`, negative, and non-integer input with `IllegalStateException` naming the variable; rejects values above `Integer.MAX_VALUE` with `IllegalStateException` naming the variable and the maximum 2147483647. R4 names `RequestData.setTimeStampTTL(int)` and `setTimeStampFutureTTL(int)` as the enforcement seam, so `Integer.MAX_VALUE` s (about 68 years) is the largest window that seam can express. Owner ruling (2026-10-21): accept 1..=2147483647, reject larger values at startup, no silent clamp, no custom validator. A silently clamped security window would fail review.

**R2 — Builder.** `SecurityProfile.Builder.timestampTtlSeconds(Integer)` and `timestampFutureToleranceSeconds(Integer)`; `null` means default. `build()` calls a new `validateTimestampWindow()`: a non-null value `<= 0` throws `IllegalArgumentException` naming the variable; a non-null value on a profile with no truststore and no keystore throws `IllegalArgumentException` (mirrors the `ALLOWED_*` material check). Accessors `timestampTtlSeconds()` and `timestampFutureToleranceSeconds()` return the effective `int` (configured or `DEFAULT_TIMESTAMP_TTL_SECONDS` = 300 / `DEFAULT_TIMESTAMP_FUTURE_TOLERANCE_SECONDS` = 60).

**R3 — WARN.** `SecurityProfileStore.timestampWindowWarning(profile)` is a pure decision point like `expiredPinnedCertWarning`. It returns one message when either effective value is above its default. The message names the profile, both effective values, ADR-0088, and that replay retention stays at most 3600 s. Values at or below the default produce no WARN. The store logs it after the existing warnings; a store test captures the log record.

**R4 — Wiring.** `WssSecurityProcessor.processInbound` sets both values on its `RequestData` next to the replay-cache setters. With no configuration the values equal the WSS4J defaults, so behavior is unchanged. `SecurityProfile.createInInterceptor()` (producer) is not changed.

**R5 — Docs.** `ws-soap.md` gains a "Timestamp freshness window (consumer)" section with the option table, the env variables, the validation rules, the 2147483647 s maximum, and the replay facts. New ADR-0088 records the trade-off; `CONTEXT-MAP.md` lists it. No bridge version appears in docs.

**R7 — Invariant.** The replay caches, the required-actions check, and `verifyTimestampSignatureCoverage` are untouched. A test proves a byte-identical resend is still rejected with a raised TTL.

**Overlap with mission 375.** 375 edits `SecurityProfile.java`, `SecurityProfileStore.java`, `WssSecurityProcessor.java`, `CxfProfileEnvVars`, `config.rs`, and `ws-soap.md` for the SOAP-header allow-list. This change is additive in those files (new fields, setters, methods, sections) and does not touch the allow-list code paths.

## Affected crates

- `crates/components/camel-cxf`: two `CxfSecurityFields` fields, validation, `Debug`, pool mapping, tests.
- `crates/services/camel-bridge`: two `CxfProfileEnvVars` fields and env emission.
- `crates/camel-test`: `CxfProfileEnvVars` struct literals gain `None` fields (compile only).
- `bridges/cxf` (Java): `SecurityProfile`, `SecurityProfileStore`, `WssSecurityProcessor`, tests.
- `docs`: `ws-soap.md`, ADR-0088, `CONTEXT-MAP.md`.

## Architecture boundaries

- Data/control plane: the window is trusted operator configuration (ADR-0032), validated at startup and fail-closed (ADR-0033). No proto change, no message-level override.
- Components: camel-cxf config grows two scalar knobs following the `fault_status` and `allowed_*` precedents. Rust only transports configuration; the Java bridge owns enforcement.
- Services: `camel-bridge` env emission only.
- Secrets: the values are not secrets; ADR-0051 redaction is not needed for them.
- Related ADRs: ADR-0085 and ADR-0087 (per-profile inbound opt-ins with a startup WARN).

## Alternatives considered

- `NonZeroU64` in serde: rejected; the serde error does not name the profile, and the repo pattern validates in `CxfPoolConfig::validate`.
- Accept every `u64` and clamp silently to `Integer.MAX_VALUE`: rejected by owner ruling; a silently clamped security window hides the effective value from the operator.
- Custom `Timestamp` validator with `long` arithmetic (window beyond 68 years): rejected by owner ruling; it re-implements WSS4J freshness and BSP checks for a range nobody needs (68 years is about 7 times the 10-year fixture need), and R4 names the `RequestData` setters.
- Couple the knobs to the `Timestamp` inbound action: rejected; WSS4J validates any `Timestamp` present in the header, so the window applies with `Signature`-only inbound actions too.
- Extend replay retention to match the TTL: out of scope (custom replay cache).
