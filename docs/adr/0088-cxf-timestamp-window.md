# ADR-0088: Per-profile WS-Security Timestamp freshness window (consumer)

- Status: Accepted (2026-10-21; bd rc-a6dyc)
- Source: bd rc-a6dyc (P2 partner interop); openspec change `cxf-timestamp-ttl`
- Cross-refs: ADR-0032 (exchange-data trust boundary; operator configuration is trusted), ADR-0033 (fail-closed
  security defaults), ADR-0085 (legacy SHA-1 inbound opt-in), ADR-0087 (expired
  pinned certificate opt-in)

## Context

Partner camel-112 signs test fixtures with a WS-Security `Timestamp` whose
`Expires` is years in the future, and sends those fixtures long after
`Created`. Some legacy peers also run clocks that are ahead of ours by more
than one minute. The consumer bridge currently uses the fixed WSS4J freshness
window: `Created` must be at most 300 s old and at most 60 s in the future.
Both partner shapes fail with `The message timestamp is out of range`, and no
setting changes the window.

A raised window keeps the existing signature and replay checks. WSS4J still
rejects a `Timestamp` whose `Expires` has passed, independently of the TTL. The
replay cache still rejects a byte-identical resend while its entry is live. The
change therefore adds one per-profile operator setting for each direction of the
window and documents the replay trade-off. The current behavior stays the
default.

## Decision

**Two per-profile knobs, defaults unchanged.** Add
`timestamp_ttl_seconds` (default 300) and
`timestamp_future_tolerance_seconds` (default 60) to
`[profiles.security]`. `timestamp_ttl_seconds` bounds how old `Created` may
be. `timestamp_future_tolerance_seconds` bounds how far ahead `Created` may be.
An omitted option keeps the WSS4J default. The values flow per profile:

`CxfSecurityFields` (Rust) → `CXF_PROFILE_<NAME>_TIMESTAMP_TTL_SECONDS` /
`CXF_PROFILE_<NAME>_TIMESTAMP_FUTURE_TOLERANCE_SECONDS` → `SecurityProfileStore`
→ `SecurityProfile.Builder` → `RequestData`.

**Validation is fail-closed at both layers.** Rust `CxfSecurityFields::validate_timestamp_window`
rejects `0` with `CamelError::Config` that names the profile and the key. It
also rejects a set value on a profile with neither a keystore nor a truststore,
because the window would be inert without verification material. Accepted values
are 1 to 2147483647 s (inclusive) — about 68 years, the WSS4J `int` limit of
`RequestData.setTimeStampTTL` and `setTimeStampFutureTTL`. Larger values are
rejected at startup: Rust returns `CamelError::Config` naming the field and the
maximum, and the bridge fails startup with an error naming the variable and the
maximum. No value is clamped silently.

The Java store parses each variable with `SecurityProfileStore.parseTimestampSeconds`.
An absent or blank variable returns `null` and keeps the default. Zero,
negative, non-integer input, and values above 2147483647 reject startup with an
error that names the variable. `SecurityProfile.Builder` repeats the zero and
inert-profile checks before the profile is built. Profile loading validates
these values regardless of endpoint role. The producer in-interceptor does not
apply them.

**Startup WARN.** When either effective value is above its default,
`SecurityProfileStore.timestampWindowWarning` returns one message. The message
names the profile, both effective values, and ADR-0088, and states that replay
retention stays at most 3600 s. Values at or below both defaults produce no
WARN. The store logs the message after the existing profile warnings.

**Consumer only.** `WssSecurityProcessor.processInbound` sets both values on
its `RequestData`. The producer `ProfileInInterceptor` path is unchanged and
carries no timestamp TTL property. WSS4J validates every `Timestamp` present in
the inbound header, so the window also applies to profiles whose inbound
actions are `Signature` only.

**Replay retention is unchanged.** Each profile has one shared
`MemoryReplayCache`, used for both timestamp and nonce entries. For a signed
message whose signature follows a `Timestamp`, the replay key is the `Created`
string plus a hash of the signature value bytes plus a hash of the verification
key encoding. It does not use `wsu:Id`. A byte-identical resend is rejected
with `A replay attack has been detected` while the entry is live. An entry is
retained until `Expires` when `Expires` is in the future and at most 3600 s
away; otherwise it is retained for 300 s. The 300 s case covers no `Expires`, a
past `Expires`, and an `Expires` more than 3600 s away. A fixture with a
multi-year `Expires` is therefore remembered for 300 s. A larger TTL widens
acceptance, never retention.

## Accepted risks

1. **A captured signed message can be replayed after its cache entry expires.**
   While the message remains inside the configured window, the replay cache no
   longer holds its key. The default window keeps this exposure to the existing
   300 s / 60 s bounds. An operator who raises the window accepts a longer
   replay window for that profile.
2. **Retention does not follow the window.** Extending replay retention to the
   configured TTL would need a custom replay cache. This change does not add
   one, so a multi-year window and a multi-year `Expires` still give 300 s of
   replay protection.
3. **A raised TTL does not accept an expired `Expires`.** WSS4J rejects it in
   strict mode regardless of the TTL. Operators who need a long-lived fixture
   must issue it with an `Expires` that is still in the future.
4. **The values are trusted operator configuration.** They are not secrets and
   carry no message-level override. A per-message header cannot widen the
   window.

## Rejected alternatives

- **Custom `Timestamp` validator with `long` arithmetic.** Rejected: it
  re-implements WSS4J freshness and BSP checks for a range nobody needs, and
  the `RequestData` setters already express about 68 years.
- **Clamp values above 2147483647 s silently.** Rejected: a silently clamped
  security window hides the effective value from the operator and would fail
  review.
- **Extend replay retention to match the TTL.** Out of scope: it needs a custom
  replay cache, with its own storage and eviction rules.
- **Couple the knobs to the `Timestamp` inbound action.** Rejected: WSS4J
  validates any `Timestamp` present in the header, so the window must apply
  with `Signature`-only inbound actions too.
- **A global or environment-wide window switch.** Rejected: it would widen
  every profile to serve one partner and would violate ADR-0033.
- **`NonZeroU64` in the serde type.** Rejected: the serde error does not name
  the profile, and the repository pattern validates in
  `CxfPoolConfig::validate`.

## Consequences

The default behavior is unchanged. Profiles that do not set the options keep
the WSS4J window. A profile that raises the window accepts older or more
future-dated `Created` values and logs one startup WARN. Replay retention,
signature coverage, the required-actions check, and the producer path are
unchanged.

The options and the replay facts are documented in
`docs/src/components/ws-soap.md`. The bridge README lists the two environment
variables. `crates/components/camel-cxf/CONTEXT.md` and `CONTEXT-MAP.md` record
the settings. No generated schema covers `CxfSecurityFields`, so
`cargo xtask schema --check` is unaffected. No bridge release is made for this
change.

## References

- bd rc-a6dyc (this decision)
- openspec change `cxf-timestamp-ttl`
- ADR-0032 (exchange-data trust boundary; operator configuration is trusted)
- ADR-0033 (security defaults and fail-closed startup validation)
- ADR-0085 (legacy SHA-1 inbound opt-in)
- ADR-0087 (expired pinned certificate opt-in)
- `bridges/cxf/src/main/java/org/rustcamel/cxf/SecurityProfile.java`
  (builder validation and effective accessors)
- `bridges/cxf/src/main/java/org/rustcamel/cxf/SecurityProfileStore.java`
  (environment parsing and startup WARN)
- `bridges/cxf/src/main/java/org/rustcamel/cxf/WssSecurityProcessor.java`
  (consumer `RequestData` wiring)
- `crates/components/camel-cxf/src/config.rs` (`CxfSecurityFields`)
- `crates/services/camel-bridge/src/process.rs` (environment emission)
