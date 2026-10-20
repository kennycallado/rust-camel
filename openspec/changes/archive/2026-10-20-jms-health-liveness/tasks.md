# Tasks: jms-health-liveness

## Scope check (STAGE 2)

- Phase 1 is accepted: one Java subsystem (`bridges/jms`) with a single deliverable
  (broker-independent health RPC) and six tasks.
- Phase 2 is accepted: one Rust/policy/schema/docs subsystem with a single deliverable
  (configurable liveness policy plus the published contract) and eight tasks.
- Both phases match `design.md` § Phases. Task count per phase stays at or below eight.

## Execution conventions (read before any task)

Worktree: `/home/shared/rust-camel-worktrees/371-jms-health-liveness`.
Never run `cargo build`, `cargo test`, `cargo clippy`, or `cargo check` in
`/home/kenny/dev/rust-camel`.

Every Cargo command below goes through this wrapper. The first argument is a
short literal slug; use the slug named in the task command. A hygiene failure
must never be swallowed and the command status must be preserved.

```bash
rc_cargo() {
  local slug="$1"; shift
  local unit="rc371-${slug}-$$"
  if [ "$(pgrep -c compiled_artifa || true)" != "0" ]; then
    printf 'rc_cargo: refusing %s: pre-existing compiled_artifa process\n' "$unit" >&2
    return 1
  fi
  systemd-run --user --scope --collect --unit="$unit" \
    -p MemoryMax=12G -p MemorySwapMax=2G -p TasksMax=1024 -p OOMPolicy=kill \
    -- env TMPDIR=/home/shared/tmp CARGO_BUILD_JOBS=6 cargo "$@"
  local status=$?
  systemctl --user stop "$unit" 2>/dev/null || true
  if [ "$(pgrep -c compiled_artifa || true)" != "0" ]; then
    printf 'rc_cargo: hygiene failure after %s: compiled_artifa remains\n' "$unit" >&2
    return 1
  fi
  return "$status"
}
```

Every Java command runs from
`/home/shared/rust-camel-worktrees/371-jms-health-liveness/bridges/jms`. Each
task names its JUnit class explicitly; for example:

```bash
nix shell nixpkgs#jdk21_headless -c ./gradlew --no-daemon test --tests org.rustcamel.jms.JmsBridgeServiceTest
nix shell nixpkgs#jdk21_headless -c ./gradlew --no-daemon spotlessCheck
```

Do not build native images, bump `BRIDGE_VERSION` or Gradle versions, merge,
archive, close bd issues, or push. Cargo manifest edits require the conditional
`feature_profiles` gate in the final gate list. No full-workspace integration or
Docker suite is run by this change.

### Final gates (run once after both phases; evidence in the handoff)

Read every command from the authoritative root `AGENTS.md` `## QUALITY GATES`
block at execution time. Run each gate separately and record exit status; do
not use a copied registry. Skip only `lint-commits` because it fetches remotely.
Scope `RUSTDOCFLAGS` to doc-build only. Run `mdbook test` with tool `workdir`
set to the worktree's `docs` directory, never by changing shell CWD. Add these
conductor commands, with workspace build before all tests:

```bash
rc_cargo gate-build build --workspace
rc_cargo gate-test-lib test --workspace --lib
rc_cargo gate-hex-test test -p camel-core --test hexagonal_architecture_boundaries_test
```

When `Cargo.toml` or `Cargo.lock` changed, additionally run
`rc_cargo gate-feature-profiles test -p camel-cli --test feature_profiles`.
Never regenerate the golden file to make that test pass.

Because `docs/**` changes, also run from `docs/`: `mdbook test`.
Run the full Java suite and `spotlessCheck`:

```bash
nix shell nixpkgs#jdk21_headless -c ./gradlew --no-daemon test
nix shell nixpkgs#jdk21_headless -c ./gradlew --no-daemon spotlessCheck
```

Record the commit range for `cargo xtask changelog --check` in the handoff for
the remote lint-commits gate; do not invent commit subjects and do not run the
gate locally.

---

## Phase 1: Broker-independent Java liveness

### bridges/jms

#### Task 1.1: Parse and validate the broker-probe mode

**Files:**
- `bridges/jms/src/main/java/org/rustcamel/jms/BridgeConfig.java` (modified)
- `bridges/jms/src/test/java/org/rustcamel/jms/BridgeConfigHealthProbeModeTest.java` (new)

**Steps:**
1. Add `static final String HEALTH_BROKER_PROBE_MODE_ENV = "JMS_HEALTH_BROKER_PROBE_MODE"` to `BridgeConfig`.
2. Add nested `public enum BrokerProbeMode { REUSE, SKIP }` with `static BrokerProbeMode from(String raw)` that trims, lower-cases with `Locale.ROOT`, maps `reuse` to `REUSE`, `skip` to `SKIP`, and throws `JournalPermanentException` naming only the variable for every other value.
3. Add `public BrokerProbeMode brokerProbeMode() throws JournalPermanentException` returning `BrokerProbeMode.REUSE` when the variable is unset or blank, otherwise `BrokerProbeMode.from(raw)`.
4. Add package-private `void validateStartup()` that calls `brokerProbeMode()` and wraps invalid mode in `IllegalStateException` naming only the variable. Add `void onStartup(@Observes StartupEvent event)` calling it, following `OtlpEgressGuard`. Import Quarkus `StartupEvent` and Jakarta `Observes`; do not rely on lazy `@ApplicationScoped` bean `@PostConstruct` to validate before the first RPC. The startup observer eagerly enforces the mode contract.
5. Extend the class Javadoc with the new variable, default `reuse`, and the allowed values.

**Tests:**
- `defaults_to_reuse`: setup `new BridgeConfig(Map.of())` → call `brokerProbeMode()` → assert `BrokerProbeMode.REUSE`. Command: `nix shell nixpkgs#jdk21_headless -c ./gradlew --no-daemon test --tests org.rustcamel.jms.BridgeConfigHealthProbeModeTest`. Expected: RED before Step 2 because the enum and method do not exist (compile failure); GREEN after Step 2.
- `parses_skip`: setup `new BridgeConfig(Map.of(HEALTH_BROKER_PROBE_MODE_ENV, "skip"))` → call `brokerProbeMode()` → assert `BrokerProbeMode.SKIP`. Command as above. Expected: RED before Step 2; GREEN after Step 2.
- `rejects_unknown_mode_without_echoing_raw_value`: setup env value `probe-v2` → assert `assertThrows(JournalPermanentException.class, config::brokerProbeMode)` → assert the message contains `JMS_HEALTH_BROKER_PROBE_MODE` and does not contain `probe-v2`. Command as above. Expected: RED before Step 2; GREEN after Step 2.
- `startup_validation_rejects_unknown_mode`: setup env value `later` → call `config.validateStartup()` → assert `IllegalStateException` is thrown with a message containing `JMS_HEALTH_BROKER_PROBE_MODE` before any health RPC. Command as above. Expected: RED before Step 4 because `validateStartup` is absent; GREEN after Step 4.
- `startup_observer_validates_before_rpc`: construct config with invalid mode and no RPC invocation → call `onStartup(new StartupEvent())`, assert `IllegalStateException` naming the variable; reflect the observer parameter and assert Jakarta `@Observes` is present so Quarkus eagerly invokes it. Command as above. Expected: RED without the observer; GREEN after Step 4.

**Acceptance:**
- The targeted JUnit class passes under JDK 21.
- `nix shell nixpkgs#jdk21_headless -c ./gradlew --no-daemon spotlessCheck` exits 0.
- `rg -n "JMS_HEALTH_BROKER_PROBE_MODE" bridges/jms/src/main/java/org/rustcamel/jms/BridgeConfig.java` returns the constant.

- [x] 1.1

#### Task 1.2: Retain one provider-specific diagnostic resource per probe generation

**Files:**
- `bridges/jms/src/main/java/org/rustcamel/jms/BrokerProbeResult.java` (new)
- `bridges/jms/src/main/java/org/rustcamel/jms/BrokerProbe.java` (new)
- `bridges/jms/src/main/java/org/rustcamel/jms/JmsBrokerProbe.java` (new)
- `bridges/jms/src/main/java/org/rustcamel/jms/JmsClientFactory.java` (modified)
- `bridges/jms/src/test/java/org/rustcamel/jms/JmsBrokerProbeTest.java` (new)

**Steps:**
1. Add `public record BrokerProbeResult(boolean connected, String message)` with `static BrokerProbeResult up()` (`true`, `"ok"`) and `static BrokerProbeResult down(String reason)` (`false`, reason).
2. Add `public interface BrokerProbe { BrokerProbeResult probe(long generation) throws Exception; void invalidate(long generation); void close(); }`. Javadoc must state that `invalidate` is non-blocking and must perform no provider I/O on the caller thread.
3. Add `JmsBrokerProbe implements BrokerProbe`. It holds `BridgeConfig config`, a `DiagnosticFactory`, and one `AtomicReference<ProbeState>` where `private record ProbeState(long generation, Diagnostic diagnostic, boolean closed)` is immutable. Initialize `ProbeState` generation to `1L` with a null diagnostic and `closed=false`. It also holds an `AtomicInteger retirementPending` and one owned cleanup `ExecutorService`.
4. Inside `JmsBrokerProbe`, define package-private nested `interface Diagnostic { void verify() throws Exception; void close(); }` and `interface DiagnosticFactory { Diagnostic open(String brokerType, String brokerUrl, String username, String password) throws Exception; }`.
5. Implement `JmsBrokerProbe(BridgeConfig config)` with a real factory and package-private `JmsBrokerProbe(BridgeConfig config, DiagnosticFactory factory)` for tests. Cleanup uses one owned daemon executor named `jms-probe-cleanup`, a bounded queue of two retirement slots, and no per-resource threads or fallback deque. Only the single health worker may open diagnostics, and no open starts while retirement is pending. Atomically detach each diagnostic exactly once; retained ownership and an in-flight open bound outstanding retirements to two. The cleanup executor stays able to accept a late-open retirement after terminal close, then terminates after that open and all retirements finish. A provider that ignores interrupts can retain this one daemon worker, but cannot accumulate threads or reopen resources.
6. Implement `probe(long generation)` as an optimistic CAS protocol with no holds while provider code runs:
   - Read `ProbeState observed`; return `down("probe closed")` when `closed`, `down("probe invalidated")` when `observed.generation() != generation`, and `down("probe cleanup pending")` while `retirementPending` is non-zero. Resource creation is forbidden while a prior retirement is pending.
   - When the retained diagnostic is null, open it outside every lock and outside the CAS. Install it with `compareAndSet(observed, new ProbeState(generation, opened, false))` only when the generation is still current and the state is still not closed. If the CAS fails, submit the newly opened resource to retirement and return `down("probe invalidated")`; the late resource can never be retained or published.
   - When a diagnostic is retained, call `verify()` outside the CAS. On success, re-read the state and return `up()` only when the state is still open, the generation still matches, and the diagnostic is still the retained instance; otherwise retire the diagnostic and return `down("probe invalidated")`.
   - On any exception, retire the diagnostic through the cleanup executor (never close it synchronously on the probe caller), re-assert the interrupt flag for `InterruptedException`, and return `down(e.getClass().getSimpleName() + " during broker probe")` so no URL and no credential reaches the message.
7. Implement monotonic `invalidate(long generation)`: ignore a supplied generation less than or equal to the current one. Otherwise CAS to that generation and null diagnostic, preserving the terminal flag; submit a detached resource once to cleanup. Return without provider I/O. Failed verify and late-open paths use identity-checked detach/retire so they cannot close an already-retired resource twice.
8. Implement `close()`: atomically set terminal, detach and retire the retained diagnostic once, and wait at most `1` second for cleanup. Return even when provider close ignores interrupts; do not drain provider closes synchronously. The late-open completion path still retires its resource and triggers final executor shutdown. A terminal probe never creates another resource.
9. Real ActiveMQ diagnostic: build an `org.apache.activemq.ActiveMQConnectionFactory`, then call package-private `static void clearExplicitClientId(org.apache.activemq.ActiveMQConnectionFactory factory)`, which calls `factory.setClientID(null)` through the provider API. Do not parse, split, or rewrite the URL: a composite `failover:` URL and its `jms.clientIDPrefix` / `jms.connectionIDPrefix` parameters must survive byte-for-byte. If the pinned ActiveMQ client does not expose `setClientID(null)`, stop and report instead of adding a bespoke parser. Then `createConnection()`, `start()`, retain the connection, and verify with `getMetaData()`.
10. Real Artemis diagnostic: keep the existing native-image-safe path exactly: `buildArtemisFactory(url, user, pass)`, `cf.getServerLocator()`, `locator.getStaticTransportConfigurations()[0]`, `locator.createSessionFactory(tc)`. Retain the `ClientSessionFactory`; verify with `createSession().close()`. Close the `ClientSessionFactory` then the Artemis factory. Apply the explicit-client-ID clearing through the Artemis provider setter only when that setter exists in the pinned client; otherwise the session-factory path applies no explicit client ID and the test asserts that fact. Never replace this with a generic `createConnection()` path.
11. Install identity/generation-scoped failure tracking when creating each diagnostic: ActiveMQ JMS `ExceptionListener` plus `TransportListener.transportInterupted`/`onException`, and Artemis session-factory failure listener. Failure callbacks mark only that diagnostic broken; `verify()` checks the flag before and after metadata/session inspection. Do not close or reset consumer resources from callbacks. Ignore callbacks for retired diagnostics; replacement clears only the new diagnostic's flag.
12. Add package-private `JmsBrokerProbe healthProbe()` to `JmsClientFactory` that safely initializes one retained probe without broker I/O. Keep `checkHealth()` as a temporary thin delegate until Task 1.5 migrates its caller. The factory's destroy hook closes the existing probe without lazily creating one. Ordinary probe failure never resets the factory.

**Tests:**
- `startsAtGenerationOne`: construct with a fake factory → call `probe(0L)` then `probe(1L)` → assert the first is disconnected with `probe invalidated` and the second is connected with `ok`, and the fake factory opened once. Command: `nix shell nixpkgs#jdk21_headless -c ./gradlew --no-daemon test --tests org.rustcamel.jms.JmsBrokerProbeTest`. Expected: RED before Step 3 because `JmsBrokerProbe` is absent; GREEN after Steps 3-6.
- `activeMqReusesRetainedDiagnosticAcrossChecks`: setup a fake `DiagnosticFactory` counting opens and a fake `Diagnostic` counting verifies, broker type `activemq` → call `probe(1L)` twice → assert opens `1`, verifies `2`, both results connected. Command as above. Expected: RED before Step 3; GREEN after Steps 3-6.
- `artemisReusesRetainedDiagnosticAcrossChecks`: same fake factory with broker type `artemis` → call `probe(1L)` twice → assert opens `1`, verifies `2`. Command as above. Expected: RED before Step 3; GREEN after Steps 3-6.
- `replacesDiagnosticAfterVerifyFailure`: fake diagnostic throws `IllegalStateException` on the first verify and succeeds afterwards → call `probe(1L)`, await the cleanup latch, then `probe(1L)` → assert the first result is disconnected, opens `2`, and the first diagnostic was closed by the cleanup executor. Command as above. Expected: RED before Step 6; GREEN after Step 6.
- `interruptFlagSurvivesInterruptedProbe`: fake diagnostic throws `InterruptedException` → call `probe(1L)` → assert the result is disconnected, `Thread.currentThread().isInterrupted()` is true, then clear the flag with `Thread.interrupted()`. Command as above. Expected: RED before Step 6; GREEN after Step 6.
- `failureMessageOmitsUrlAndCredentials`: configure URL `tcp://admin:secret@broker:61616` and a fake diagnostic throwing `new IllegalStateException("tcp://admin:secret@broker:61616")` → call `probe(1L)` → assert the result message does not contain `admin`, `secret`, or `broker:61616`. Command as above. Expected: RED before Step 6; GREEN after Step 6.
- `activeMqExplicitClientIdClearedWithoutUrlRewriting`: build a real `org.apache.activemq.ActiveMQConnectionFactory` from `failover:(tcp://a:61616,tcp://b:61616)?jms.clientID=shared&jms.clientIDPrefix=pre&jms.connectionIDPrefix=conn`, call `clearExplicitClientId(factory)` → assert `factory.getClientID()` is null, `factory.getClientIDPrefix()` is `pre`, `factory.getConnectionIDPrefix()` is `conn`, and `factory.getBrokerURL()` still starts with `failover:(` and still contains both `tcp://a:61616` and `tcp://b:61616`. Command as above. Expected: RED before Step 9; GREEN after Step 9.
- `artemisNativePathRetainsFactoryWithoutRewritingUrl`: inject a `DiagnosticFactory` whose `open` delegates to the production Artemis builder and returns a mock `ClientSessionFactory`; call `probe(1L)` → assert the real Artemis `ActiveMQConnectionFactory` type was constructed, `getServerLocator()` produced at least one static transport configuration, the URL passed to the factory still contains `jms.clientIDPrefix=pre`, and no `createConnection()` path ran. Command as above. Expected: RED before Step 10; GREEN after Step 10.
- `artemisExplicitClientIdPolicyIsEnforced`: construct the real Artemis factory from `tcp://h:61616?jms.clientID=shared&jms.clientIDPrefix=pre`; call the production Artemis builder and then inspect the factory → when the pinned client exposes `getClientID()`, assert it is null and `getClientIDPrefix()` is `pre`; when it does not, assert the builder applied no explicit client ID and retained the session-factory path with the unmodified URL. In both branches, assert the retained resource is a `ClientSessionFactory`, not a `Connection`. Command as above. Expected: RED before Step 10; GREEN after Step 10.
- `invalidateReturnsBeforeBlockedProviderClose`: fake diagnostic blocks on a latch in `close()` → call `probe(1L)`, then measure `invalidate(2L)` → assert invalidation returns in under `100` ms and a following `probe(1L)` opens nothing and returns `probe invalidated`; release the latch and await cleanup. Command as above. Expected: RED before Steps 7-8; GREEN after Steps 7-8.
- `lateOpenCannotRetainAfterInvalidate`: fake factory blocks in `open()` on a latch → start `probe(1L)` on a test executor, wait for open entry, call `invalidate(2L)`, release open, await probe → assert the opened diagnostic was retired and closed, the probe result is disconnected, and a following `probe(2L)` opens a fresh diagnostic. Command as above. Expected: RED before Step 6 because the late resource would be retained; GREEN after Step 6.
- `reverseOrderedInvalidationsCannotRegressGeneration`: gate the generation-2 invalidation, complete generation-3 invalidation first, then release generation-2 → assert `probe(3L)` is eligible, `probe(2L)` is invalidated, and detached resources close once. Command as above. Expected: RED with unconditional generation assignment; GREEN after Step 7.
- `providerCallbacksInvalidateOnlyTheirDiagnostic`: construct production ActiveMQ and Artemis adapters with mocked provider resources; capture and fire their failure/interruption callbacks while metadata/session access still succeeds → assert the next verify is disconnected and replacement is admitted only after retirement; invoke the old callback after replacement → assert the new diagnostic remains connected and no consumer/pool reset occurs. Command as above. Expected: RED without Step 11; GREEN with identity-scoped failure tracking.
- `probeExceptionRetiresAndNeverResetsThePool`: fake diagnostic throws, and the fake factory counts opens; after `probe(1L)` await cleanup → assert the diagnostic is closed by the cleanup executor, `JmsClientFactory.reset()` appears nowhere in `JmsBrokerProbe.java`, and a second probe opens exactly one replacement. Command as above. Expected: RED before Steps 6-7; GREEN after Steps 6-7.
- `closeRetiresWithinBoundAndSpansOneCleanupThread`: fake diagnostic close ignores interrupts behind a latch → call `close()` and measure elapsed → assert elapsed is at most `1_500` ms, no new diagnostic can open, and exactly one cleanup worker exists; release the latch in `finally` and assert eventual executor termination and exactly one close of the detached diagnostic. Command as above. Expected: RED before Step 8; GREEN after Step 8.

**Acceptance:**
- Targeted JUnit class passes.
- `spotlessCheck` exits 0.
- `rg -n "getStaticTransportConfigurations|createSessionFactory" bridges/jms/src/main/java/org/rustcamel/jms/JmsBrokerProbe.java` proves the Artemis native path survived.
- `rg -n "clearExplicitClientId" bridges/jms/src/main/java/org/rustcamel/jms/JmsBrokerProbe.java` shows the provider setter helper is used, and `rg -n "split\\(|indexOf\\('\\?'\\)|getQuery" bridges/jms/src/main/java/org/rustcamel/jms/JmsBrokerProbe.java` returns no URL-rewriting parser.
- `rg -n "checkHealth" bridges/jms/src/main/java/org/rustcamel/jms/JmsClientFactory.java` still shows the temporary delegate; Task 1.5 removes it with its caller.

- [x] 1.2

#### Task 1.3: Add the immutable health snapshot and single-flight refresh

**Files:**
- `bridges/jms/src/main/java/org/rustcamel/jms/HealthSnapshot.java` (new)
- `bridges/jms/src/main/java/org/rustcamel/jms/BridgeHealthCache.java` (new)
- `bridges/jms/src/test/java/org/rustcamel/jms/BridgeHealthCacheTest.java` (new)

**Steps:**
1. Add `public record HealthSnapshot(long generation, Status status, boolean healthy, boolean brokerConnected, String message, long checkedAtMillis)` with `public enum Status { UNKNOWN, SKIPPED, STALE, UP, DOWN }`, `HealthResponse toResponse()`, and static factories `unknown(long generation, long now)`, `stale(long generation, long ageMillis, long now)`, `up(long generation, long now)`, `down(long generation, String reason, long now)`, and `skipped(long generation, long now)`.
2. Messages must be exactly: unknown `broker status not yet checked`; skipped `broker probe skipped; connectivity unverified`; stale `broker status stale; age ` followed by the numeric age and ` ms`; up `ok`; down the supplied credential-safe reason. Unknown, skipped, and stale carry `healthy=true`, `brokerConnected=false`; up carries both true; down carries both false. Unknown is an explicit `Status.UNKNOWN`, never inferred from `checkedAtMillis == 0`; a verified `UP` snapshot at clock `0` is valid and connected.
3. Add `BridgeHealthCache` with `HEALTH_TTL_MS = 10_000L` and `REFRESH_AFTER_MS = 5_000L` (proactive refresh starts at half the TTL, not at 9 s, so the refresh cadence cannot flap between callers).
4. Fields: `BrokerProbe probe`, `boolean skip`, `LongSupplier clock`, `Supplier<ExecutorService> executorFactory`, one `ExecutorService worker` created once with daemon thread `jms-health-refresh`, and one `AtomicReference<CacheState> state`. Define immutable `private record CacheState(long generation, HealthSnapshot snapshot, Long refreshStartedAtMillis, boolean closed)`. Initialize generation `1L`, unknown snapshot, null refresh start, and `closed=false`. A boxed nullable timestamp distinguishes no refresh from a refresh started at time zero.
5. Production constructor `BridgeHealthCache(BrokerProbe probe, boolean skip, LongSupplier clock)` delegates to a package-private four-argument test constructor. A non-null refresh timestamp indicates an in-flight refresh. Admission remains single-flight even across invalidation: preserve the old timestamp until the existing worker exits, then clear it through CAS without publishing an obsolete result.
6. Implement `current()` with no blocking and no `probe()` call:
   - Read `now = clock.getAsLong()` and one `CacheState observed = state.get()`.
   - When `skip` is true return `HealthSnapshot.skipped(observed.generation(), now)` without admitting work.
   - When `observed.closed()` is true return `HealthSnapshot.unknown(observed.generation(), now)` without admitting work.
    - Compute snapshot age and in-flight age; only null means no refresh, and a refresh started at clock zero has its real elapsed age.
   - When `snapshotAge >= HEALTH_TTL_MS` or `inflightAge >= HEALTH_TTL_MS`, admit one refresh when none is in flight and return `HealthSnapshot.stale(observed.generation(), max(snapshotAge, inflightAge), now)`. This stale rule applies even after a successful send published a fresh snapshot, because a hung in-flight refresh older than the TTL still makes the status untrustworthy.
    - When the snapshot is `UNKNOWN` and the refresh timestamp is null, admit one refresh through CAS and return the unknown snapshot.
   - When the snapshot is still `UNKNOWN` and a fresh refresh is in flight, return `HealthSnapshot.unknown(observed.generation(), now)`.
   - When `snapshotAge >= REFRESH_AFTER_MS`, admit one refresh when none is in flight and return the existing snapshot so readiness does not flap.
   - Otherwise return the existing snapshot.
7. Admission CASes a null timestamp to `now`, then submits one refresh to the single daemon executor. On submission rejection, identity-check and clear that admission's timestamp to null. There is no accumulating refresh queue. No invalidation can admit a second task while the old task remains active.
8. `runRefresh(long capturedGeneration)` calls the probe. CAS-loop publication changes the snapshot only when the generation still matches and the cache is open. In `finally`, clear the timestamp belonging to this refresh even after invalidation, without changing the new generation's snapshot. Failed checks use credential-safe class-only summaries. Send updates preserve the in-flight timestamp. Introduce `invalidate()` in this task: CAS-advance generation, unknown snapshot, preserve in-flight timestamp, then call non-blocking probe invalidation. Task 1.4 adds terminal close and its race tests, not this already-used method.
9. Add `long generation()`, `HealthSnapshot snapshotValue()` (package-private test accessor), and `recordSendSuccess(long expectedGeneration)` that CASes an `up(expectedGeneration, clock.getAsLong())` snapshot only when the expected generation still matches and the cache is not closed; a stale generation writes nothing.

**Tests:**
- `unknownSchedulesExactlyOneRefresh`: setup a fake `BrokerProbe` blocking on a `CountDownLatch` and a `Supplier` counting executor creation → call `current()` five times with clock `0` → assert every result has `status=UNKNOWN`, `healthy=true`, `brokerConnected=false`, message `broker status not yet checked`, probe calls `1`, executor creation `1`. Command: `nix shell nixpkgs#jdk21_headless -c ./gradlew --no-daemon test --tests org.rustcamel.jms.BridgeHealthCacheTest`. Expected: RED before Step 6 because `BridgeHealthCache` is absent; GREEN after Steps 6-8.
- `verifiedUpAndDownMapToTheContract`: fake probe returns `BrokerProbeResult.up()` then `BrokerProbeResult.down("broker unreachable")` on successive refreshes → after each completed refresh call `current()` → assert the first is `UP`/`healthy=true`/`brokerConnected=true`/`ok`, the second is `DOWN` with both false and `broker unreachable`. Command as above. Expected: RED before Step 8; GREEN after Step 8.
- `skipModeNeverCallsTheProbe`: construct with `skip=true` and a fake probe that throws on any call → call `current()` three times → assert probe calls `0`, results have `status=SKIPPED`, `healthy=true`, `brokerConnected=false`, message `broker probe skipped; connectivity unverified`. Command as above. Expected: RED before Step 6; GREEN after Step 6.
- `verifiedSnapshotAtClockZeroIsNotUnknown`: fake probe returns `up()` at clock `0` → call `current()` once to admit, await the publish, then call `current()` at clock `0` → assert `status=UP`, `healthy=true`, `brokerConnected=true`, `checkedAtMillis=0`, and message `ok`; the explicit status, not the zero timestamp, decides freshness. Command as above. Expected: RED before Steps 1-8 if unknown is inferred from `checkedAtMillis`; GREEN after Steps 1-8.
- `staleAfterTtlWhileRefreshIsBlocked`: fake probe returns up for generation `1`, then blocks → call `current()` at `0`, await the first publish, advance clock to `5_001`, call `current()` (admits refresh `2`, returns the old connected snapshot), advance to `10_001`, call `current()` → assert the last result has `status=STALE`, `healthy=true`, `brokerConnected=false`, message contains `stale` and `10001`; call again and assert probe calls stay `2` and executor creation stays `1`. Command as above. Expected: RED before Steps 6-8; GREEN after Step 8.
- `refreshStartsAtHalfTtlWithoutFlapping`: fake probe returns up at clock `0`; await publish; advance clock to `5_001`, call `current()` → assert the returned snapshot still says `UP`/connected (`healthy=true`, `brokerConnected=true`), the probe call count is `2`, and after the second refresh publishes, `current()` stays connected. Command as above. Expected: RED before Step 6 because no proactive refresh starts at 5 s; GREEN after Step 6.
- `hungRefreshKeepsCurrentStaleAfterSendSuccess`: fake probe blocks forever on its second call → admit the first publish as `up`, advance to `5_001` and call `current()` to start the hung refresh, call `recordSendSuccess(cache.generation())` at `9_000` to publish a fresh `UP`, advance to `15_002`, call `current()` → assert the result is `STALE` because the in-flight refresh age is at least `10_000`, even though the snapshot itself is only `6_002` ms old. Command as above. Expected: RED before Step 6 if only snapshot age is considered; GREEN after Step 6.
- `sendSuccessPublishesVerifiedUp`: fake probe is blocked; call `long gen = cache.generation(); cache.recordSendSuccess(gen);` → assert `current()` returns `status=UP`, `healthy=true`, `brokerConnected=true`, `ok` without waiting for the probe. Command as above. Expected: RED before Step 9; GREEN after Step 9.
- `staleGenerationRefreshAndSendDoNotWrite`: fake probe calls `cache.invalidate()` from inside `probe(1L)` before returning `up()`; separately capture `gen = cache.generation()`, invalidate, then call `recordSendSuccess(gen)` → assert the generation advanced, the snapshot remains `UNKNOWN` with the new generation, and no `UP` snapshot was published by either the stale refresh or the stale send. Command as above. Expected: RED before Steps 8-9 because a stale generation can publish; GREEN after Steps 8-9.
- `executorRejectionClearsInFlightForCurrentGeneration`: supply an executor whose first `execute` throws `RejectedExecutionException` and whose second accepts → call `current()` twice → assert the first submission cleared `refreshStartedAtMillis`, the second call admits and the probe runs exactly once, executor creation stays `1`. Command as above. Expected: RED before Step 7; GREEN after Step 7.
- `expiredRefreshesReuseOneDiagnosticForActiveMq`: build `JmsBrokerProbe` with a counting fake `DiagnosticFactory` for broker type `activemq`, wrap it in a cache, call `current()`, await the first publish, advance the clock past `HEALTH_TTL_MS`, call `current()` again, await the second publish → assert factory opens stay `1`. Command as above. Expected: RED before Steps 3-8; GREEN after Step 8.
- `expiredRefreshesReuseOneDiagnosticForArtemis`: same as the ActiveMQ case with broker type `artemis` → assert factory opens stay `1` across expired refreshes. Command as above. Expected: RED before Steps 3-8; GREEN after Step 8.

**Acceptance:**
- Targeted JUnit class passes.
- `spotlessCheck` exits 0.
- The class contains no `synchronized` block, no `probe()` call from `current()`, and exactly one `AtomicReference<CacheState>`.
- One daemon worker named `jms-health-refresh` is created per cache instance, and `REFRESH_AFTER_MS` is `5_000L`.

- [x] 1.3

#### Task 1.4: Bound invalidation, late-result rejection, and teardown

**Files:**
- `bridges/jms/src/main/java/org/rustcamel/jms/BridgeHealthCache.java` (modified)
- `bridges/jms/src/test/java/org/rustcamel/jms/BridgeHealthCacheTest.java` (modified)

**Steps:**
1. Preserve Task 1.3's non-blocking atomic invalidation, including its existing in-flight timestamp. Add the terminal guard: an already closed cache returns without work. Probe invalidation does no provider I/O on the caller thread. Do not clear admission early or create another worker.
2. In `runRefresh`, publish only by CAS when the captured generation equals the current generation and the state is not closed. A stale result must not publish and must not schedule a replacement worker.
3. Implement `close()` with terminal-once semantics:
   - Add one `ExecutorService closeWorker` created once in the constructor with a daemon thread named `jms-health-close`; this is the only thread allowed to run the provider close.
   - CAS `closed` from false to true exactly once. A concurrent or later `close()` returns immediately without repeating teardown.
    - In the winning CAS, advance generation and install unknown status; preserve in-flight identity until that worker finishes. Terminal state prevents further admission.
   - Call `probe.invalidate(newGeneration)` (non-blocking detach), then `worker.shutdownNow()` and await worker termination for at most `2` seconds.
    - Submit `probe.close()` to `closeWorker`, call its shutdown method, and await at most `1` second before interrupting it. Never await an executor that has not been shut down. Return within three seconds plus scheduling overhead even if provider cleanup hangs. Tests allow four seconds scheduling margin. Do not touch the producer pool or consumer connections.
4. `recordSendSuccess(long expectedGeneration)` ignores every generation that is not current or that is already closed, so a send that began before `invalidate()` or `close()` cannot resurrect old broker status.
5. Make `current()` return the current unknown snapshot after `close()` and never admit new work when `closed` is true.

**Tests:**
- `invalidateRejectsLateRefreshResult`: fake probe captures the generation, blocks on a latch, and on release returns `up()` → call `current()`, call `invalidate()`, release the latch, await the worker → assert `current()` is not connected (`status=UNKNOWN`, `healthy=true`, `brokerConnected=false`, message `broker status not yet checked`) and the fake recorded the new generation as invalidated. Command: `nix shell nixpkgs#jdk21_headless -c ./gradlew --no-daemon test --tests org.rustcamel.jms.BridgeHealthCacheTest`. Expected: RED before Step 2; GREEN after Step 2.
- `invalidateDoesNotCreateSecondWorker`: call `current()` five times, `invalidate()` three times, then `current()` once → assert executor creation count stays `1`, only one `jms-health-refresh` thread exists, and the generation advanced exactly three times. Command as above. Expected: RED before Step 1; GREEN after Step 1.
- `invalidateReturnsPromptlyWhileProviderCloseBlocks`: fake probe records invalidation and its provider close blocks on a latch → call `invalidate()` while measuring elapsed → assert it returns in under `100` ms, the provider close has not run on the caller thread, and the latch is still unreached; release the latch and await cleanup. Command as above. Expected: RED before Step 1 because invalidation closes inline; GREEN after Step 1.
- `lateSendSuccessCannotResurrectInvalidatedGeneration`: read `gen = cache.generation()`, call `invalidate()`, then `recordSendSuccess(gen)` → assert `current()` is not connected. Command as above. Expected: RED before Step 4; GREEN after Step 4.
- `closeReturnsWithinBoundWhenProbeCloseIgnoresInterrupt`: fake probe and fake probe close both block on latches and ignore interrupts → call `close()` and measure elapsed → assert elapsed is at most `4_000` ms (the `3` second bound plus reasonable scheduling overhead), the worker was interrupted, and a following `current()` schedules no work. Command as above. Expected: RED before Step 3 because `close()` is absent; GREEN after Step 3.
- `secondCloseIsNoOpAndClosesProviderOnce`: call `close()` twice → assert the second call returns immediately, the provider close count is exactly `1`, and the worker and close executors are terminated. Command as above. Expected: RED before Step 3 because terminal-once is absent; GREEN after Step 3.
- `closeAndInvalidateRaceClosesResourceExactlyOnce`: run one `close()` and one `invalidate()` on two threads with a start latch → assert both return within the bound, the provider close count is exactly `1`, invalidations are monotonic (the close may contribute the second generation), and `current()` admits no new work. Command as above. Expected: RED before Steps 1-3; GREEN after Step 3.
- `lateRefreshAndSendAfterCloseWriteNothing`: fake probe releases `up()` after `close()`; call `recordSendSuccess` with the pre-close generation → assert the terminal snapshot remains `UNKNOWN` with the post-close generation, no executor is recreated, and no further probe runs. Command as above. Expected: RED before Steps 2-5; GREEN after Step 5.
- `invalidateClosesOwnedDiagnosticOnly`: fake probe records `invalidate` and `close` calls; call `invalidate()` then `close()` → assert exactly two distinct invalidate generations (the explicit invalidation and the terminal close invalidation), exactly one provider `close`, and the cache source contains no `JmsClientFactory.reset()` call. Command as above. Expected: RED before Steps 1-3; GREEN after Step 3.

**Acceptance:**
- Targeted JUnit class passes.
- `spotlessCheck` exits 0.
- `rg -n "reset\\(\\)" bridges/jms/src/main/java/org/rustcamel/jms/BridgeHealthCache.java` returns no matches.
- `close()` is bounded by `awaitTermination(2, TimeUnit.SECONDS)` for the worker and `awaitTermination(1, TimeUnit.SECONDS)` for the dedicated close executor, with the deadline assertion at `4_000` ms.

- [x] 1.4

#### Task 1.5: Wire the cached snapshot into the health and send RPCs

**Files:**
- `bridges/jms/src/main/java/org/rustcamel/jms/JmsBridgeService.java` (modified)
- `bridges/jms/src/main/java/org/rustcamel/jms/JmsClientFactory.java` (modified)
- `bridges/jms/src/test/java/org/rustcamel/jms/JmsBridgeServiceHealthTest.java` (new)

**Steps:**
1. Replace the `lastHealthy`, `lastHealthCheck`, and `lastHealthMessage` fields with `private volatile BridgeHealthCache healthCache`, `private final Object healthCacheLock = new Object()`, and `private volatile boolean shutdownStarted`.
2. Add a lazy accessor that keeps the synchronized section narrow and performs no broker I/O and no eager CDI initialization:
   - Read the volatile `healthCache`; return it when non-null.
   - Otherwise enter `synchronized (healthCacheLock)`, re-check the field, and when `shutdownStarted` is true throw `IllegalStateException("JMS bridge service is shutting down")` instead of creating a cache.
    - When the field is still null, construct `new BridgeHealthCache(clientFactory.healthProbe(), config.brokerProbeMode() == BridgeConfig.BrokerProbeMode.SKIP, System::currentTimeMillis)`, register `clientFactory.setResetHook(created::invalidate)`, assign the field, and return it. Construction performs no broker I/O. Wrap checked mode errors in `IllegalStateException`; the Quarkus startup observer already validates production configuration eagerly.
3. Rewrite `health()` to `responseObserver.onNext(healthCache().current().toResponse())` followed by `onCompleted()`. No broker I/O, no `synchronized (this)`, and no service-wide lock remains in the method.
4. In `send()`, capture `long generation = healthCache().generation()` before `producer.send(request)`. After a successful response and `onCompleted()`, call `healthCache().recordSendSuccess(generation)`. Keep the existing failure logging and `Status.INTERNAL` behavior.
5. In shutdown, acquire `healthCacheLock`, set `shutdownStarted=true`, and capture the existing cache under that same lock used for construction/publication. Release the lock before non-blocking invalidation, consumer drain, and bounded cache close. Never call the lazy accessor from shutdown. This makes first construction and terminal capture mutually exclusive without holding a lock across provider work.
6. Add package-private `BridgeHealthCache healthCacheForTests()` returning the field without lazily creating it, and `void installHealthCacheForTests(BridgeHealthCache cache)` that assigns the field and registers the same reset hook, so tests can inject a fake-clock cache without touching CDI.
7. Remove `JmsClientFactory.checkHealth()` and its single caller together now that the cached path owns all production health access. Add to `JmsClientFactory` a `volatile Runnable resetHook` and package-private `void setResetHook(Runnable hook)`. In `reset()`, invoke the hook before tearing down connection resources so an old in-flight refresh is invalidated through the probe generation. The hook only calls `BridgeHealthCache.invalidate()` and never calls `reset()`, so there is no recursion. Ordinary probe failure never calls `reset()`.
8. Ensure test fixtures close deterministically: a package-private `@AfterEach` method calls `service.shutdown()` inside `try`/`finally` and awaits the cache worker termination, and every injected cache is closed by that fixture even when a test assertion fails.

**Tests:**
- `healthReturnsCachedSnapshotWithoutWaitingForProbe`: inject a cache whose fake probe blocks on a latch, call `health()` twice, measure elapsed time → assert each response is `healthy=true`, `brokerConnected=false`, message `broker status not yet checked`, each call returns under `100` ms, and the probe call count is `1`. Command: `nix shell nixpkgs#jdk21_headless -c ./gradlew --no-daemon test --tests org.rustcamel.jms.JmsBridgeServiceHealthTest`. Expected: RED before Step 3 because the old handler probes the broker synchronously under `synchronized(this)`; GREEN after Step 3.
- `skipModeNeverCallsTheProbe`: inject a cache built with `skip=true` and a fake probe that throws when called → call `health()` → assert `healthy=true`, `brokerConnected=false`, message `broker probe skipped; connectivity unverified`, probe calls `0`. Command as above. Expected: RED before Steps 2-3; GREEN after Steps 2-3.
- `providerFailureDoesNotResetTheProducerPool`: inject a cache whose fake probe returns `down("broker unreachable")`; mock `clientFactory` → call `health()` and await the refresh publish, call `health()` again → assert the response has both fields false, the message is `broker unreachable`, and `verify(clientFactory, never()).reset()` passes. Command as above. Expected: RED before Step 3; GREEN after Step 3.
- `successfulSendPublishesVerifiedUp`: mock `producer.send(request)` to return `"id-1"` and a recording `StreamObserver` → call `send(request, observer)` → call `health()` → assert `healthy=true`, `brokerConnected=true`, message `ok`, and the send observer received the response and completion. Command as above. Expected: RED before Step 4; GREEN after Step 4.
- `lateSendCannotResurrectInvalidatedGeneration`: mock `producer.send(request)` to block on a latch → call `send(request, observer)` on an executor, wait for entry, call `service.healthCacheForTests().invalidate()`, release the latch, await the send → call `health()` → assert the snapshot is not connected. Command as above. Expected: RED before Steps 4-6; GREEN after Steps 4-6.
- `providerFailureDoesNotEvictActiveConsumers`: insert a mock `JmsConsumer` into the service's `activeConsumers` map through a local reflection helper that mirrors `JmsBridgeServiceTest.activeConsumersOf`, inject a cache whose fake probe returns `down("broker unreachable")`, await the refresh, then call `health()` → assert `health()` returns both fields false, the same consumer instance is still in `activeConsumers`, and `verify(consumer, never()).stop()` passes. Command as above. Expected: RED before Steps 3-7 because the old handler races the broker and the test cannot pin the map; GREEN after Steps 3-7.
- `concurrentFirstHealthAccessCreatesOneCacheAndWorker`: mock `clientFactory.healthProbe()` to return one blocking fake probe, start eight threads on a `CountDownLatch`, have each call `health()` once → assert all eight observe the same non-null `healthCacheForTests()` instance, exactly one thread named `jms-health-refresh` exists, the probe call count is `1`, and every response is `UNKNOWN`/unverified. Command as above. Expected: RED before Step 2 because the accessor is eager; GREEN after Step 2.
- `shutdownNeverCreatesACache`: construct the service without calling `health()`, call `shutdown()`, then call `shutdown()` again → assert `healthCacheForTests()` is null, no `jms-health-refresh` thread was created, and a concurrent `health()` racing the shutdown either throws `IllegalStateException` with the shutdown message or observes the captured terminal cache, never a newly created worker. Command as above. Expected: RED before Steps 2-5 because shutdown uses the lazy accessor; GREEN after Steps 2-5.
- `shutdownCapturesCacheBeingConstructed`: gate cache creation after the accessor acquires `healthCacheLock`; start shutdown concurrently; release construction → assert shutdown captures and closes that one cache, no subsequent accessor constructs another, and all owned workers terminate after test latches release. Command as above. Expected: RED when shutdown reads outside the initialization lock; GREEN after Step 5.
- `factoryResetInvalidatesActiveRefreshGeneration`: inject a cache with a gated fake probe; call `health()` to admit the refresh; call the real `clientFactory.reset()` under the reset hook → assert the fake probe recorded the next generation as invalidated, the late refresh result is not published, and `health()` remains `UNKNOWN`; no provider close runs on the reset caller. Command as above. Expected: RED before Steps 2 and 7 because no reset hook exists; GREEN after Steps 2 and 7.
- `factoryResetIsNotCalledOnProbeFailure`: inject a cache whose probe returns `down("broker unreachable")`, await the publish, call `health()` → assert `verify(clientFactory, never()).reset()` and the probe retains its generation. Command as above. Expected: RED if Step 7 wires failure to reset; GREEN after Step 7.

**Acceptance:**
- Targeted JUnit class passes.
- `spotlessCheck` exits 0.
- `rg -n "synchronized" bridges/jms/src/main/java/org/rustcamel/jms/JmsBridgeService.java` shows no `synchronized (this)` and no synchronized block inside `health()`.
- The service never calls `clientFactory.reset()` from a probe path, and `checkHealth` no longer exists in either `JmsClientFactory` or `JmsBridgeService`.

- [x] 1.5

#### Task 1.6: Prove T5 and the Java half of T6 through the real handler

**Files:**
- `bridges/jms/src/test/java/org/rustcamel/jms/JmsBridgeServiceHealthTest.java` (modified)
- `bridges/jms/src/test/java/org/rustcamel/jms/BridgeHealthCacheTest.java` (modified)

**Steps:**
1. Add a reusable test helper inside `BridgeHealthCacheTest` that builds a fake `BrokerProbe` with a `CountDownLatch` gate, a `BrokerProbeResult` result, and counting opens/verifies. Keep it package-private in the test source; do not add test-only branches to production policy.
2. Add clock-driven Java T6 with a blocked probe and calls at `0`, `5_000`, `10_000`, and `15_000` ms, representing at least three 5-second periods. Each real handler call finishes inside the test's configured `100` ms deadline; fake diagnostic opens stay one.
3. Add the T5 interleaving test: after the first health call admits the blocked refresh, call `complete(CompletionRequest.getDefaultInstance(), observer)` and another `health()` before releasing the gate. Assert both complete within `100` ms, the probe was invoked once, and the executor still has one worker.
4. Assert T6 language explicitly in the test Javadoc: this is Java-side paired-contract coverage for a blocked fake broker, not a live-broker or native-image soak; the Rust real monitor supplies the zero-restart half and CI supplies native/live-broker evidence.

**Tests:**
- `t6BlockedBrokerThreeProbePeriodsKeepsHealthPromptAndOpensFlat`: setup the real handler with blocked fake probe, fake clock zero, deadline `100` ms and period `5000` ms → call health at `0`, `5000`, `10000`, `15000`; assert every call finishes within deadline, returns live/unverified, opens stay one, and simulated elapsed time is at least three periods. Command: `nix shell nixpkgs#jdk21_headless -c ./gradlew --no-daemon test --tests org.rustcamel.jms.JmsBridgeServiceHealthTest`. Expected: RED before cached handling; GREEN after Steps 1-2.
- `t5BlockedBrokerDoesNotDelayHealthOrUnrelatedRpc`: setup the same blocked probe → call `health()`, then `complete(CompletionRequest.getDefaultInstance(), observer)`, then `health()` before releasing → assert each call returns within `100` ms, the completion observer receives `accepted=false` and `onCompleted()`, and the probe invocation count stays `1`. Command as above. Expected: RED before Step 3; GREEN after Step 3.
- `repeatedHealthCallsDoNotQueueRefreshWork`: setup the blocked probe → call `health()` twenty times before release → assert fake probe invocations `1`, executor creation `1`, and every response arrives within `100` ms. Command as above. Expected: RED before Step 3; GREEN after Step 3.

**Acceptance:**
- Both targeted JUnit classes pass.
- `spotlessCheck` exits 0.
- The T6 test name and Javadoc contain `paired-contract`, `not a live-broker or native-image soak`, and `CI`.
- No production file gains a test-only branch in this task.

- [x] 1.6

---

## Phase 2: Engine policy and complete contract

### camel-component-jms

#### Task 2.1: Add the three pool health settings with defaults and validation

**Files:**
- `crates/components/camel-jms/src/config.rs` (modified)
- `crates/components/camel-jms/src/lib.rs` (modified)

**Steps:**
1. Add `#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)] #[serde(rename_all = "lowercase")] pub enum HealthBrokerProbeMode { Reuse, Skip }` with an explicit `impl Default` returning `Reuse` and `pub(crate) fn as_env_str(self) -> &'static str` returning `reuse` or `skip`. Do not derive `Serialize` and do not add unused public helpers; keep the type surface limited to what config deserialization and the bridge environment need.
2. Add `fn default_health_timeout_ms() -> u64 { 3_000 }` and `fn default_health_failure_threshold() -> u32 { 3 }`.
3. Add to `JmsPoolConfig`: `#[serde(default = "default_health_timeout_ms")] pub health_timeout_ms: u64`, `#[serde(default = "default_health_failure_threshold")] pub health_failure_threshold: u32`, and `#[serde(default)] pub health_broker_probe_mode: HealthBrokerProbeMode`.
4. Initialize the same values in `JmsPoolConfig::default()`.
5. Extend `validate()`: return `CamelError::Config("health_timeout_ms must be > 0")` when zero and `CamelError::Config("health_failure_threshold must be > 0")` when zero. Unknown serde modes are rejected by `Deserialize` before `validate()`.
6. Re-export `HealthBrokerProbeMode` from `crates/components/camel-jms/src/lib.rs` beside the existing config exports.
7. Do not add Tokio test-util or modify manifests for monitor timing. Task 2.4 uses the real clock with bounded event-driven waits and no arbitrary sleep/poll loops; this avoids paused-time auto-advance racing real tonic socket delivery. Task 2.6 owns schema dependency changes.
8. Ownership boundaries: this task does not add a `BridgeSlot` field. The per-slot `Notify` and every `BridgeSlot` literal change belong only to Task 2.5. Task 2.2 carries the probe mode into the pool; Task 2.4 must not add a duplicate `health_broker_probe_mode` field.

**Tests:**
- `health_settings_default_to_3000_3_and_reuse`: setup `JmsPoolConfig::default()` → assert `health_timeout_ms == 3000`, `health_failure_threshold == 3`, `health_broker_probe_mode == HealthBrokerProbeMode::Reuse`, and `health_check_interval_ms == 5000`. Command: `rc_cargo t21 test -p camel-component-jms --lib health_settings_default_to_3000_3_and_reuse`. Expected: RED before Steps 1-4 because the fields do not exist (compile failure); GREEN after Step 4.
- `partial_toml_receives_health_defaults`: deserialize `brokers.default` only with `toml::from_str::<JmsPoolConfig>` → assert the three defaults above. Command: `rc_cargo t21 test -p camel-component-jms --lib partial_toml_receives_health_defaults`. Expected: RED before Steps 1-4; GREEN after Step 4.
- `toml_parses_skip_probe_mode`: deserialize `health_broker_probe_mode = "skip"` → assert `HealthBrokerProbeMode::Skip`. Command: `rc_cargo t21 test -p camel-component-jms --lib toml_parses_skip_probe_mode`. Expected: RED before Step 1; GREEN after Step 1.
- `toml_rejects_unknown_probe_mode`: deserialize `health_broker_probe_mode = "probe"` → assert `Err` and the error names `health_broker_probe_mode`. Command: `rc_cargo t21 test -p camel-component-jms --lib toml_rejects_unknown_probe_mode`. Expected: RED before Step 1; GREEN after Step 1.
- `validate_rejects_zero_health_timeout`: setup default config, set `health_timeout_ms = 0` → call `validate()` → assert `Err` containing `health_timeout_ms`. Command: `rc_cargo t21 test -p camel-component-jms --lib validate_rejects_zero_health_timeout`. Expected: RED before Step 5; GREEN after Step 5.
- `validate_rejects_zero_health_failure_threshold`: setup default config, set `health_failure_threshold = 0` → call `validate()` → assert `Err` containing `health_failure_threshold`. Command: `rc_cargo t21 test -p camel-component-jms --lib validate_rejects_zero_health_failure_threshold`. Expected: RED before Step 5; GREEN after Step 5.

**Acceptance:**
- All six filtered tests pass.
- `rc_cargo t21-check check -p camel-component-jms` exits 0.
- `rc_cargo t21-fmt fmt --check` reports no diff for the two source files.
- No new timing dependency or lockfile change is introduced by this task.

- [x] 2.1

#### Task 2.2: Carry the probe mode into every bridge launch environment

**Files:**
- `crates/components/camel-jms/src/component.rs` (modified)

**Steps:**
1. Add `pub(crate) health_broker_probe_mode: HealthBrokerProbeMode` to `JmsBridgePool` and copy `pool_config.health_broker_probe_mode` in `from_config`.
2. Add `pub(crate) fn health_probe_env(mode: HealthBrokerProbeMode) -> (String, String)` returning `("JMS_HEALTH_BROKER_PROBE_MODE".to_string(), mode.as_env_str().to_string())`.
3. Add a `health_broker_probe_mode: HealthBrokerProbeMode` parameter to the private `start_bridge_process` in addition to its existing parameters.
4. Make `process_config` mutable in `start_bridge_process` and push `health_probe_env(health_broker_probe_mode)` into the existing `process_config.env_vars` after the existing `BridgeProcessConfig::jms` call. Do not modify `crates/services/camel-bridge`.
5. Pass `self.health_broker_probe_mode` from `get_or_create_slot` and from the health monitor restart branch.
6. Task 2.4 must not add this field again; this task owns the only pool copy of the mode.

**Tests:**
- `health_probe_env_uses_lowercase_names`: setup `Reuse` and `Skip` → call `health_probe_env` → assert keys are both `JMS_HEALTH_BROKER_PROBE_MODE` and values are `reuse` and `skip`. Command: `rc_cargo t22 test -p camel-component-jms --lib health_probe_env_uses_lowercase_names`. Expected: RED before Step 2 because the helper is absent; GREEN after Step 2.
- `start_bridge_process_exports_health_probe_mode`: setup a Unix stub script that writes `$JMS_HEALTH_BROKER_PROBE_MODE` to a temp file, prints `{"status":"ready","port":1}`, and sleeps; call the private `start_bridge_process` with `health_broker_probe_mode = HealthBrokerProbeMode::Skip` and `bridge_start_timeout_ms = 200` under `BRIDGE_ENV_LOCK` → assert the mode file contains `skip` even though connect/health fails afterwards. Command: `rc_cargo t22 test -p camel-component-jms --lib start_bridge_process_exports_health_probe_mode`. Expected: RED before Steps 3-4 because the env var is absent; GREEN after Step 4.
- `pool_carries_skip_mode_from_config`: setup `JmsPoolConfig { health_broker_probe_mode: HealthBrokerProbeMode::Skip, .. }` → build `JmsBridgePool` → assert `pool.health_broker_probe_mode == HealthBrokerProbeMode::Skip`. Command: `rc_cargo t22 test -p camel-component-jms --lib pool_carries_skip_mode_from_config`. Expected: RED before Step 1 because the field is absent; GREEN after Step 1.

**Acceptance:**
- All three filtered tests pass.
- `rg -n "health_broker_probe_mode" crates/components/camel-jms/src/component.rs` shows the pool field copy, the initial launch argument, and the restart launch argument.
- `git diff --quiet -- crates/services/camel-bridge` exits 0.
- `rc_cargo t22-check check -p camel-component-jms` exits 0.

- [x] 2.2

#### Task 2.3: Map the bridge response table to readiness without losing broker status

**Files:**
- `crates/components/camel-jms/src/health.rs` (modified)

**Steps:**
1. Add `#[derive(Debug, Clone, PartialEq, Eq)] pub(crate) struct BridgeProbeResult { pub healthy: bool, pub broker_connected: bool, pub message: String }`.
2. Change `type HealthFuture` to `Result<BridgeProbeResult, CamelError>` and change `BridgeHealthProbe::probe` to return it.
3. In the real probe, convert the proto `HealthResponse` into `BridgeProbeResult` and return it instead of only `resp.healthy`.
4. Keep `not_started()` lazy behavior and its `Degraded` message exactly.
5. Map in `JmsHealthCheck::check`: `healthy && broker_connected` to `Healthy`; `!healthy` to `Unhealthy` with the result message when non-empty, otherwise `bridge reported unhealthy`; `healthy && !broker_connected` to `Degraded` with the result message when non-empty, otherwise `broker status unverified`. RPC errors and timeouts remain `Unhealthy`.
6. Update the mock probes in the test module to return `BridgeProbeResult`.

**Tests:**
- `jms_health_check_healthy_when_verified_up`: mock returns `healthy=true`, `broker_connected=true`, `message="ok"` → call `check()` → assert `HealthStatus::Healthy` and no message. Command: `rc_cargo t23 test -p camel-component-jms --lib jms_health_check_healthy_when_verified_up`. Expected: RED before Steps 1-5 because the signature is still `bool`; GREEN after Step 5.
- `jms_health_check_unhealthy_when_verified_down`: mock returns `healthy=false`, `broker_connected=false`, `message="broker unreachable"` → call `check()` → assert `HealthStatus::Unhealthy` and message `broker unreachable`. Command: `rc_cargo t23 test -p camel-component-jms --lib jms_health_check_unhealthy_when_verified_down`. Expected: RED before Step 5; GREEN after Step 5.
- `jms_health_check_degraded_for_unknown_or_skipped`: mock returns `healthy=true`, `broker_connected=false`, `message="broker probe skipped; connectivity unverified"` twice → assert both results are `HealthStatus::Degraded` and preserve the message. Command: `rc_cargo t23 test -p camel-component-jms --lib jms_health_check_degraded_for_unknown_or_skipped`. Expected: RED before Step 5 because the old code maps `true` to Healthy; GREEN after Step 5.
- `jms_health_check_degraded_for_stale`: mock returns `healthy=true`, `broker_connected=false`, `message="broker status stale; age 10001 ms"` → assert `Degraded` and the exact message. Command: `rc_cargo t23 test -p camel-component-jms --lib jms_health_check_degraded_for_stale`. Expected: RED before Step 5; GREEN after Step 5.
- `jms_health_check_degraded_when_bridge_not_started`: existing lazy probe returns `not_started=true` and a probe that must not run → assert `Degraded` and the message contains `not started`. Command: `rc_cargo t23 test -p camel-component-jms --lib jms_health_check_degraded_when_bridge_not_started`. Expected: RED if Step 4 changes the message; GREEN after Step 4.
- `jms_health_check_unhealthy_on_error_and_timeout`: existing error probe and sleeping probe with a 5 ms timeout → assert `Unhealthy` for both and the timeout message stays `health probe timed out`. Command: `rc_cargo t23 test -p camel-component-jms --lib jms_health_check_unhealthy_on`. Expected: RED before Step 5 if mapping changes; GREEN after Step 5.

**Acceptance:**
- All filtered `health::tests` pass.
- `rc_cargo t23-clippy clippy -p camel-component-jms --all-targets -- -D warnings` exits 0.
- The table in `specs/jms/spec.md` is represented by one test per row.

- [x] 2.3

#### Task 2.4: Apply the configured liveness timeout and failure threshold in the real monitor

**Files:**
- `crates/components/camel-jms/src/bridge_client_test.rs` (modified)
- `crates/components/camel-jms/src/component.rs` (modified)

**Steps:**
1. Add to `bridge_client_test.rs` the test-only scripted health fixtures:
   - `pub(crate) enum ScriptedHealth { Respond(HealthResponse), Gate { response: HealthResponse, gate: Arc<ProbeGate> }, Error(Status) }`.
   - `pub(crate) struct ProbeGate { pub entered: Arc<tokio::sync::Notify>, pub release: Arc<tokio::sync::Notify> }`.
   - `pub(crate) struct ScriptedHealthHandle { pub port: u16, pub calls: Arc<AtomicUsize>, pub events: tokio::sync::mpsc::UnboundedReceiver<HealthEvent>, shutdown: tokio::sync::oneshot::Sender<()>, task: tokio::task::JoinHandle<()> }` with `pub async fn shutdown(self)` that sends the shutdown signal and awaits the task under a bounded `tokio::time::timeout`, aborting the task if the bound expires. The handle owns the server task and its shutdown sender; no detached server survives a test.
   - `pub(crate) enum HealthEvent { Started, Completed }`; these are server-side events, not proof that the client monitor processed the response. Add a test-only per-pool `probe_processed` unbounded channel carrying probe index, current state, and outcome (successful response, timeout, or RPC error). The real monitor sends this acknowledgement only after applying its actual counter/reporting/publication path. Assertions await monitor acknowledgements; server events only coordinate gates.
2. The scripted server pops one entry per health call, increments `calls`, applies any gate before returning the response or status. When the queue is empty it returns `healthy=true`, `broker_connected=true`, `message="ok"` so a test can enqueue only the exceptional prefix. Add a `TestBridgeFixture` that owns the monitor `JoinHandle`, the `ScriptedHealthHandle`, and an optional real child process (`tokio::process::Child` with `kill_on_drop(true)`); `shutdown().await` aborts/joins the monitor, shuts down the server, and kills/awaits the child under bounded timeouts, and `Drop` aborts the same handles so a panicking test leaks nothing.
3. Add `pub(crate) health_timeout_ms: u64` and `pub(crate) health_failure_threshold: u32` to `JmsBridgePool`; copy both from `JmsPoolConfig` in `from_config`. Do not add `health_broker_probe_mode` here; Task 2.2 already owns that field. Add `#[cfg(test)] pub(crate) broker_status_transitions: Arc<std::sync::Mutex<Vec<BrokerStatusTransition>>>` and `#[cfg(test)] pub(crate) replacement_attempts: Arc<std::sync::atomic::AtomicUsize>`, both initialized in `from_config`.
4. Add test-only `BrokerStatusTransition { VerifiedUp, BrokerDown, Unverified }` and `replacement_outcomes: Arc<tokio::sync::Mutex<VecDeque<Result<(BridgeProcess, Channel), StartBridgeError>>>>`. Tests prepare real `BridgeProcess` instances with the existing ready-handshake child stub and pair them with a real scripted-server channel. The existing replacement branch counts the attempt, stops the prior process, and substitutes only the launch result from this queue when present. It MUST then execute the unchanged `slot.install_ready_process(process, channel)` and real success/error handling; no synthetic `Ok(())`, skipped Ready publication, or duplicate threshold policy. With an empty queue, launch normally. Count attempts after terminal/restart-budget guards but before stopping the old process; add a test-only replacement-processed notification after actual installation/error handling. Queue/fixture teardown stops any unused prepared processes.
5. In the real monitor, on every `Ok(Ok(resp))` explicitly read both `resp.healthy` and `resp.broker_connected`; the branch must report broker conditions instead of discarding them. Add `fn report_broker_status(&self, last: &mut Option<(bool, bool)>, healthy: bool, broker_connected: bool)` that emits one bounded, credential-safe `tracing::warn!` only when the `(healthy, broker_connected)` pair differs from `last`, with a fixed message that contains no URL, username, password, or provider message. Under `#[cfg(test)]`, the same method records the matching `BrokerStatusTransition` in `broker_status_transitions` instead of exposing raw credentials. This status reporting never restarts the bridge.
6. Change `async fn spawn_health_monitor` to `pub(crate) async fn spawn_health_monitor` so component and consumer tests invoke the real monitor.
7. In `spawn_health_monitor`, capture `health_timeout = Duration::from_millis(self.health_timeout_ms)` and `threshold = self.health_failure_threshold.max(1)` before the task. Keep one `consecutive_failures: u32` and one `last_broker_status: Option<(bool, bool)>` local to the monitor task; pass `&mut last_broker_status` into `report_broker_status` so transitions are logged only on change and the local state survives across loop iterations.
8. On `Ok(Ok(_))` reset `consecutive_failures` to `0` regardless of `healthy` and `broker_connected`; the completion of the RPC is liveness proof. Report the broker status through Step 5 and stay in `Ready`; broker conditions never publish `Degraded` or trigger a replacement. On `Ok(Err(_))` or timeout increment `consecutive_failures`. Below `threshold`, stay in `Ready` and do not publish `Degraded`. At or above `threshold`, publish `Degraded` through `publish_unless_terminal` with the existing timeout/error text, then reset the counter after a successful replacement install.
9. Reset `consecutive_failures = 0` after a successful replacement install. Do not change the initial-start failure path or terminal guards.
10. The monitor must call the real `bridge_service_client(channel).health(HealthRequest {})`; tests use the scripted server and never reimplement the policy. Tests must not contain decision logic: they only arrange scripted responses and assert observed state, counters, and transitions.
11. Timing discipline: use the real Tokio clock, not pause/advance around real sockets. Await gate and monitor-side notifications under five-second outer bounds. The production monitor supplies its normal cadence sleeps; test bodies contain no arbitrary sleeps or polling. Server-side completion alone is insufficient for assertions. Connect sockets and prepare real child stubs before starting timed assertions. Test fixture cleanup aborts and joins every monitor/server and stops every installed or unused child on success or panic.

**Tests:**
- `t1_monitor_reports_verified_down_and_keeps_ready_consumer`: setup interval `20` ms, timeout `250` ms, threshold `5`, real channel and child, response `healthy=false, broker_connected=false`; spawn the actual monitor and await its successful processed acknowledgement → assert `BrokerDown` was reported, the Ready channel and child PID are unchanged, and attempts remain zero. Java `providerFailureDoesNotEvictActiveConsumers` supplies attached-consumer preservation coverage. Command: `rc_cargo t24 test -p camel-component-jms --lib t1_monitor_reports_verified_down_and_keeps_ready_consumer`. Expected: reporting/observer absent before implementation; GREEN afterward.
- `t2_monitor_restarts_exactly_at_nth_failure`: setup threshold `3`, timeout `250` ms, interval `20` ms, one prepared real replacement process/channel, and three RPC errors; await each monitor processed acknowledgement → assert zero attempts after failures one and two, then await replacement-processed after failure three and assert exactly one attempt, Ready state, and actual replacement child installed. Command: `rc_cargo t24 test -p camel-component-jms --lib t2_monitor_restarts_exactly_at_nth_failure`. Expected: RED with restart-on-first-error; GREEN after threshold handling.
- `t2_monitor_timeouts_respect_threshold_and_success_reset`: setup threshold `3`, interval `20` ms, timeout `50` ms, one prepared actual replacement process/channel; script gated timeout, successful false/false response, then three gated timeouts. Hold each timed-out response beyond its deadline; await monitor-side acknowledgement for every outcome. Assert attempts stay zero through the penultimate outcome, then exactly one replacement after the final timeout and actual prepared child/channel installed Ready. Release all gates in bounded fixture teardown. Command: `rc_cargo t24 test -p camel-component-jms --lib t2_monitor_timeouts_respect_threshold_and_success_reset`. Expected: RED if timeout restarts immediately or successful response fails to reset; GREEN after shared threshold handling.
- `t2_monitor_resets_failure_count_on_successful_false_response`: setup threshold `2`, timeout `250` ms, interval `20` ms; script error, successful false/false response, error; await three monitor acknowledgements → assert zero attempts and Ready throughout. Command: `rc_cargo t24 test -p camel-component-jms --lib t2_monitor_resets_failure_count_on_successful_false_response`. Expected: RED with restart-on-first-error or failure to reset after success; GREEN after implementation.
- `t2_monitor_resets_failure_count_after_successful_replacement`: setup threshold `2`, timeout `250` ms, interval `20` ms, two prepared real replacement process/channel pairs and four RPC errors; await monitor and replacement acknowledgements → assert zero/one/one/two attempts after successive errors and actual Ready installation after each pair. Command: `rc_cargo t24 test -p camel-component-jms --lib t2_monitor_resets_failure_count_after_successful_replacement`. Expected: RED without generation reset; GREEN with the actual installation path.
- `t3_monitor_accepts_response_before_configured_timeout`: setup timeout `500` ms, threshold `1`, interval `20` ms and gated healthy response; await server entry, promptly release, then await monitor success → assert response completed within deadline, Ready, zero attempts. Command: `rc_cargo t24 test -p camel-component-jms --lib t3_monitor_accepts_response_before_configured_timeout`. Expected: baseline-compatible regression; GREEN after new fixture compiles. This case pairs with the beyond-deadline test rather than claiming baseline failure.
- `t3_monitor_times_out_after_configured_timeout`: setup timeout `50` ms, threshold `1`, interval `20` ms, one real replacement process/channel and a gated response; hold the gate, await monitor timeout and replacement-processed under a one-second bound → assert elapsed since request entry is at least `50` ms but below one second, exactly one replacement, and actual Ready installation; release the old response and assert it cannot overwrite replacement state. Command: `rc_cargo t24 test -p camel-component-jms --lib t3_monitor_times_out_after_configured_timeout`. Expected: RED with hardcoded three-second deadline; GREEN after configured timeout.
- `t6_real_monitor_three_stale_periods_zero_restarts_and_stable_child`: setup actual pool interval `20` ms, timeout `250` ms, threshold `5`, real slot child and real tonic channel; script five prompt `healthy=true, broker_connected=false` stale responses. Call real `spawn_health_monitor`, await its first processed acknowledgement and record real monotonic time, then await three more processed acknowledgements under five seconds → assert elapsed at least `3 * interval`, at least four completed probes total, Ready at every acknowledgement, zero replacement attempts throughout and finally, and the slot's actual process PID unchanged. Command: `rc_cargo t24 test -p camel-component-jms --lib t6_real_monitor_three_stale_periods_zero_restarts_and_stable_child`. Expected: fixture compile RED only; baseline already accepts broker false. GREEN afterward. No duplicate decision logic. A restart-on-broker-false mutation must fail this regression.
- `t6_real_monitor_verified_down_false_false_zero_restarts`: setup actual pool interval `20` ms, timeout `250` ms, threshold `5`, real slot child/channel and five prompt false/false verified-down replies; call real monitor, await one processed acknowledgement then three more → assert at least three real periods elapsed, `BrokerDown` reported, Ready throughout, zero attempts, and unchanged actual slot PID. Command: `rc_cargo t24 test -p camel-component-jms --lib t6_real_monitor_verified_down_false_false_zero_restarts`. Expected: fixture compile RED only; GREEN after implementation. Restart-on-broker-status mutation must fail.
- `initial_launch_failure_path_is_unchanged`: call `JmsBridgePool::apply_initial_start_result` with `Err(StartBridgeError::Other("initial launch failed".to_string()))` → assert `Degraded` exactly as before and `replacement_attempts == 0`. Command: `rc_cargo t24 test -p camel-component-jms --lib initial_launch_failure_path_is_unchanged`. Expected: RED if Step 9 changes initialization; GREEN after Step 9.
- Retain the existing terminal-state monitor tests unchanged: `parked_slot_is_not_respawned`, `late_ready_or_restart_publication_preserves_parked`, and `restart_slot_does_not_overwrite_stopped`. They must still pass after the monitor edits.

**Acceptance:**
- All filtered monitor tests pass.
- `rg -n "Duration::from_secs\\(3\\)" crates/components/camel-jms/src/component.rs` returns no match in the monitor.
- `rg -n "spawn_health_monitor" crates/components/camel-jms/src/component.rs` shows `pub(crate)`.
- No test uses `tokio::time::sleep` or a polling loop; `rc_cargo t24-lint xtask lint-test-sleep` and `rc_cargo t24-unbounded xtask lint-unbounded-wait` exit 0.
- `rc_cargo t24-clippy clippy -p camel-component-jms --all-targets -- -D warnings` exits 0.

- [x] 2.4

#### Task 2.5: Make restart_slot a coalesced liveness wake-up for producer and consumer callers

**Files:**
- `crates/components/camel-jms/src/component.rs` (modified)
- `crates/components/camel-jms/src/consumer.rs` (modified)
- `crates/components/camel-jms/src/bridge_client_test.rs` (modified, shared test fixtures)

**Steps:**
1. Add `pub(crate) probe_wake: Arc<tokio::sync::Notify>` as an owned field of `BridgeSlot`. Update every `BridgeSlot` struct literal in `component.rs`, `consumer.rs`, and their test modules to pass a fresh `Arc::new(tokio::sync::Notify::new())`. No other task in this change touches `BridgeSlot` literals; Task 2.5 owns all of them. Verify with `rg -n "BridgeSlot \\{"` that every literal has the field.
2. Rewrite `restart_slot` so it never writes `BridgeState::Restarting`, never starts, stops, or degrades a bridge, and never fakes a watch change. Read the current state; if it is `Parked` or `Stopped`, keep the existing warning and return. Otherwise call `slot.probe_wake.notify_one()` and return. `Notify` coalesces repeated requests and stores one permit when the monitor is between waits.
3. In the Ready arm, select between a fresh `tokio::time::sleep(Duration::from_millis(health_interval))` and `slot.probe_wake.notified()`. Do not replace the fresh sleep with `interval.tick()`: no immediate initial tick or catch-up burst is allowed. Without explicit wake-ups, preserve the existing full interval after each probe completion. After either wait, reread lifecycle/generation/current channel before probing; terminal or no-longer-Ready state skips the probe. A wake requests verification but does not increment the failure count.
4. Keep the terminal/publication guards in `publish_unless_terminal` untouched. A wake cannot resurrect `Parked` or `Stopped`, and a wake arriving during an in-flight probe only sets the permit for the next loop iteration; it never starts an overlapping probe.
5. Extract the existing duplicated consumer refresh/cancel/wake action into private `async fn refresh_channel_or_request_probe(pool: &JmsBridgePool, broker_name: &str, destination: &str, idx: usize, cancel: &CancellationToken, ctx: &ConsumerContext) -> RecoveryOutcome`, with private `enum RecoveryOutcome { Continue, Cancelled }`. Both existing consumer sites call it. Move their cancellation select, failed-refresh warning, and `pool.restart_slot(broker_name)` into this helper; map `Cancelled` to the original return. Keep every counter increment, threshold (`>= 2`), and reset at the original sites unchanged. Do not seed counters in tests or change reconnect policy. Change the producer and helper warning to `requesting liveness probe`.
6. Comment beside the natural stream-error branch: successful subscribe resets `consecutive_transport_failures` to zero, so the first stream error reaches one and breaks; the existing threshold-two branch is unreachable under this invariant. Cite `rc-5vfz3`. Its shared recovery action is tested directly and its wiring statically; do not claim end-to-end runtime coverage of that natural branch.
7. Introduce private `const CHANNEL_REFRESH_TIMEOUT_MS: u64 = 2_000` in `component.rs`. In `refresh_slot_channel`, wrap process-lock acquisition and the entire selected connector future in one absolute two-second timeout established on method entry. Partial connection progress, retries, or progress callbacks never renew it. Timeout returns a fixed credential-safe `CamelError::ProcessorError`, releases ownership/locks, and never publishes restart or increments liveness failures. The consumer helper requests a liveness probe after refresh failure or expiration; cancellation remains selectable. This bounds one recovery operation to two seconds plus the separately tested 500 ms probe-wake interval, excluding scheduler margin; it does not bound an entire reconnect loop or delivery.
8. Add test-only `RefreshAttempt` observations on `JmsBridgePool`, with broker name, actual selected endpoint (optional when no process was selected), failed flag, and timeout flag. Record completion inside `refresh_slot_channel`, including the timeout path, using the endpoint read from the process actually passed to `connect()`. Publish a bounded-wait notification after recording. A test-only connector override may substitute only the selected connector future, after real endpoint selection and inside the unchanged production deadline. All selection, timeout, observation, and lifecycle policy remain real.
9. Rewrite the existing producer refresh tests to observe the attempt generated by the producer call itself. Remove manual follow-up refresh calls as evidence. Keep original transport error and no-resend assertions. Consumer regression tests install real ready-handshake processes with distinct dead endpoints, coordinate retries with gates, and observe the second actual endpoint failure. Start the 500 ms wake assertion after this failure observation, with a separate fixed outer deadline covering connection and wake; progress must not reset either deadline.
10. Retain the bodies of existing counter/threshold tests unchanged: `retry_loop_invokes_operation_exactly_max_attempts_times`, `reconnect_exhaustion_is_surfaced`, `transient_error_still_reconnects`, and all Task 2.4 `t2_monitor_*` tests. Compare their source to commit `98cd8972` and run them. Re-run `parked_slot_is_not_respawned`, `late_ready_or_restart_publication_preserves_parked`, and `restart_slot_does_not_overwrite_stopped`. Extend shared test fixtures only for real process ownership, actual refresh observations, and deterministic connector/cancellation gates.

**Tests:**
- `lazy_producer_requests_liveness_probe_when_refresh_unavailable`: arrange failed-send channel and installed distinct dead-process refresh endpoint; invoke the real producer once; inspect observations created before that call returns, without a manual refresh → assert failed refresh names that actual second endpoint, original transport error survives, Ready is preserved. Command: `rc_cargo t25 test -p camel-component-jms --lib lazy_producer_requests_liveness_probe_when_refresh_unavailable`. Expected: RED before real-attempt observations exist; GREEN after implementation.
- `transport_error_requests_liveness_probe_and_does_not_resend`: arrange a counted failing send and distinct dead-process endpoint; invoke producer once and inspect its own refresh observation → assert one send, original `BRIDGE_TRANSPORT_ERROR_PREFIX`, selected second endpoint failed, and Ready preserved. Command: `rc_cargo t25 test -p camel-component-jms --lib transport_error_requests_liveness_probe_and_does_not_resend`. Expected: RED without actual-call evidence; GREEN afterward.
- `restart_slot_wakes_monitor_and_preserves_ready`: setup a scripted healthy bridge, interval `60_000` ms, slot `Ready`, spawn monitor, call `pool.restart_slot("default")` → assert the scripted calls counter reaches `1` within `500` ms without advancing the 60 s cadence, state is `Ready`, and `replacement_attempts == 0`. Command: `rc_cargo t25 test -p camel-component-jms --lib restart_slot_wakes_monitor_and_preserves_ready`. Expected: RED before Steps 1-3 because no probe happens before 60 s; GREEN after Steps 1-3.
- `repeated_restart_requests_coalesce_to_one_probe`: setup scripted bridge and 60 s interval, notify ten times before monitor wait, then spawn the real monitor → assert the scripted health-call counter and monitor processed acknowledgement show exactly one probe, no overlap, and no failure increment. Command: `rc_cargo t25 test -p camel-component-jms --lib repeated_restart_requests_coalesce_to_one_probe`. Expected: RED without coalescing wake policy; GREEN afterward.
- `producer_transport_failure_notifies_probe_wake`: arrange a real monitor at interval `60_000` ms, prompt live health responder, failing send, and dead-process refresh endpoint; invoke producer, await its recorded refresh failure → assert health probe within 500 ms after that observation, Ready, zero replacements, and bounded producer completion. Command: `rc_cargo t25 test -p camel-component-jms --lib producer_transport_failure_notifies_probe_wake`. Expected: RED with direct restart or absent observation; GREEN afterward.
- `consumer_subscribe_retry_notifies_probe_wake`: arrange real consumer, always-unavailable subscribe responder, real process with dead refresh endpoint, live health responder and actual monitor interval `60_000` ms; await subscribe retry and actual failed-refresh observation → assert probe within the following 500 ms, Ready, zero replacements, and cancellation within five seconds. Bound refresh observation separately by three seconds. Command: `rc_cargo t25 test -p camel-component-jms --lib consumer_subscribe_retry_notifies_probe_wake`. Expected: RED without real failed-target evidence; GREEN afterward.
- `consumer_channel_refresh_retry_notifies_probe_wake`: arrange gated consumer reconnects, initial actual dead-process endpoint, real pool/monitor, and a second distinct dead process; observe first failed refresh, install second process before releasing next retry → assert second observation names that endpoint and failure, probe within 500 ms after it, Ready, zero replacements, cancellation within five seconds. This covers subscribe-side reconnect recovery, not the unreachable natural stream-error branch. Command: `rc_cargo t25 test -p camel-component-jms --lib consumer_channel_refresh_retry_notifies_probe_wake`. Expected: RED with process-less shortcut or inferred-only target; GREEN afterward.
- `stream_site_recovery_helper_notifies_probe_wake`: arrange real pool/monitor at interval `60_000` ms, prompt health, cancellation tokens and actual dead-process endpoint; call `refresh_channel_or_request_probe` directly → assert `Continue`, actual failed endpoint recorded within three seconds, probe within 500 ms after failure, Ready, zero replacements. Command: `rc_cargo t25 test -p camel-component-jms --lib stream_site_recovery_helper_notifies_probe_wake`. Expected: RED before helper exists; GREEN afterward. Coverage is direct helper behavior plus static wiring, not natural branch execution.
- `stream_site_recovery_helper_returns_cancelled`: gate a selected connector future after real endpoint selection, call helper then cancel; assert `Cancelled` within five seconds, no wake/probe/replacement request, and connector future dropped. Command: `rc_cargo t25 test -p camel-component-jms --lib stream_site_recovery_helper_returns_cancelled`. Expected: RED before helper exists; GREEN afterward.
- `slow_progressing_connector_has_fixed_budget_and_never_restarts_live_bridge`: arrange real slot process, actual `refresh_slot_channel`, helper and monitor, timeout/threshold from pool config, and a test connector future that signals progress every 25 ms for up to five seconds without completing; start recovery and record a monotonic start → assert refresh expires on the single two-second production deadline within one second scheduling margin despite multiple progress signals, timeout observation names actual selected endpoint, helper returns and a live health probe is processed within 500 ms after expiration, Ready and zero attempts throughout. Connector is cancelled, not permitted to finish its five-second progress loop. Repeat with a promptly succeeding connector to show progress/completion within budget succeeds. Command: `rc_cargo t25 test -p camel-component-jms --lib slow_progressing_connector_has_fixed_budget_and_never_restarts_live_bridge`. Expected: RED if progress renews deadline or no total bound exists; GREEN afterward. Together with unchanged real-monitor T6 broker-down/stale tests this pins R1: slow broker work alone never restarts a live bridge.
- `counter_semantics_preserved_by_recovery_helper`: source comparison of both consumer counter regions against `98cd8972`, excluding extracted recovery-action text and the explanatory comment; assert identical initialization, increments, both threshold-two checks, and resets. Run unchanged `retry_loop_invokes_operation_exactly_max_attempts_times`, `reconnect_exhaustion_is_surfaced`, `transient_error_still_reconnects`, and `t2_monitor_` test bodies. Command: `rc_cargo t25-counter test -p camel-component-jms --lib` plus `git diff 98cd8972 -- crates/components/camel-jms/src/consumer.rs`. Expected: existing tests remain GREEN without editing their bodies; source comparison fails any moved/new reset or threshold change. This is a handoff/reviewer inspection, not a `#[test]` function; no Cargo test may run `git` or read commit `98cd8972`. Evidence: the `git diff 98cd8972 -- crates/components/camel-jms/src/consumer.rs` excerpt for both counter regions in the handoff, plus the unchanged named tests passing.
- `terminal_wake_skips_probe_and_stays_terminal`: for both `Parked` and `Stopped` slots, call `pool.restart_slot("default")` and also deliver a wake while a probe is in flight → assert the warning path returns, `probe_wake` does not produce a probe, the state remains terminal, and no restart occurs. Command: `rc_cargo t25 test -p camel-component-jms --lib terminal_wake_skips_probe_and_stays_terminal`. Expected: RED before Step 4 if a wake bypasses the terminal re-read; GREEN after Step 4.
- `wake_during_inflight_probe_does_not_start_overlapping_probe`: gate the first probe open, call `restart_slot` while it is in flight, release the gate → assert the probe invocation count increments by one for the wake only after the first completes, never concurrently, and the failure counter stays `0`. Command: `rc_cargo t25 test -p camel-component-jms --lib wake_during_inflight_probe_does_not_start_overlapping_probe`. Expected: RED before Steps 3-4; GREEN after Steps 3-4.
- `terminal_restart_requests_still_do_nothing`: reuse `parked_slot_is_not_respawned` and `restart_slot_does_not_overwrite_stopped` → assert `restart_slot` leaves `Parked` and `Stopped` unchanged. Command: `rc_cargo t25 test -p camel-component-jms --lib restart_slot` and `rc_cargo t25-park test -p camel-component-jms --lib parked_slot_is_not_respawned`. Expected: RED before Step 4 if wake changes terminal state; GREEN after Step 4.

**Acceptance:**
- All listed test groups pass.
- Static wiring: consumer production source has exactly one `pool.restart_slot(broker_name)` inside the helper and two helper call sites. Count AST-equivalent invocations or restrict textual counts to production source before `#[cfg(test)]`; helper tests must not inflate the count. Verify the invariant comment is adjacent to the unreachable natural branch.
- Existing counter/threshold test bodies are unchanged from `98cd8972` and all pass. No new counter reset points, thresholds, or artificial counter-seeding seams exist.
- Actual endpoint/failure observations originate inside `refresh_slot_channel` from each real caller; no independent follow-up refresh is accepted as evidence. Two-second refresh and subsequent 500 ms wake checks use fixed deadlines, never progress-renewed deadlines.
- `rg -n "BridgeState::Restarting" crates/components/camel-jms/src/component.rs` shows no occurrence inside `restart_slot`.
- Every `BridgeSlot` literal contains `probe_wake`; `rg -n "send_modify" crates/components/camel-jms/src/component.rs` shows no fake watch bump in `restart_slot`.
- `rc_cargo t25-clippy clippy -p camel-component-jms --all-targets -- -D warnings` exits 0.

- [x] 2.5

#### Task 2.6: Publish the scoped JSON Schema and register its executable checks

**Files:**
- `schemas/jms-pool-health-config.json` (new)
- `crates/components/camel-jms/Cargo.toml` (modified)
- `crates/components/camel-jms/tests/jms_pool_health_schema.rs` (new)
- `scripts/xtask/Cargo.toml` (modified)
- `scripts/xtask/src/main.rs` (modified)
- `Cargo.lock` (modified by Cargo)

**Steps:**
1. Write `schemas/jms-pool-health-config.json` as a draft 2020-12 object schema with exactly these four properties and no others: `health_timeout_ms` integer minimum `1` maximum `18446744073709551615` default `3000`; `health_failure_threshold` integer minimum `1` maximum `4294967295` default `3`; `health_broker_probe_mode` string enum `["reuse","skip"]` default `"reuse"`; `health_check_interval_ms` integer minimum `1` default `5000`. Set `additionalProperties: true` and state in `description` that this is a scoped health fragment, not a complete pool validator, and that other pool properties keep runtime validation.
2. Add `jsonschema = { workspace = true }` under camel-jms `[dev-dependencies]` for schema tests only, never its production dependencies. This task owns necessary manifest/lock changes. Do not add timing dependencies, duplicate versions, or unrelated lock churn.
3. Add `crates/components/camel-jms/tests/jms_pool_health_schema.rs` that loads the schema with `jsonschema::validator_for`, constructs JSON objects with `serde_json::json!`, and compares schema defaults, minimums, maximums, and enum with `JmsPoolConfig::default()` and `JmsPoolConfig::validate()`.
4. Add `jsonschema = { workspace = true }` to `scripts/xtask/Cargo.toml`.
5. In `scripts/xtask/src/main.rs`, add `fn validate_handwritten_schemas(workspace_root: &Path) -> Result<(), String>` that reads `schemas/jms-pool-health-config.json`, compiles it with `jsonschema::validator_for`, requires the four property names and `additionalProperties == true`, and executes validation against known-invalid instances for each property. It must not stop at `serde_json::from_str`: a parse-only check does not prove the schema rejects bad values.
6. Call `validate_handwritten_schemas` from `run_schema_generation` in the `check` branch only, before the drift verdict, and append its error to the returned message.
7. Add xtask unit tests for the helper path using a temp file with malformed JSON and a temp file with a missing property; assert both return `Err`, and assert a valid schema passes the executable validation path.
8. Run the targeted tests once so Cargo updates `Cargo.lock`; commit the lockfile change with the manifest change and no unrelated lock churn. Manifest edits trigger the conditional `feature_profiles` gate.

**Tests:**
- `schema_compiles_and_is_scoped_to_four_health_properties`: load the schema → compile with `validator_for` → assert the property key set equals `health_timeout_ms`, `health_failure_threshold`, `health_broker_probe_mode`, `health_check_interval_ms`, and `additionalProperties` is `true`. Command: `rc_cargo t26-schema test -p camel-component-jms --test jms_pool_health_schema`. Expected: RED before Step 1 because the file is absent; GREEN after Step 1.
- `schema_defaults_minimums_enum_and_runtime_agree`: read defaults, minimums, maxima, and the enum from the schema and compare them with `JmsPoolConfig::default()` and `JmsPoolConfig::validate()` → assert `3000`, `3`, `"reuse"`, `5000`, minimum `1`, `u64::MAX`, and `u32::MAX` all match the runtime contract. Command as above. Expected: RED before Steps 1-3 because one side is absent; GREEN after Steps 1-3.
- `zero_values_wrong_types_and_overflow_are_rejected`: validate zero, string-valued numeric fields, `health_timeout_ms=18446744073709551616`, and `health_failure_threshold=4294967296`; parse overflow JSON text rather than oversized Rust literals. Assert schema rejection and relevant property in each error's `instance_path`, not Display. Assert zero cases fail runtime `validate()` with named property. Assert wrong types/overflows return serde `Err` without requiring field names in serde_json messages. Command as above. Expected: RED before schema/runtime checks; GREEN afterward.
- `omitted_health_keys_validate_and_deserialize_to_defaults`: instance `{"brokers":{}}` validates against the schema and deserializes into `JmsPoolConfig` → assert the three health defaults. Command as above. Expected: RED before Step 3; GREEN after Step 3.
- `unknown_probe_mode_is_rejected_by_schema_and_serde`: instance `{"health_broker_probe_mode":"probe"}` → assert schema rejection with `instance_path` identifying `health_broker_probe_mode` and serde_json deserialization returns `Err`; do not require field naming from serde_json error Display. The Task 2.1 TOML test separately covers named-key diagnostics. Command as above. Expected: RED before schema/runtime checks; GREEN afterward.
- `unknown_other_pool_key_is_permitted`: instance `{"max_bridges":4,"brokers":{},"future_pool_key":true}` validates, confirming the schema is a scoped fragment and not a complete pool validator. Command as above. Expected: RED before Step 1; GREEN after Step 1.
- `xtask_schema_check_executes_handwritten_validation`: run `rc_cargo t26-xtask xtask schema --check` → assert exit 0. Companion unit tests `rejects_malformed_handwritten_schema` and `rejects_handwritten_schema_missing_property`: write malformed JSON and a schema missing a required property to two temp paths and call the helper → assert both return `Err`. Command: `rc_cargo t26-xtask-test test -p xtask rejects_malformed_handwritten_schema` and `rc_cargo t26-xtask-test test -p xtask rejects_handwritten_schema_missing_property`. Expected: RED before Steps 5-6 because the helper and call are absent; GREEN after Steps 5-6.

**Acceptance:**
- `rc_cargo t26-schema test -p camel-component-jms --test jms_pool_health_schema` passes.
- `rc_cargo t26-xtask xtask schema --check` exits 0.
- `rc_cargo t26-xtask-test test -p xtask rejects_malformed_handwritten_schema` and `rc_cargo t26-xtask-test test -p xtask rejects_handwritten_schema_missing_property` pass.
- `rc_cargo gate-feature-profiles test -p camel-cli --test feature_profiles` passes without regenerating the golden.
- `Cargo.lock` contains one `jsonschema` package reused by `camel-component-jms` and `xtask`, with no duplicate package entries and no unrelated churn from Task 2.1.

- [x] 2.6

#### Task 2.7: Record ADR-0086 and its context pointer

**Files:**
- `docs/adr/0086-jms-liveness-and-broker-health.md` (new)
- `CONTEXT-MAP.md` (modified)

**Steps:**
1. Verify allocation with `test ! -e docs/adr/0086-jms-liveness-and-broker-health.md`. If it exists, stop and report the allocation conflict instead of overwriting.
2. Create the ADR with sections Status (`Accepted`, 2026-10-20), Context, Decision, Consequences, Evidence and limits, and References. Do not create any new checkpoint file; the ADR and the docs named in Task 2.8 are the only planned documentation additions.
3. Decision: a completed health RPC proves liveness regardless of `healthy` and `brokerConnected`; only consecutive failed liveness probes restart; broker status is a cached snapshot with a 10 s TTL and refresh after 5 s; `reuse` retains one provider diagnostic and `skip` performs no broker I/O; readiness distinguishes live-unverified (`Degraded`, 200) from verified-down (`Unhealthy`, 503), where a healthy RPC reporting `healthy=false` maps to `Unhealthy` 503 rather than waiting for an RPC failure.
4. Consequences: record the default worst-case dead-process detection as threshold times interval plus timeout (`3 x (5 s + 3 s)`, about 24 s) and state that slower dead-process detection is accepted to remove broker-latency restarts; the bridge version and protobuf stay unchanged.
5. Evidence and limits: add an explicit `Accepted limitation` subheading. State that T6 is paired-contract coverage, not a live-broker or native-image soak; the Java fake-broker test proves prompt health calls over three periods with flat opens, the Rust real monitor proves zero replacements over three periods for both stale and verified-down responses, and native/live-broker evidence remains owned by CI (`CI owner`, `CI-owned`). State that the eventual park remains `not_reproduced` and its `next_steps` are unchanged.
6. Keep operational execution evidence out of the normative ADR. Require the final park report to record the containment deviation: the initial hash helper compiled xtask outside the required systemd scope inside the feature worktree, never main; exit 0; post-run process count 0; resource peak unmeasured, so no claim of zero impact. Also require `not_reproduced` and `next_steps` to identify paired T6 limits and CI ownership of native/live-broker evidence.
7. References: link ADR-0036 for IPC security, ADR-0052 for diagnostics, and CONTEXT-MAP's Degraded/Unhealthy readiness rule. State that JMS health fields do not control data-plane admission or settlement.
8. Add one ADR-0086 bullet to the ADR list in `CONTEXT-MAP.md` with a one-line summary matching the ADR title.

**Tests:**
- `adr_0086_exists_and_number_is_unique`: command `test -f docs/adr/0086-jms-liveness-and-broker-health.md` and `test "$(find docs/adr -maxdepth 1 -name '0086-*' | wc -l)" = "1"` → both exit 0. Expected: RED before Step 2; GREEN after Step 2.
- `adr_records_paired_contract_limit_and_ci_owner`: command `rg -n "Accepted limitation|paired[- ]contract|not a live-broker or native-image soak|CI-owned|not_reproduced|next_steps" docs/adr/0086-jms-liveness-and-broker-health.md` → assert at least one hit for each of the six phrases. Expected: RED before Step 5; GREEN after Step 5.
- `adr_maps_verified_down_to_unhealthy_503`: command `rg -n "verified-down|Unhealthy|503|Degraded|200" docs/adr/0086-jms-liveness-and-broker-health.md` → assert the readiness mapping names `verified-down` with `Unhealthy` `503` and live-unverified with `Degraded` `200`, not an RPC-failure-only rule. Expected: RED before Step 3; GREEN after Step 3.
- `park_preserves_evidence_limits_and_containment_deviation`: final-response inspection → assert `not_reproduced` names live-broker/native evidence absent, `next_steps` assigns it to CI, and operational deviation names the uncontained worktree-only xtask build, exit 0, post-run process count 0, and unmeasured resource peak. Expected: incomplete until final park; pass only when all fields are present. This is a reporting criterion, not an implementation test.
- `context_map_links_adr_0086`: command `rg -n "0086-jms-liveness-and-broker-health" CONTEXT-MAP.md` → assert exactly one hit. Expected: RED before Step 8; GREEN after Step 8.

**Acceptance:**
- All five commands exit 0.
- The ADR contains no credential-bearing example and references ADR-0036, ADR-0052, and CONTEXT-MAP.
- `rc_cargo t27-context xtask lint-context-citations` exits 0.
- No checkpoint file was created.

- [x] 2.7

#### Task 2.8: Document the operator contract in the existing JMS and health docs

**Files:**
- `crates/components/camel-jms/README.md` (modified)
- `bridges/jms/README.md` (modified)
- `docs/src/components/jms.md` (modified)
- `docs/src/operations/health.md` (modified)

**Steps:**
1. In `crates/components/camel-jms/README.md`, add a `Pool health settings` table for `health_timeout_ms` (3000), `health_failure_threshold` (3), `health_broker_probe_mode` (`reuse`/`skip`), and `health_check_interval_ms` (5000, unchanged). State that only consecutive probe failures restart, a successful RPC resets the count even when it reports a broker problem, and the default worst-case dead-process detection is about 24 s.
2. In `bridges/jms/README.md`, add `JMS_HEALTH_BROKER_PROBE_MODE` to the environment table with default `reuse` and values `reuse`/`skip`. Explain the retained diagnostic, the 10 s TTL, refresh after 5 s, stale/unknown status, and that unsupported values abort startup.
3. In `docs/src/components/jms.md`, add a `Health and broker status` section with the five-row response table: verified up (Healthy 200), verified down (Unhealthy 503), not yet checked (Degraded 200), skipped (Degraded 200), stale (Degraded 200). State explicitly that a verified-down broker response maps to `Unhealthy` 503 and is not merely a message attached to a healthy response; an RPC error or timeout is also `Unhealthy` 503. Keep the timeout-versus-interval distinction.
4. In `docs/src/operations/health.md`, extend the Degraded vs Unhealthy section with JMS-specific text: a live bridge with unknown, skipped, or stale broker status is `Degraded` and passes readiness; a healthy RPC that reports verified broker down is `Unhealthy` 503; an RPC failure or timeout is also `Unhealthy` 503; broker fields never trigger a process restart.
5. In both JMS docs, state the version-compatibility limit: the broker-latency isolation guarantee requires the updated Java health handler, and no protobuf change is needed; an older blocking bridge cannot provide the guarantee.
6. Keep all prose English and do not add a new `CONTEXT.md` file.

**Tests:**
- `readmes_and_docs_contain_all_keys`: command `rg -n "health_timeout_ms|health_failure_threshold|health_broker_probe_mode" crates/components/camel-jms/README.md docs/src/components/jms.md` and `rg -n "JMS_HEALTH_BROKER_PROBE_MODE" bridges/jms/README.md` → assert every command exits 0. Expected: RED before Steps 1-2; GREEN after Steps 1-2.
- `docs_table_maps_verified_down_to_unhealthy_503`: command `rg -n "verified down|Unhealthy|503" docs/src/components/jms.md docs/src/operations/health.md` → assert each of the three terms has a hit and the verified-down row pairs `Unhealthy` with `503`. Expected: RED before Steps 3-4; GREEN after Steps 3-4.
- `docs_contain_readiness_and_stale_contract`: command `rg -n "Degraded|Unhealthy|stale|skipped|not yet checked" docs/src/components/jms.md docs/src/operations/health.md` → assert at least one hit for every term across the pair. Expected: RED before Steps 3-4; GREEN after Steps 3-4.
- `docs_state_older_bridge_cannot_isolate`: command `rg -n "older|updated Java health handler|no protobuf change|isolation guarantee" docs/src/components/jms.md` → assert the compatibility paragraph is present. Expected: RED before Step 5; GREEN after Step 5.
- `docs_build_after_edit`: command `mdbook test` with tool workdir set to the worktree's `docs` directory → assert exit 0; then the contained `RUSTDOCFLAGS="-D warnings" rc_cargo t28-doc doc -p camel-api -p camel-core -p camel-builder -p camel-dsl -p camel-endpoint --no-deps` in the worktree root → assert exit 0. Expected: RED if a code block is malformed; GREEN after docs edits.

**Acceptance:**
- The three `rg` commands exit 0.
- `mdbook test` exits 0.
- `rc_cargo t28-context xtask lint-context-citations` exits 0.
- No `CONTEXT.md` file is created; the pointer lives in `CONTEXT-MAP.md` from Task 2.7.

- [x] 2.8
