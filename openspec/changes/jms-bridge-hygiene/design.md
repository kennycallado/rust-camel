# Design: jms-bridge-hygiene

## Context

`bridges/jms` Java sidecar + `crates/components/camel-jms` Rust component.
Referenced ADRs: ADR-0033 (fail-closed configuration posture — the
unknown-scheme/no-host aborts), ADR-0032 (exchange-data trust boundary),
ADR-0012 (handler-contract boundary for the reject path).
Existing canonical specs: `bridge-transport-security` holds the TLS-scheme
and body-cap requirements this change modifies.

## Fix 1 — fail-loud unknown schemes (rc-2yiq)

`transportConfig(URI, ...)` (`JmsClientFactory.java:212-230`) currently:
host null → `"localhost"`, port ≤ 0 → 61616, scheme ∈ {ssl, wss} → secure
material checks, everything else → insecure params. A `failover:(...)` or
`failover://...` URI therefore builds a plaintext localhost transport.

Change, in order:
1. Extract `scheme = brokerUri.getScheme()` first (already done at `:229` —
   move the read up). Null scheme → `IllegalStateException` (a switch on a
   null String would NPE; the spec demands an actionable error).
2. Switch dispatch to an exhaustive three-way: `"tcp"`/`"ws"` → insecure
   params (unchanged defaults); `"ssl"`/`"wss"` → existing material checks;
   **default → throw** `IllegalStateException("Unsupported broker URL
   scheme '<scheme>' (URL: <uri>): unwrap failover:/fanout: wrappers to a
   single primary broker URL; configure HA broker-side or as multiple
   broker entries")`.
3. Host fallback REMOVED (checked AFTER the scheme dispatch — opaque URIs
   like `failover:(...)` have null hosts and must surface the scheme
   error): a null or blank host now throws the same
   `IllegalStateException` family ("broker URL '<uri>' has no host; a
   complete URL is required — no default host is assumed"). Port fallback
   to 61616 stays (port-less URLs are established Artemis syntax).

Rust complement (broker-type-aware): `crates/components/camel-jms/src/config.rs:469`
validation — a `failover://`-prefixed `broker_url` is REJECTED when the
entry's broker type is `artemis` (error names the URL and the remediation:
single primary URL or multiple broker entries) and ACCEPTED for `activemq`
(Classic path supports it natively via `ActiveMQConnectionFactory`,
`JmsClientFactory.java:147-180`). Tests: `accepts_known_broker_url_schemes`
keeps the failover case ONLY under an `activemq` entry; add
`rejects_failover_scheme_for_artemis_with_migration_hint` and
`accepts_failover_scheme_for_classic`. Update the schemes row in
`docs/src/components/jms.md` with the type-aware rule.

## Fix 2 — byte-accurate TextMessage cap (rc-5r45)

`JmsConsumer.convertMessage` TextMessage branch (`JmsConsumer.java:251-271`)
currently gates `text.length()` (UTF-16 units) vs byte cap, then encodes.
Replace with materialize-once:

```java
String text = tm.getText();
ByteString body = ByteString.copyFromUtf8(text != null ? text : "");
long cap = resolveMaxBodyBytes();
if (body.size() > cap) { /* same diagnostic, byte count, warn, throw */ }
b.setBody(body);
```

Zero extra encodings (the ByteString was already being built); the gate
becomes exact. Diagnostic wording changes "N chars" → "N bytes" (it now
reports `body.size()`). README `JMS_MAX_BODY_BYTES` paragraph states the
full semantics: the TextMessage text is materialized and UTF-8 encoded
BEFORE enforcement, so the cap bounds the FORWARDED body size, not the
peak sidecar allocation (a transient ~2x-3x allocation of oversized text
precedes rejection); the ordering constraint (cap ≤ 19 MiB < Rust decode
20 MiB) then holds universally again.

## Fix 3 — exactly-once destroy (rc-lupv)

Both teardown paths gate `destroy` on winning the owner-checked removal,
and shutdown closes the subscribe-races-shutdown window with an admission
flag + drain loop:
- `cleanupSubscription` (`:163-172`): keep CAS; BOTH `consumer.stop()` and
  the destroy run only `if (activeConsumers.remove(subId, consumer))` —
  symmetric with the drain, and a late CAS winner performs no second stop
  or destroy (times(1) test assertions hold).
- `shutdown()` (`:175-182`): set a `shutdown` AtomicBoolean FIRST; then
  drain: `while (!activeConsumers.isEmpty())` iterate `entrySet()`, for
  each entry `if (activeConsumers.remove(entry.getKey(),
  entry.getValue()))` then `stop()` + `destroy()`. NO final `clear()`.
- `subscribe()` admission: the critical section `synchronized
  (shutdownLock)` contains ONLY the flag check and the `putIfAbsent`
  registration — no CDI destroy, no gRPC response inside the lock. On
  refusal (flag set): release the lock FIRST, then `consumerFactory.
  destroy(consumer)` and `onError(Status.UNAVAILABLE...)`. `shutdown()`
  sets the flag inside the SAME lock, then drains outside it. A short map
  mutation under `@Blocking` is acceptable; destruction or JMS cleanup
  under the shared monitor is not.

Termination argument (linearization): the lock totally orders
registration-vs-flag — if a subscribe's critical section completes before
shutdown's flag-set, its entry is already in the map when the drain loop
first checks emptiness (drain catches it); if it completes after, it
observes the flag and refuses. No check-then-act gap remains. After the
flag, no new registrations succeed; the drain loop removes + destroys
whatever remains and exits.
`remove(k,v)` atomic ⇒ exactly one destroyer per entry across ALL
interleavings, including post-shutdown late CAS winners (their remove
fails — the entry was removed by the shutdown drain).

Race tests (executable contract for the plan): Mockito, deterministic
ordering, same harness as `JmsBridgeServiceTest` (mocked
`Instance<JmsConsumer>`, captured inner observers, reflection map reader):
- `lateCleanupAfterDrainDoesNotDoubleDestroy`: subscribe s1 (consumer A,
  captured inner observer); run `shutdown()`; THEN drive A's inner
  `onError` (late CAS winner — its `finished` CAS succeeds). Assert
  `verify(instance, times(1)).destroy(A)` (drain's destroy only),
  `verify(A, times(1)).stop()`, map empty.
- `drainCatchesSubscriberRegisteredBeforeFlag`: subscribe s1 (A) normally;
  call `shutdown()`. Assert `verify(instance, times(1)).destroy(A)` and
  map empty (drain linearization).
- `subscribeRefusedAfterFlagDestroysOwnConsumer`: `shutdown()` on an empty
  map (flag set); new `subscribe` (consumer B from `instance.get()`).
  Assert observer receives UNAVAILABLE error,
  `verify(instance, times(1)).destroy(B)`,
  `verify(B, never()).subscribe(any(), any(), any(), any())`, map empty.
- `racingRegistrationObservedAfterFlagRefuses`: start a subscribe in an
  executor; park it inside a `doAnswer` latch on `instance.get()` ( BEFORE
  the critical section); run `shutdown()` to completion; release the
  latch. Assert the subscribe refuses + self-destroys
  (`verify(instance).destroy(B)`, UNAVAILABLE error, map empty). The
  registration-before-flag order is covered by
  `drainCatchesSubscriberRegisteredBeforeFlag`; no third order exists.

## Affected boundaries

Components (camel-jms): config allowlist + tests only. Bridges (jms):
factory dispatch, consumer gate, service teardown. No proto, no Runtime/DSL.
Spec deltas: MODIFIED ×2 + ADDED ×1, all in `bridge-transport-security`.

## Phases

Single phase — three independent S-size fixes, one bridge, one goal
(hygiene for 0.6.1). Exit: all three bd criteria green, full battery green.
