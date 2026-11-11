# Tasks: native-evictor-verification

Single-phase change (design.md has no `## Phases` section). Tasks run in
order; each builds on the previous. All Java files live in the test sourceset
of `bridges/jms` — no production source changes anywhere in this change.

## bridges/jms — stub layer

### Task 1.1: Hand-rolled JMS stub layer (broker-free, native-safe)

**Files:**
- `bridges/jms/src/test/java/org/rustcamel/jms/StubJmsConnection.java` (new)
- `bridges/jms/src/test/java/org/rustcamel/jms/StubJmsSession.java` (new)
- `bridges/jms/src/test/java/org/rustcamel/jms/StubJmsConnectionFactory.java` (new)
- `bridges/jms/src/test/java/org/rustcamel/jms/StubTransport.java` (new)
- `bridges/jms/src/test/java/org/rustcamel/jms/StubClassicConnection.java` (new)
- `bridges/jms/src/test/java/org/rustcamel/jms/StubClassicConnectionFactory.java` (new)

**Steps:**
1. Create `StubJmsConnection implements javax.jms.Connection`: package-private
   final class. Fields: `private final java.util.List<Long> closeTimes = new
   java.util.concurrent.CopyOnWriteArrayList<>();` and `private final
   java.util.concurrent.atomic.AtomicBoolean closed = new
   java.util.concurrent.atomic.AtomicBoolean(false);` with public read-only
   accessors `closeTimes()` and `isClosed()`. Implement the full JMS 2.0
   interface: `createSession()` / `createSession(int)` /
   `createSession(boolean, int)` return a new `StubJmsSession`;
   `getMetaData()` returns `null`; `getExceptionListener()` returns `null`;
   `setExceptionListener(...)` no-op; `start()` / `stop()` no-op;
   `setClientID(...)` no-op; `getClientID()` returns `null`; the
   `createDurableConnectionConsumer` and `createConnectionConsumer` variants
   return `null` no-ops; `close()`
   records `System.nanoTime()` into `closeTimes` and sets `closed`.
2. Create `StubJmsSession implements javax.jms.Session`: package-private final
   class implementing every method as a no-op returning `null` or `false`
   per its return type (`getTransacted()=false`, `getAcknowledgeMode()=0`,
   `commit()`/`rollback()`/`close()` no-ops, all factory methods return
   `null`), so an unexpected pooled-connection call never NPEs.
3. Create `StubJmsConnectionFactory implements javax.jms.ConnectionFactory`:
   package-private final class with `private final
   java.util.function.Supplier<javax.jms.Connection> connectionSupplier;`
   (constructor takes the supplier). `createConnection()` and
   `createConnection(String, String)` return `connectionSupplier.get()`.
   Implement all four `createContext` overloads explicitly returning `null`
   — the pinned javax.jms API declares them `abstract` (no defaults).
4. Create `StubTransport implements org.apache.activemq.transport.Transport`:
   package-private final class. Fields: `private volatile boolean stopped;`,
   `private final java.util.List<Long> stopTimes = new
   java.util.concurrent.CopyOnWriteArrayList<>();`, `private volatile
   org.apache.activemq.transport.TransportListener listener;` with public
   read-only accessors `isStopped()`, `stopTimes()`, `listener()`. Method
   behavior: `start()` no-op; `stop()` records `System.nanoTime()` into
   `stopTimes` and sets `stopped=true`; `oneway(Object)` no-op (return void);
   `asyncRequest` returns `null`; `request(Object)` / `request(Object, int)`
   return `null`; `getTransportListener()` returns `listener`;
   `setTransportListener(...)` stores it; `narrow(Class)` returns `null`;
   `getRemoteAddress()` returns `"stub://evictor-verifier"`;
   `isFaultTolerant()` returns `false`; `isDisposed()` returns `isStopped()`;
   `isConnected()` returns `!isStopped()`; `isReconnectSupported()` and
   `isUpdateURIsSupported()` return `false`; `reconnect(URI)` no-op;
   `updateURIs(boolean, URI[])` no-op; `getReceiveCounter()` returns `0`;
   `getPeerCertificates()` returns `null`; `setPeerCertificates(...)` no-op;
   `getWireFormat()` returns `null`. Rationale (do not deviate): the
   `ActiveMQConnection` protected constructor calls ONLY
   `isFaultTolerant()` and `setTransportListener(...)` (verified from
   activemq-client 5.19.10 bytecode), and `close()` ends in `stop()`.
5. Create `StubClassicConnection extends org.apache.activemq.ActiveMQConnection`:
   package-private final class with constructor
   `StubClassicConnection(StubTransport transport) throws Exception` calling
   `super(transport, new org.apache.activemq.util.IdGenerator(), new
   org.apache.activemq.util.IdGenerator(), new
   org.apache.activemq.management.JMSStatsImpl())`. Add accessor
   `StubTransport stubTransport()` returning the stored transport (keep the
   constructor argument in a field).
6. Create `StubClassicConnectionFactory extends
   org.apache.activemq.ActiveMQConnectionFactory`: package-private final class
   with constructor `StubClassicConnectionFactory()` calling
   `super("tcp://localhost:1")` (URL is parsed but never connected). Override
   `createConnection()` and `createConnection(String, String)` to return `new
   StubClassicConnection(new StubTransport())` — a fresh transport per
   connection so each physical connection's close is independently
   observable. Both overrides must NOT call `super.createConnection(...)`.
7. Run `cd bridges/jms && ./gradlew --offline compileTestJava` (with
   `JAVA_HOME=/nix/store/p7sk0r60bbn9ynrpbcjfjpvlj3f5jv9i-temurin-bin-21.0.11`
   first in PATH) and fix compile errors until clean.

**Tests:** (executable spec)
- `compileTestJava`: six stub classes exist as specified → run
  `./gradlew --offline compileTestJava` → exit code 0 (before this task:
  classes absent, task still passes trivially; after: sources compile — this
  check's value is that the full javax.jms and Transport surfaces are
  implemented correctly, which javac enforces exhaustively).
- Stub behavioral contracts exercised later by Task 1.3's verification run
  (each listed so the contracts are explicit): `StubJmsConnection.close()`
  appends exactly one nanoTime entry per call; `StubTransport.stop()`
  appends exactly one nanoTime entry per call;
  `StubClassicConnectionFactory.createConnection()` returns a
  `StubClassicConnection` whose `stubTransport()` is a fresh
  `StubTransport` per call; `StubClassicConnection` construction succeeds
  with no broker and `super` ctor touches only `isFaultTolerant` /
  `setTransportListener`.

**Acceptance:**
- `./gradlew --offline compileTestJava` exits 0.
- No production source file modified (`git status --short
  bridges/jms/src/main` empty).
- No new dependency in `build.gradle.kts`; no Mockito import in any new file.
- `./gradlew --offline spotlessCheck` exits 0 (googleJavaFormat).

- [x] 1.1

## bridges/jms — verifier

### Task 1.2: PoolEvictorVerifier main (both branches, fail-closed proofs)

**Files:**
- `bridges/jms/src/test/java/org/rustcamel/jms/PoolEvictorVerifier.java` (new)

**Steps:**
1. Create package-private final class `PoolEvictorVerifier` with
   `public static void main(String[] args)`. Structure: constants
   `IDLE_TIMEOUT_MS = 1000`, `CHECK_INTERVAL_MS = 1000`, `START_POLL_MS =
   10_000`, `EVICT_DEADLINE_MS = 15_000`, `IDLE_FLOOR_MS = IDLE_TIMEOUT_MS`,
   `EVICT_UPPER_MS = IDLE_TIMEOUT_MS + 2 * CHECK_INTERVAL_MS + 3000`,
   `STOP_BOUND_MS = 10_000`, `SHUTDOWN_POLL_MS = 10_000`,
   `GLOBAL_BUDGET_MS = 120_000`.
2. Watchdog: in `main`, first start a daemon thread that sleeps the effective
   budget — `GLOBAL_BUDGET_MS` unless the env override
   `POOL_EVICTOR_BUDGET_MS` is set (used only by the fault regressions in
   Task 1.3) — then triggers the diagnostic as BEST-EFFORT on a separate
   daemon thread (thread names, daemon flags and states from
   `Thread.getAllStackTraces()` only, plus volatile stub fields — never pool
   getters), waits a short fixed grace (1500 ms) independent of whether the
   printing finished (a blocked or synchronized `PrintStream` must not
   prevent termination), and calls `Runtime.getRuntime().halt(2)`. The
   watchdog stays ARMED until process death: no defuse, no cancellation.
   `System.exit` invokes shutdown hooks before halting, so a blocked hook
   could defeat a normal exit — the armed watchdog is what guarantees
   unconditional termination.
3. Implement a bounded-run helper `<T> T bounded(String what, long boundMs,
   java.util.concurrent.Callable<T> call)`: runs `call` on a single-thread
   `ExecutorService`, `future.get(boundMs, MILLISECONDS)`; on
   `TimeoutException` prints the same lock-free diagnostic plus the step name
   and bound, and throws `IllegalStateException("UNBOUNDED: " + what)`; on
   execution failure rethrows the cause. Shut down the executor after each
   call. Void operations pass null-returning lambdas —
   `bounded("check-in", STOP_BOUND_MS, () -> { borrowed.close(); return
   null; })` — never method references to void methods (they do not satisfy
   `Callable<T>`); apply consistently to check-in and `stop()`.
4. Implement an evictor-thread scanner `threadNamesContaining(String needle)`
   returning lowercase-matched names from `Thread.getAllStackTraces()`.
5. Implement `int verifyClassic()` returning 0 on success, 1 on failure with
   diagnostics printed. Sequence — mirror the production setter sequence from
   `JmsClientFactory.buildPooledActiveMqFactory` (knobs before `start()`,
   `setMaxConnections(5)`):
   `StubClassicConnectionFactory delegate = new StubClassicConnectionFactory();
   org.apache.activemq.pool.PooledConnectionFactory pool = new
   org.apache.activemq.pool.PooledConnectionFactory(delegate);` record
   `preStartNanos = System.nanoTime();` then
   `bounded("pool start", STOP_BOUND_MS, () -> { pool.setMaxConnections(5);
   pool.setIdleTimeout(IDLE_TIMEOUT_MS);
   pool.setTimeBetweenExpirationCheckMillis(CHECK_INTERVAL_MS); pool.start();
   return null; });` — the pre-start origin is the idle origin for the
   startup-retained connection (Classic checks it in before `start()`
   returns; timestamping after `start()` would understate its idle time and
   falsely violate the floor under caller descheduling). Then: (a) START —
   poll (250 ms interval) inside `bounded(...,
   START_POLL_MS)` until `threadNamesContaining("commons-pool-evictor")` is
   non-empty; empty at deadline → print thread dump + fail. (b) RETAINED —
   `javax.jms.Connection borrowed = bounded("borrow",
   STOP_BOUND_MS, pool::createConnection);` record `idleOriginNanos =
   System.nanoTime();` then `bounded("check-in", STOP_BOUND_MS,
   () -> { borrowed.close(); return null; });` then read
   `n = pool.getNumConnections();` assert
   `n >= 1` (observed baseline; Classic is expected to report 2:
   `createConnectionOnStartup=true` retains one connection at `start()` plus
   the explicit borrow — record the exact N in the output line
   `BASE branch=classic retained=N`). (c) EVICT — poll inside
   `bounded(..., EVICT_DEADLINE_MS)` (250 ms interval) until
   `pool.getNumConnections() == 0`; record observation time; then assert
   every recorded `StubTransport.stopTimes()` entry minus the owning
   connection's idle origin satisfies `elapsed >= IDLE_FLOOR_MS` (event
   correlation against poll descheduling). Idle origins: the borrowed
   connection's origin is the recorded `idleOriginNanos` (captured before
   check-in); the `createConnectionOnStartup` baseline connection's origin is
   `preStartNanos` (captured before `pool.start()` — both origins sit at the
   pre-event boundary so elapsed time overstates true idle by at most call
   overhead, never understates it). Track connection→transport identity via the
   delegate-supplied `StubClassicConnection.stubTransport()` references kept
   in a list; assert observed eviction time ≤ `EVICT_UPPER_MS` from the
   latest idle origin; assert total stop records == n (all baseline
   connections physically closed). (d) SHUTDOWN — `bounded("stop", ...)` run
   `pool.stop()`; poll ≤ SHUTDOWN_POLL_MS until
   `threadNamesContaining("commons-pool-evictor")` is empty AND
   `pool.getNumConnections() == 0`; on success print exactly one line
   `PASS branch=classic proofs=START,RETAINED,EVICT,SHUTDOWN`. Every failure
   path prints `FAIL branch=classic <proof> <detail>` plus the lock-free
   diagnostic (pool count via `pool.getNumConnections()` only on the
   verifier's own thread — never from the watchdog — stub close/stop
   records, thread dump) and returns 1.
6. Implement `int verifyArtemis()` with the same proof script, mirroring
   `JmsClientFactory.buildPooledArtemisFactory`: `StubJmsConnectionFactory
   delegate = new StubJmsConnectionFactory(supplier)` where the supplier
   creates a `StubJmsConnection` AND appends the instance to a
   `java.util.List<StubJmsConnection>` held by the verifier (the pool holds
   the same instances, so their `closeTimes()` are the physical-close
   evidence);
   `org.messaginghub.pooled.jms.JmsPoolConnectionFactory pool = new
   org.messaginghub.pooled.jms.JmsPoolConnectionFactory();` record
   `preStartNanos` the same way, then
   `bounded("pool start", STOP_BOUND_MS, () -> { pool.setConnectionFactory(
   delegate); pool.setMaxConnections(5);
   pool.setConnectionIdleTimeout(IDLE_TIMEOUT_MS);
   pool.setConnectionCheckInterval(CHECK_INTERVAL_MS); pool.start();
   return null; });`
   Physical-close evidence = `StubJmsConnection.closeTimes()` of the
   connections recorded by the supplier (keep the list; the pool holds the
   same instances). PASS line `PASS branch=artemis proofs=...`; same
   `BASE branch=artemis retained=N` line.
7. Fault hooks for the Task 1.3 regressions, keyed on env
   `POOL_EVICTOR_FAULT` (absent in production runs; no default behavior
   change). Each hook prints a fault-entry line on activation (e.g.
   `FAULT entered block-checkin`) so the regressions can prove the fault was
   actually reached:
   `block-checkin` — the VERIFIER's own Classic check-in step blocks forever
   (wait on a monitor) instead of calling `borrowed.close()` (the fault is
   verifier-side, not in the stub: `StubJmsConnection` is the Artemis stub,
   and the Classic borrowed connection is a pooled wrapper the stub cannot
   block);
   `block-hook` — `main` registers a shutdown hook that blocks forever and
   prints `FAULT entered block-hook` BEFORE running the branches;
   `fail-artemis` — `verifyArtemis` prints its `FAIL branch=artemis`
   diagnostic and returns 1 immediately after the START proof;
   `skip-artemis` — `verifyArtemis` returns 0 WITHOUT printing its PASS line
   (prints `FAULT entered skip-artemis`) — simulates an undiscovered
   verification for the shared-validator rejection test in Task 1.3. This
   hook is test-only; the real `verifyArtemis` always prints PASS or FAIL.
8. `main` runs `verifyClassic()`, then `verifyArtemis()`, collects return
   codes, prints `PASS both` only if both returned 0, and exits with
   `codes.sum() == 0 ? 0 : 1` via `System.exit`.
9. Run `cd bridges/jms && ./gradlew --offline compileTestJava` and fix
   compile errors until clean.

**Tests:** (executable spec)
- `compileTestJava`: verifier class exists with `public static void main` →
  `./gradlew --offline compileTestJava` → exit 0.
- `verifyClassic JVM run` (executed by Task 1.3's `evictorVerifyJvm`):
  stub layer present + verifier built → run → exit 0, stdout contains
  `BASE branch=classic retained=N` with `N >= 1`,
  `PASS branch=classic proofs=START,RETAINED,EVICT,SHUTDOWN`.
- `verifyArtemis JVM run` (executed by Task 1.3's `evictorVerifyJvm`):
  stub layer present + verifier built → run → exit 0, stdout contains
  `BASE branch=artemis retained=N` with `N >= 1`,
  `PASS branch=artemis proofs=START,RETAINED,EVICT,SHUTDOWN`.
- `watchdog halt` and fault regressions: executed by Task 1.3's
  `evictorVerifierFaults` task (executable subprocess runs with fault env
  vars — see Task 1.3 Tests). Structure enforced here: the watchdog body
  must contain no `getNumConnections()` call and must call
  `Runtime.getRuntime().halt(2)`.

**Acceptance:**
- `./gradlew --offline compileTestJava` exits 0.
- Watchdog body contains no `getNumConnections()` call (code review — the
  watchdog uses `Thread.getAllStackTraces()` and stub fields only; total
  `getNumConnections` call sites live in `verifyClassic`/`verifyArtemis`
  only).
- `grep -c "Runtime.getRuntime().halt(2)"` ≥ 1 and
  `grep -c "System.exit" ` shows exactly one intentional exit at the end of
  `main` (no other exit paths besides the watchdog halt).
- No `Assumption`, `@Disabled`, `@Tag`, or `continue-on-error` anywhere in
  the new sources.
- `./gradlew --offline spotlessCheck` exits 0.

- [x] 1.2

## bridges/jms — gradle wiring + local evidence

### Task 1.3: Gradle tasks and local JVM verification run

**Files:**
- `bridges/jms/build.gradle.kts` (modified)

**Steps:**
1. Add three tasks at the end of `build.gradle.kts`. First read
   `native-build-args.env` and extract the quoted value of
   `NATIVE_BUILD_ARGS_NEUTRAL` (same parse contract as
   `scripts/xtask/src/main.rs` `read_neutral_native_build_args`: strip
   surrounding double quotes; fail the build if the file is missing or the
   value is empty — fail-closed, no silent default).
2. `val evictorVerifierClass = "org.rustcamel.jms.PoolEvictorVerifier"` and a
   helper to compute the verifier classpath:
   `sourceSets.test.get().runtimeClasspath.asPath` (this includes main
   classes, test classes, and all runtime jars).
3. Task `evictorVerifyJvm` (`JavaExec`): `dependsOn(tasks.compileTestJava)`;
   `mainClass.set(evictorVerifierClass)`; `classpath =
   sourceSets.test.get().runtimeClasspath`; do NOT set any env overrides —
   the verifier is self-contained. JavaExec fails the task on non-zero exit
   (gradle default). Set `maxHeapSize = "1g"`.
4. Task `evictorVerifyNativeImage` — register it as
   `val evictorVerifyNativeImage = tasks.register("evictorVerifyNativeImage")`
   (KEEP the TaskProvider — `tasks.evictorVerifyNativeImage` is NOT a
   generated accessor for registered tasks; later tasks depend on the
   provider), `dependsOn(tasks.compileTestJava)`. In `doLast`:
   mkdir `build/evictor-verify`; resolve the launcher exactly as Quarkus
   does — `native-image.cmd` when
   `org.gradle.internal.os.OperatingSystem.current().isWindows`, else
   `native-image` (Windows CreateProcess cannot launch a bare script name);
   build the argument list `--no-fallback -Ob <neutral args split on commas>
   -cp <verifier classpath> <evictorVerifierClass> evictor-verifier`; write it
   ONE TOKEN PER LINE into `build/evictor-verify/native-image.args` — the
   `-cp` value is a single quoted token per native-image argfile rules, with
   backslashes and double quotes ESCAPED when written (GraalVM's argfile
   parser interprets backslash escapes inside quotes, so raw Windows
   `C:\Users\...` paths would corrupt — escape them; path separators may
   equivalently be normalized to `/` for the classpath, which Java accepts
   on Windows). To guard the escaping, the task performs an executable
   round-trip check: re-read the argfile, split it the way the launcher
   will (newline-separated tokens, quotes stripped, escapes decoded), and
   assert the decoded `-cp` value equals the original classpath string
   exactly — covering entries with spaces and Windows separators; mismatch
   fails the task. Invoke `<launcher>
   @native-image.args` (relative to the exec workingDir — the argfile lives
   in `build/evictor-verify`; an absolute path also works, but the
   project-relative `@build/evictor-verify/...` form would re-resolve
   beneath the changed workingDir and fail) with `workingDir =
   build/evictor-verify` (so the emitted image lands at
   `build/evictor-verify/evictor-verifier`, where step 5 expects it) via
   `providers.exec` — consume its lazy result and assert successful
   exit; do NOT use
   `project.exec` (removed in Gradle 9) and do NOT pass the classpath inline
   — the offline classpath is ~265 entries / ~42,700 characters, beyond the
   Windows command-line limit, which is why the argfile exists. The neutral
   args MUST come from step 1; `--no-fallback` is REQUIRED and
   hard-coded — a fallback image would run on the JVM and fake the proof.
   Split the comma-separated neutral args value correctly: it is a single
   token stream today (`-H:+AllowVMInspection`).
5. Task `evictorVerifyNative` — register as
   `val evictorVerifyNative = tasks.register("evictorVerifyNative")`,
   `dependsOn(evictorVerifyNativeImage)` (the TaskProvider from step 4).
   `doLast` runs the produced binary `build/evictor-verify/evictor-verifier`
   (append `.exe` when `org.gradle.internal.os.OperatingSystem.current().isWindows`)
   via `providers.exec` with output capture, then calls the SHARED
   `validateVerifierOutput` (defined in step 6) on the exit code and stdout,
   failing the task with the validator's rejection reason echoed into the
   gradle log.
6. Task `evictorVerifierFaults` — register, `dependsOn(tasks.compileTestJava)`.
   First define ONE shared output validator, e.g.
   `fun validateVerifierOutput(exitCode: Int, stdout: String)` in the build
   script, encoding the CI gate: PASS only when `exitCode == 0` AND stdout
   contains BOTH full branch PASS lines; it returns a rejection reason
   string otherwise (nonzero exit, or either branch PASS line missing).
   `evictorVerifyNative` (step 5) MUST call this same validator — the CI
   gate and the fault regressions then exercise identical rejection logic.
   In `doLast`, run five subprocess cases with `java`
   (`<java from JAVA_HOME or PATH> org.rustcamel.jms.PoolEvictorVerifier`)
   launched via `java.lang.ProcessBuilder` — NOT gradle exec, because these
   cases EXPECT nonzero exits and a deadline kill: pass the oversized
   classpath through the `CLASSPATH` environment variable (env vars have no
   command-line limit and avoid argfile quoting pitfalls), set the case's
   env vars, drain stdout and stderr on separate daemon threads (a full
   pipe buffer would deadlock the child), `waitFor(deadline, SECONDS)` with
   `destroyForcibly()` on timeout, and assert each case:
   (a) env `POOL_EVICTOR_FAULT=block-checkin POOL_EVICTOR_BUDGET_MS=3000` →
   exits 2 (watchdog halt) within 60 s, stdout contains
   `FAULT entered block-checkin` and a watchdog diagnostic, and NO `PASS both`;
   (b) env `POOL_EVICTOR_FAULT=block-hook POOL_EVICTOR_BUDGET_MS=15000` →
   exits 2 within 60 s (halt beats the blocked shutdown hook), stdout
   contains `FAULT entered block-hook`;
   (c) env `POOL_EVICTOR_FAULT=fail-artemis` → exits 1, stdout contains
   `FAIL branch=artemis` and NO `PASS both`;
   (d) env `POOL_EVICTOR_FAULT=skip-artemis` → exits 0 with only the classic
   PASS line, and `validateVerifierOutput` REJECTS this output (missing
   artemis PASS) — proving the shared validator catches an undiscovered
   verification that exits 0;
   (e) replay case (c)'s captured output through `validateVerifierOutput` →
   REJECTED (nonzero exit) — proving the validator rejects nonzero exits
   even when a PASS line is present.
   Cases (a)/(b) prove the watchdog, (c)/(d)/(e) prove the shared gate
   rejects exactly the failing-verifier outputs it will see in CI. Any case
   not matching its requirement fails the task.
7. Run locally, in a transient systemd scope per governor:
   `systemd-run --user --scope --collect --unit=fleet-nativeredis -p
   MemoryMax=6G -p TasksMax=512 -- env TMPDIR=/home/shared/tmp
   JAVA_HOME=/nix/store/p7sk0r60bbn9ynrpbcjfjpvlj3f5jv9i-temurin-bin-21.0.11
   PATH=/nix/store/p7sk0r60bbn9ynrpbcjfjpvlj3f5jv9i-temurin-bin-21.0.11/bin:$PATH
   ./gradlew --offline evictorVerifyJvm evictorVerifierFaults` (from
   `bridges/jms`), then
   `systemctl --user stop fleet-nativeredis.scope`. Iterate until exit 0 with
   both PASS lines. Capture the full output as local evidence.
8. If the JVM run reveals stub-construction defects (e.g. an
   `ActiveMQConnection` constructor or close-path call not covered by
   `StubTransport`), fix the stub classes — those are test-sourceset changes,
   in scope — and re-run until green. Do NOT touch production sources to make
   the verifier pass; if a fix seems to require one, STOP and report
   `blocked: production-source change required: <what>`.

**Tests:** (executable spec)
- `evictorVerifyJvm local run`: tasks 1.1 + 1.2 done →
  `./gradlew --offline evictorVerifyJvm` → exit 0, stdout contains both
  `BASE branch=... retained=N` lines (N ≥ 1 each) and both full
  `PASS branch=... proofs=START,RETAINED,EVICT,SHUTDOWN` lines (before
  Task 1.3: task does not exist, command fails).
- `evictorVerifierFaults local run`: `./gradlew --offline
  evictorVerifierFaults` → exit 0; case (a) exit 2 + `FAULT entered
  block-checkin` + watchdog diagnostic + no `PASS both`; case (b) exit 2
  (halt beats blocked hook); case (c) exit 1 + `FAIL branch=artemis` + no
  `PASS both`; case (d) exit 0 but shared validator REJECTS (missing artemis
  PASS); case (e) validator REJECTS replayed nonzero output (before
  Task 1.3: task does not exist).
- `fail-closed structure`: `grep -c "no-fallback"` on `build.gradle.kts` ≥ 1
  (the native image task cannot silently produce a fallback image),
  `grep -c "NATIVE_BUILD_ARGS_NEUTRAL"` ≥ 1 (args come from the single-source
  env file), `grep -c "native-image.args"` ≥ 1 (argfile, not inline
  classpath), and NO occurrence of `project.exec` in `build.gradle.kts`.

**Acceptance:**
- `./gradlew --offline evictorVerifyJvm evictorVerifierFaults` exits 0 with
  both PASS lines and all three fault cases matching their requirements; the
  captured output is recorded in the mission evidence file.
- `./gradlew --offline spotlessCheck` exits 0.
- `git status --short bridges/jms/src/main` still empty (no production
  changes).
- `pgrep -c compiled_artifa` returns 0 after the run; the systemd scope is
  stopped.

- [x] 1.3

## CI — workflow step + local gates

### Task 1.4: CI step in the native tests job + YAML structural gate

**Files:**
- `.github/workflows/bridge-jvm-tests.yml` (modified)

**Steps:**
1. In job `jms-bridge-native-tests`, immediately after the existing
   `Native build (bridges/jms)` step, add a new step:
   `name: Evictor verification (native image)`, `shell: bash`,
   `working-directory: bridges/jms`, run block:
   `./gradlew --offline evictorVerifyNative`. No `continue-on-error`, no
   `if:` condition on this step, no timeout override (the job-level
   `timeout-minutes: 60` applies; the verifier's own watchdog bounds the
   process).
2. Do NOT modify any other step, the job matrix, triggers, or any other
   workflow file.
3. Write and run the local YAML structural check (justified gate —
   actionlint/js-yaml/pyyaml unavailable offline; GitHub's own parse is the
   authoritative backstop on push): a small pure-Python script (run from
   /tmp, NOT committed) that (a) rejects any tab character, (b) walks the
   file's block structure verifying indentation is a consistent multiple of
   spaces with no dangling keys, (c) asserts top-level keys include `name`,
   `on`, `permissions`, `jobs`, (d) asserts job `jms-bridge-native-tests`
   contains `steps` where every step has one of `run`/`uses`, (e) asserts the
   new step's key set is exactly within
   `{name, shell, working-directory, run}` and its `run` line equals
   `./gradlew --offline evictorVerifyNative`, (f) asserts the new step
   contains neither `continue-on-error` nor an `if` key. Record the check
   output in the mission evidence file.
4. Run the full gradle gate locally (transient scope, foreground):
   `./gradlew --offline cleanTest test` from `bridges/jms` with
   `JAVA_HOME=/nix/store/p7sk0r60bbn9ynrpbcjfjpvlj3f5jv9i-temurin-bin-21.0.11`
   first in PATH, inside `systemd-run --user --scope --collect
   --unit=fleet-nativeredis ...` (MemoryMax=6G, TasksMax=512), stop the scope
   after; confirm `pgrep -c compiled_artifa` is 0 before and after.

**Tests:** (executable spec)
- `yaml structural check`: modified workflow → run the pure-Python check →
  exit 0 with `OK: bridge-jvm-tests.yml structure valid, evictor step
  verified` (before Task 1.4: fails — step absent; after: passes).
- `gradle gate`: full JVM suite → `./gradlew --offline cleanTest test` →
  exit 0 (suite includes all pre-existing tests; new verifier is not a JUnit
  test and must not be picked up by the `test` task — its class has no
  `@Test` methods; assert the gradle test report contains no
  `PoolEvictorVerifier` entry).
- `diff scope check`: `git diff $(git merge-base HEAD main)...HEAD --name-only`
  lists ONLY: the six stub files, `PoolEvictorVerifier.java`,
  `build.gradle.kts`, `.github/workflows/bridge-jvm-tests.yml`, and the
  `openspec/changes/native-evictor-verification/` artifacts — no
  `Cargo.toml`, no `Cargo.lock`, no `*.rs`, no production Java
  (`bridges/jms/src/main/**` absent).

**Acceptance:**
- Workflow step added exactly as specified; YAML structural check exits 0.
- `./gradlew --offline cleanTest test` exits 0 from `bridges/jms`.
- Rust gates: `N/A — no Rust changed` (diff contains no `*.rs`, no
  `Cargo.toml`).
- `git status --short` in the worktree shows nothing outside the allowed
  file list above.

- [x] 1.4

## Implementation deviations

- Task 1.1 step 1 specified `StubJmsConnection.getMetaData()` returning
  `null`; the shipped stub returns a minimal JMS 2.0 `ConnectionMetaData`.
  Reason: pooled-jms 2.0.8's `PooledConnection` constructor reads
  `ConnectionMetaData.getJMSMajorVersion()` and a null aborts every physical
  connection creation on the Artemis branch (found by Task 1.3's JVM run,
  fixed in commit 11033ff3d under Task 1.3 step 8's in-scope stub-fix
  provision). Behavior of every other stub method is unchanged.
- design.md's CI-step command was corrected post-bless to
  `./gradlew --offline evictorVerifyNative` (one word) to match the shipped
  workflow step and tasks.md Task 1.4.
