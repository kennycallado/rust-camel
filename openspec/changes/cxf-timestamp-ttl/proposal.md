# Proposal: cxf-timestamp-ttl

## Why

Partner camel-112 signs its test fixtures with a WS-Security `Timestamp` whose `Expires` is years in the future, and replays those fixtures long after `Created`. Some legacy peers also have clocks that drift ahead of ours by more than one minute. The consumer bridge uses the fixed WSS4J freshness window: `Created` must be at most 300 s old and at most 60 s in the future. Both partner shapes fail with `The message timestamp is out of range`, and no setting changes the window.

The window must become per-profile operator configuration. The current behavior must stay the default.

Bd: rc-a6dyc.

## What Changes

- Two new per-profile settings in `[profiles.security]`: `timestamp_ttl_seconds` (default 300) and `timestamp_future_tolerance_seconds` (default 60). They flow `CxfSecurityFields` → `CXF_PROFILE_<NAME>_TIMESTAMP_TTL_SECONDS` / `CXF_PROFILE_<NAME>_TIMESTAMP_FUTURE_TOLERANCE_SECONDS` → `SecurityProfileStore` → `SecurityProfile.Builder`.
- Values are integers from 1 to 2147483647 s (about 68 years, the WSS4J `int` limit). Larger values are rejected at startup, never clamped. Zero is rejected with a typed config error (`CamelError::Config` in Rust; a startup error that names the variable in Java). A set value on a profile without verification material is rejected, because it would be inert.
- The bridge logs one startup WARN that names the profile when either value is above its default.
- The consumer path (`WssSecurityProcessor.processInbound`) sets `RequestData.setTimeStampTTL` and `RequestData.setTimeStampFutureTTL` from the profile. The producer `ProfileInInterceptor` path is unchanged.
- Docs: `docs/src/components/ws-soap.md` and new ADR-0088, which records the replay-window trade-off and the verified replay-cache facts.

Excluded: a custom replay cache, key rotation, per-message header overrides, and any change to the replay cache or the Body+Timestamp signature coverage.

Affected crates: `crates/components/camel-cxf`, `crates/services/camel-bridge`, `crates/camel-test` (struct literals only), `bridges/cxf` (Java), `docs`.

## Acceptance criteria

- Rust, for both keys: TOML parse; defaults unset (bridge defaults 300/60 apply); zero rejected by `CxfPoolConfig::validate`; `2147483647` accepted; `2147483648` and `u64::MAX` rejected naming the key and `2147483647`; env vars emitted only when set.
- Java, for both variables: env parse; unset gives 300/60; `0`, negative, and non-integer rejected naming the variable; `2147483647` accepted; `2147483648` and `18446744073709551615` rejected naming the variable and `2147483647`; WARN emitted (captured from the store log) when either value is above default and absent at or below default.
- Java consumer, each comparison on fresh processors: a signed message whose `Created` is 600 s old is rejected with the default and accepted with `timestamp_ttl_seconds = 900`; a `Created` 120 s in the future is rejected with the default and accepted with `timestamp_future_tolerance_seconds = 300`; a fixture-style 9-year-old `Created` with a 10-year `Expires` is accepted with a 10-year TTL; an expired `Expires` is still rejected with a raised TTL; a byte-identical resend is still rejected as a replay.
- Producer in-interceptor properties carry no timestamp TTL keys.
- Gates: `cargo fmt`, clippy `-D warnings`, `camel-component-cxf` and `camel-bridge` tests, `feature_profiles`, xtask lints (including `lint-bridge-versions`), schema check, `mdbook test`, `./gradlew spotlessCheck test`, `openspec validate`.
- No bridge version bump, no pin edit, no release.

## Risk budget

Accepted risk (ADR-0088): a larger TTL widens the acceptance window for a captured, validly signed message. Replay retention does not grow with it. `MemoryReplayCache.add` keeps a timestamp entry until `Expires` only when `Expires` is in the future and at most `MAX_TTL` = 3600 s away; otherwise (no `Expires`, a past `Expires`, or an `Expires` more than one hour away) it keeps the entry for `DEFAULT_TTL` = 300 s. A fixture with a multi-year `Expires` is therefore remembered for 300 s only. After its cache entry is gone, a captured message can be replayed while it is still inside the configured window. Operators who raise the TTL accept this for the profile they configure.

Owner ruling (2026-10-21): the WSS4J `RequestData` setters named by R4 take `int` seconds, so values are limited to 2147483647 s and larger values are rejected at startup. No silent clamp, no custom `Timestamp` validator.

Out of bounds: changing the replay cache, removing the Timestamp signature-coverage check, a global or environment-wide window switch, and the producer path.
