# Tasks: jms-bridge-hygiene

## Phase 1: hygiene

### Task 1: Artemis transport config fails loud on unknown schemes and hostless URIs

- **ID:** jms-scheme-java
- **Files:**
  - `bridges/jms/src/main/java/org/rustcamel/jms/JmsClientFactory.java` (modified)
  - `bridges/jms/src/test/java/org/rustcamel/jms/JmsClientFactorySchemeTest.java` (new)
  - `bridges/jms/README.md` (modified — drop `nio://` from the plaintext schemes list, ~line 38)
- **Steps:**
  1. In `transportConfig(URI, String, String, String, String)` (`JmsClientFactory.java:206-245`): hoist `String scheme = brokerUri.getScheme();` to the top of the method body.
  2. Order of checks (matters — opaque URIs like `failover:(...)` carry a null host; the scheme error must win): FIRST `if (scheme == null) throw new IllegalStateException("Broker URL '" + brokerUri + "' has no scheme; a complete URL is required");` THEN the exhaustive dispatch `switch (scheme)` assigning a `boolean secure`: case `"tcp"`, `"ws"` → `secure = false`; case `"ssl"`, `"wss"` → `secure = true`; `default`: throw `IllegalStateException("Unsupported broker URL scheme '" + scheme + "' (URL: " + brokerUri + "): unwrap failover:/fanout: wrappers to a single primary broker URL; configure HA broker-side or as multiple broker entries")` (a null-scheme switch would NPE).
  3. IMMEDIATELY after the switch (before parameter construction, before TLS-material checks, before any plaintext return): if `brokerUri.getHost() == null || brokerUri.getHost().isBlank()`, throw `IllegalStateException("Broker URL '" + brokerUri + "' has no host; a complete URL is required — no default host is assumed")`. Port fallback to 61616 stays (port-less URLs are established Artemis syntax). The insecure/secure flows then proceed exactly as today (`secure` gates the material checks).
  4. Remove the now-dead `localhost` host fallback assignment.
  5. Update the method javadoc: drop `nio` from the plaintext list and the "outer `failover:` ... get no SSL properties" sentence; state the exhaustive fail-loud dispatch and the no-default-host rule.
- **Tests** (`JmsClientFactorySchemeTest.java`, plain JUnit calling the package-private static seam `transportConfig(...)` with explicit material strings — mirror `JmsClientFactoryTlsTest` conventions):
  - `failoverParenthesizedInnerAborts`
    - setup: `URI.create("failover:(ssl://broker:61617)")` — note `URI.getHost()` is null for this shape, scheme is `failover`.
    - action: `transportConfig(uri, "/k", "/t", "pw", "artemis")`.
    - assert: throws `IllegalStateException` whose message contains `failover` and `multiple broker entries` (the scheme error fires BEFORE the host check — opaque URIs have null host); no params map is returned.
    - command: `./gradlew test --tests 'org.rustcamel.jms.JmsClientFactorySchemeTest'` (cwd `bridges/jms`).
    - expected: RED before steps 2 (current code returns plaintext params silently).
  - `failoverPrefixedUriAborts`
    - setup: `URI.create("failover://tcp://broker:61616")`.
    - assert: same `IllegalStateException` family, message contains `failover`.
    - expected: RED before.
  - `hostlessKnownSchemeAborts`
    - setup: `URI.create("ssl://:61617")` (host empty), valid material strings.
    - assert: throws `IllegalStateException` containing `no host`.
    - expected: RED before step 3 (current code maps to localhost).
  - `missingSchemeAbortsActionably` — `URI.create("//broker:61616")` or an env-style bare `localhost` (parse to a URI with null scheme): assert `IllegalStateException` containing `no scheme`. Expected: RED before (current code treats null scheme as insecure plaintext with a localhost fallback).
  - `tcpSchemeStaysPlaintext` — `URI.create("tcp://broker:61616")` returns params with no SSL properties; PIN (passes before and after).
  - `sslSchemeEnablesSsl` — `URI.create("ssl://broker:61617")` with material returns params with `SSL_ENABLED_PROP_NAME=true`; PIN (unchanged behavior; the TLS test suite also covers this).
  - `wsSchemeStaysPlaintext`, `wssSchemeRequiresMaterial` — `ws://broker:61618` returns insecure params (PIN); `wss://broker:61619` without truststore throws the material `IllegalStateException` (PIN — material checks unchanged).
- **Acceptance:**
  - `./gradlew test --tests 'org.rustcamel.jms.JmsClientFactorySchemeTest'` exits 0.
  - `rg -n '"localhost"' bridges/jms/src/main/java/org/rustcamel/jms/JmsClientFactory.java` returns no hits (fallback dead code removed).
  - `rg -n "nio://" bridges/jms/README.md` returns no hits (doc no longer advertises a rejected scheme).
  - `./gradlew test` full suite green (TLS suite regression).
- `[x] jms-scheme-java`

### Task 2: Rust config validation is broker-type-aware for failover URLs

- **ID:** jms-scheme-rust
- **Files:**
  - `crates/components/camel-jms/src/config.rs` (modified)
  - `docs/src/components/jms.md` (modified)
- **Steps:**
  1. In `JmsPoolConfig::validate` (`config.rs:461-490` region): after the existing `known_schemes` prefix check, add: if `bc.broker_url.starts_with("failover://")` AND `bc.broker_type == BrokerType::Artemis` (per-entry field, `config.rs:389`), return `Err(CamelError::ProcessorError(format!("broker '{}' uses failover:// with the artemis broker type (URL '{}'): the Artemis sidecar does not unwrap failover wrappers — use a single primary broker URL or multiple broker entries", name, bc.broker_url)))`.
  2. Test updates in `config.rs` test module: `accepts_known_broker_url_schemes` keeps the `failover://` case ONLY under `BrokerType::ActiveMq`. Add:
     - `rejects_failover_scheme_for_artemis_with_migration_hint`: `single_broker("failover://tcp://localhost:61616", BrokerType::Artemis)` → `validate()` errs; error string contains `failover` and `broker entries`.
     - `accepts_failover_scheme_for_classic`: `single_broker("failover://tcp://localhost:61616", BrokerType::ActiveMq)` → `validate()` ok.
     - expected: `rejects_...` RED before step 1 (currently accepted); `accepts_...` PIN.
  3. `docs/src/components/jms.md`: there is NO existing scheme-allowlist row — ADD the type-aware sentence to the "Broker configuration" section (~:94-105): `failover://` URLs are accepted for Classic (`activemq`) brokers and rejected for `artemis` (use a single primary URL or multiple broker entries).
- **Acceptance:**
  - `cargo test -p camel-component-jms --lib` exits 0.
  - `cargo fmt --check` and `cargo clippy -p camel-component-jms --all-targets -- -D warnings` exit 0.
  - `rg -n "failover" docs/src/components/jms.md` shows the type-aware rule.
- `[x] jms-scheme-rust`

### Task 3: TextMessage cap counts UTF-8 bytes and README states the semantics

- **ID:** jms-cap-java
- **Files:**
  - `bridges/jms/src/main/java/org/rustcamel/jms/JmsConsumer.java` (modified)
  - `bridges/jms/src/test/java/org/rustcamel/jms/JmsConsumerBodyCapTest.java` (modified)
  - `bridges/jms/README.md` (modified)
- **Steps:**
  1. In `convertMessage` TextMessage branch (`JmsConsumer.java:251-271`): replace the sequence `String text = tm.getText(); int len = ...; if (len > cap) {...}` with: `String text = tm.getText(); com.google.protobuf.ByteString body = ByteString.copyFromUtf8(text != null ? text : ""); long size = body.size(); long cap = resolveMaxBodyBytes(); if (size > cap) { ... }` — the diagnostic reports `size` and the unit word `bytes` (was `chars`); keep the warn + `JMSException` throw shape and the ADR-0012 comment; `b.setBody(body)` reuses the materialized bytes (single encode).
  2. `JmsConsumerBodyCapTest`: update any TextMessage over-cap test's diagnostic assertion from `chars` to `bytes` (read the test first; adjust the exact expected-substring). Add:
     - `textMessageUtf8OverCapRejected` — `TextMessage` mock whose `getText()` returns EXACTLY `TEST_CAP_BYTES` CJK chars: `"\u4e2d".repeat(Math.toIntExact(TEST_CAP_BYTES))` (`TEST_CAP_BYTES` is `long`; `repeat` takes `int`) (TEST_CAP_BYTES is the pinned cap, 1024 — `JmsConsumerBodyCapTest.java:44`). UTF-16 length 1024 ≤ cap so the OLD gate forwards it (honest RED: assert-JMSException fails pre-change); UTF-8 size 3072 > cap so the NEW gate rejects. Assert `JMSException` message contains `3072` and `bytes`.
     - expected: RED before step 1 (old gate forwards the text — no exception).
     - `textMessageAsciiAtExactlyCapPasses` — ASCII text of exactly `cap` chars: assert forwarded body size equals cap. PIN (passes before and after; guards the boundary).
  3. `bridges/jms/README.md` `JMS_MAX_BODY_BYTES` paragraph (~line 20): replace "TextMessage bodies are checked against the materialized text length" with byte-accurate semantics: the text is materialized and UTF-8 encoded BEFORE enforcement, the cap bounds the FORWARDED body size, not the peak sidecar allocation (a transient ~2-3x allocation of oversized text precedes rejection); keep the ceiling/ordering-constraint sentences.
- **Acceptance:**
  - `./gradlew test` full suite green (BodyCap + ContentType + MessageTypes + Scheme + TLS + BridgeService).
  - `rg -n "chars \(cap" bridges/jms/src/main/java/org/rustcamel/jms/JmsConsumer.java` returns no hits (unit wording updated).
  - `rg -n "peak sidecar allocation" bridges/jms/README.md` returns 1 hit.
- `[x] jms-cap-java`

### Task 4: Consumer teardown destroys exactly once across shutdown races

- **ID:** jms-teardown-java
- **Files:**
  - `bridges/jms/src/main/java/org/rustcamel/jms/JmsBridgeService.java` (modified)
  - `bridges/jms/src/test/java/org/rustcamel/jms/JmsBridgeServiceTest.java` (modified)
- **Steps:**
  1. Add fields: `private final Object shutdownLock = new Object();` and `private volatile boolean shutdown = false;` (replace or complement per existing style).
  2. `subscribe()` (`:59-62` region): move the `putIfAbsent` registration inside `synchronized (shutdownLock)` together with the flag check: `synchronized (shutdownLock) { if (shutdown) { refused = true; } else { existing = activeConsumers.putIfAbsent(subId, consumer); } }` — ONLY the check + map mutation inside the lock. After the lock: if refused, `consumerFactory.destroy(consumer)` then `responseObserver.onError(Status.UNAVAILABLE.withDescription("bridge shutting down").asException()); return;`. The existing duplicate-rejection (`existing != null`) path stays as-is (outside or inside the lock — keep it outside; putIfAbsent already ran).
  3. `cleanupSubscription` (`:163-173`): gate BOTH stop and destroy inside the owner-check — `if (activeConsumers.remove(subId, consumer)) { consumer.stop(); consumerFactory.destroy(consumer); } return true;` (symmetric with the drain; the late-CAS-winner path then performs NO second stop — `times(1)).stop()` assertions hold).
  4. `shutdown()` (`:175-182`): `synchronized (shutdownLock) { shutdown = true; }` then drain: `while (!activeConsumers.isEmpty()) { for (var e : activeConsumers.entrySet()) { if (activeConsumers.remove(e.getKey(), e.getValue())) { e.getValue().stop(); consumerFactory.destroy(e.getValue()); } } }` — NO final `clear()`.
- **Tests** (`JmsBridgeServiceTest.java`, extend the existing harness: mocked `Instance<JmsConsumer>`, captured inner observers, reflection map reader):
  - `lateCleanupAfterDrainDoesNotDoubleDestroy`
    - setup: subscribe s1 (consumer A, captured inner observer); run `service.shutdown()`.
    - action: drive A's inner observer `onError(...)` (late CAS winner).
    - assert: `verify(instance, times(1)).destroy(A)`; `verify(A, times(1)).stop()`; map empty.
    - expected: RED before steps 3-4 (current code double-destroys: shutdown + late cleanup).
  - `drainCatchesSubscriberRegisteredBeforeFlag`
    - setup: subscribe s1 (A); call `service.shutdown()`.
    - assert: `verify(instance, times(1)).destroy(A)`; map empty. PIN-with-new-semantics (passes before and after, different mechanism).
  - `subscribeRefusedAfterFlagDestroysOwnConsumer`
    - setup: `service.shutdown()` on empty map; new `subscribe(req(s2), obsB)` with consumer B.
    - assert: obsB receives error with `Status.fromThrowable(...).getCode() == Status.Code.UNAVAILABLE`; `verify(instance, times(1)).destroy(B)`; `verify(B, never()).subscribe(any(), any(), any(), any())`; map empty.
    - expected: RED before step 2 (current code registers normally).
  - `racingRegistrationObservedAfterFlagRefuses`
    - setup: stub `instance.get()` with a `doAnswer` that awaits a `CountDownLatch` before returning consumer B; start `service.subscribe(req(s2), obsB)` in an executor; await the stub's entry signal; run `service.shutdown()` to completion; count down the latch; await the executor task.
    - assert: same as `subscribeRefusedAfterFlagDestroysOwnConsumer` (UNAVAILABLE, self-destroy, never subscribed, map empty).
    - expected: RED before step 2.
  - `shutdownAndCleanupRaceDestroysExactlyOnce`
    - setup: subscribe s1 (consumer A, captured inner observer); a `CyclicBarrier(2)`; thread T1 will call `service.shutdown()`, thread T2 will call A's inner `onError(...)`.
    - action: both threads `await()` the barrier then run their calls; join both.
    - assert: `verify(A, times(1)).stop()`; `verify(instance, times(1)).destroy(A)`; map empty — whichever side wins the owner-checked removal, exactly one stop+destroy.
    - expected: PIN-grade with new semantics (the barrier makes the race real; the owner-check makes the outcome deterministic in aggregate even though the winner is scheduler-dependent).
  - Existing four tests (`duplicateSubscriptionIdRejectedAlreadyExists`, `cancelledFirstStreamDoesNotEvictSecondOwner`, `differentlyKeyedCancellationLeavesOtherIntact`, `rejectedStreamRegistersNoCleanup`) must stay green — regression. ADD to `differentlyKeyedCancellationLeavesOtherIntact` (it owns the spec's normal-completion-destroys-once scenario): `verify(consumerA).stop(); verify(instance).destroy(consumerA);` after completing s1.
- **Acceptance:**
  - `./gradlew test --tests 'org.rustcamel.jms.JmsBridgeServiceTest'` exits 0 (9 tests).
  - `rg -n "clear\(\)" bridges/jms/src/main/java/org/rustcamel/jms/JmsBridgeService.java` returns no hits in shutdown (map clear removed).
  - `./gradlew test` full suite green.
- `[x] jms-teardown-java`
