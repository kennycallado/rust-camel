# Design: native-evictor-verification

## Approach

**How in-image execution is achieved.** The existing `jms-bridge-native-tests`
job builds the bridge image but does not execute test bodies inside it: the
Quarkus Gradle plugin's `BeforeTestAction` (gradle-application-plugin 3.39.1,
bytecode-inspected) only sets a `native.image.path` system property on the
plain JVM `Test` task, and `org.graalvm.junit:junit-platform-native` — the
only in-image JUnit harness — is not a dependency and cannot be fetched
offline. The verification is therefore a plain `main()` class
(`org.rustcamel.jms.PoolEvictorVerifier`, test sourceset) compiled into a
small SECOND native image by a new gradle task inside the same job, and
executed as a process whose exit code and PASS output the task checks.
`--no-fallback` is mandatory: a fallback image would silently run on the host
JVM and fake the proof. The image reuses the single-source neutral args from
`bridges/jms/native-build-args.env` (parsed the same way xtask parses it) plus
`-Ob` to bound build time; the bridge's own build tasks are untouched.

**Branch wiring mirrors production without touching it.** The verifier
constructs the production pool classes directly (precedent:
`JmsClientFactoryTlsTest`) with the exact setter sequence of the production
builders in `JmsClientFactory` (knobs before `start()`), rather than calling
the builders — direct invocation would be feasible (`buildPooledArtemisFactory`
already takes the interface; `StubClassicConnectionFactory` extends
`ActiveMQConnectionFactory`; `BridgeConfig` is Map-constructible in tests),
but the mirror is the blessed choice and it keeps the verifier independent of
`BridgeConfig`. The mirror carries a known residual: a future production knob
added to a builder would not automatically reach the verifier's mirror. That
residual is bounded by change discipline — any knob change updates
`JmsClientFactoryPoolKnobsTest` (which pins the builder wiring) and this
repository's spec-driven flow would touch the verifier in the same change.
The knob setters the mirror applies are exactly those of the blessed builders.
That the builders apply these exact setters is already pinned by
`JmsClientFactoryPoolKnobsTest`; the verifier proves what those knobs make the
evictor do. Knob profile: `idle timeout 1000`, `expiration check 1000`
(inside the validated [1000, 86400000] range; `BridgeConfig` parsing itself is
covered by `BridgeConfigPoolKnobsTest` and is not exercised here).

**Stub layer, broker-free and native-safe.** No Mockito (runtime class
generation does not exist in SubstrateVM tests) and no broker.

- Artemis branch: `JmsPoolConnectionFactory` around a hand-rolled
  `javax.jms.ConnectionFactory` stub returning stub `Connection`s. The pinned
  `javax.jms` API declares the four `createContext` overloads `abstract` (no
  defaults), so the stub implements all of them (returning `null`; never
  invoked by the `createConnection` path).
- ActiveMQ Classic branch: `PooledConnectionFactory`'s anonymous
  `ConnectionPool.wrap()` casts the delegate connection to `ActiveMQConnection`
  (bytecode-verified: `checkcast org/apache/activemq/ActiveMQConnection`), so a
  plain stub cannot pass. The stub factory subclasses
  `ActiveMQConnectionFactory` overriding `createConnection(...)` to return a
  real `ActiveMQConnection` constructed through its protected constructor
  `(Transport, IdGenerator, IdGenerator, JMSStatsImpl)` over a hand-rolled stub
  `Transport`. The constructor touches only `Transport.isFaultTolerant()` and
  `Transport.setTransportListener(...)` (bytecode-verified). The stub Transport
  records `stop()` — that record is the physical-close proof on this branch.

**Per-branch proof script, all fail-closed.** `START`, `RETAINED`, `EVICT`,
`SHUTDOWN`:

1. START — poll ≤ 10 s for a thread whose name starts with `commons-pool-evictor`
   (commons-pool2 2.13.1 constant pool; both pools funnel eviction through
   commons-pool2 `EvictionTimer`). Absent → fail.
2. RETAINED — borrow one pooled connection, close it client-side (check-in),
   then read the retained count. The baseline is the OBSERVED count, not an
   assumed constant: Classic's `createConnectionOnStartup=true` makes `start()`
   retain one connection before the explicit borrow (JVM-probed: 2 after
   check-in, max connections 5), Artemis retains 1. The verifier asserts
   retained ≥ 1, records the exact baseline N with the check-in timestamp, and
   uses N as the EVICT origin.
3. EVICT — poll until the retained count reaches 0. Both bounds assert the
   configured interval, with scheduling tolerance: eviction must be observed no
   earlier than the 1000 ms idle floor and within
   `idleTimeout + 2 × checkInterval + 3000 ms` (deadline 15 s). The floor is
   proven by EVENT correlation, not by poll sampling: each stub records the
   physical-close wall-clock time (Artemis: stub `Connection` close; Classic:
   stub `Transport.stop()`), and the verifier asserts
   `physicalCloseTime − idleOrigin ≥ idleTimeout` for every closed connection —
   poll descheduling cannot manufacture a false floor pass. On success the
   physical closes for all N baseline connections are asserted.
4. SHUTDOWN — `pool.stop()` bounded ≤ 10 s, then poll ≤ 10 s for: no
   `commons-pool-evictor` thread and retained count 0. commons-pool2's
   EvictionTimer reaper terminates the shared executor once the last task
   cancels; the poll absorbs the asynchronous exit.

Every blocking call (borrow, check-in, `start()`, `stop()`, each poll
window) runs under a preemptive bound: the helper runs it on a single-thread
executor with a timeout (void operations pass null-returning lambdas — a void
method reference does not satisfy `Callable`); on timeout it dumps
diagnostics and fails. A watchdog daemon thread forces
`Runtime.getRuntime().halt(2)` at a global budget (120 s; overridable by
`POOL_EVICTOR_BUDGET_MS` for the fault regressions only) and STAYS ARMED
until process death — no defuse: `System.exit` invokes shutdown hooks before
halting, so a blocked hook could defeat a normal exit, and the armed watchdog
is the unconditional guarantee. Its diagnostics are best-effort on a separate
daemon thread with a short fixed grace before the halt (a blocked or
synchronized `PrintStream` must not prevent termination) and read only
lock-free sources (`Thread.getAllStackTraces()`, volatile stub fields) —
never pool getters, which can block on pool locks. Even a blocked `stop()`,
a blocked check-in, or a blocked shutdown hook cannot hang the step —
bounded process exit is guaranteed by construction. Each branch prints exactly one
`PASS branch=<name> proofs=START,RETAINED,EVICT,SHUTDOWN`
line on success; the gradle run task requires exit code 0 AND both PASS lines
(undiscovered or partially-run verification cannot pass). The CI job's
`timeout-minutes: 60` remains the outer backstop.

**Executable fail-closedness.** The verifier reads `POOL_EVICTOR_FAULT`
(test-sourceset fault hooks only; unset in real runs; each hook prints a
fault-entry line): `block-checkin` (the verifier's own Classic check-in
blocks forever — verifier-side, because the Classic borrowed connection is a
pooled wrapper a stub cannot block), `block-hook` (a blocked shutdown hook),
`fail-artemis` (a deterministic branch failure), and `skip-artemis` (returns
0 without printing its PASS line — the undiscovered-verification case;
test-only). A local gradle task `evictorVerifierFaults` shares ONE output
validator with the CI run task (`validateVerifierOutput`: PASS only on exit 0
AND both branch PASS lines) and runs the verifier as subprocesses via
`ProcessBuilder` — classpath through the `CLASSPATH` env var (no
command-line limit), pipes drained on daemon threads, `waitFor` with an
external deadline plus `destroyForcibly()` — requiring: halt exit code 2
with the fault-entry line and watchdog diagnostics under `block-checkin`;
halt exit code 2 beating the blocked hook under `block-hook`; exit 1 with
`FAIL branch=artemis` under `fail-artemis`; and — through the SHARED
validator — rejection of the `skip-artemis` output (exit 0, missing branch
PASS) and of replayed nonzero-exit output. The red path is executable
evidence against the same gate logic CI uses, not review.

**Diagnostic capture.** On any breach the verifier prints: pool stats
(`getNumConnections`), stub state (close records), elapsed times per proof,
and a full thread dump (name, daemon, state). The gradle test/task logging and
the step output carry it into the CI log.

**CI step.** One step after the bridge image build in
`jms-bridge-native-tests` running `./gradlew --offline evictorVerifyNative`. The
environment is already proven sufficient for `native-image` there — the
preceding step just built a native image in it. The image task resolves the
launcher as `native-image.cmd` on Windows (as Quarkus does) and passes its
arguments through an argfile — the offline classpath is ~42,700 characters,
beyond the Windows command-line limit; the `-cp` token's backslashes and
quotes are escaped for the argfile parser (with an executable round-trip
check), and the argfile is referenced relative to the invocation's working
directory. No `continue-on-error` on the
verification step; a diagnostic step MAY use `if: always()` later if needed,
provided it never masks the failure.

**Local evidence.** `evictorVerifyJvm` (JavaExec, same main) runs on this host
for both branches — proof that the stub layer, the mirrored wiring, and the
bounds work; it cannot prove in-image behavior. The YAML gate is a structural
check (see Gate plan).

**What only the native image can prove.** That `PoolEvictorVerifier`, the
production pool classes, commons-pool2's `EvictionTimer` machinery (thread
factory, weak-ref task map, reaper), and the stub layer are fully reachable
inside SubstrateVM with no reflection/registration gap; that the evictor
thread starts, fires within the configured interval, and cancels cleanly there;
and that the process exits bounded. First CI run of `jms-bridge-native-tests`:
green step = proven; red step = the log names the failed proof with the
captured diagnostic.

## Affected crates

- None — no Rust code, `Cargo.toml`, or `Cargo.lock` changes. Rust gates are
  N/A ("no Rust changed").
- `bridges/jms`: `PoolEvictorVerifier` + stub classes (test sourceset); three
  gradle tasks in the existing build script.
- `.github/workflows/bridge-jvm-tests.yml`: one named step in the existing
  `jms-bridge-native-tests` job.

## Architecture boundaries

No Runtime, DSL, Components, Services, Languages, or Functions surface
changes; no Java production-source changes at all. CI changes are confined to
one step in one existing job. Domain language per CONTEXT-MAP.md: "bridge",
"pool knobs", "broker branch" keep their established meanings.

## Gate plan

- `./gradlew --offline cleanTest test` from `bridges/jms` (JAVA_HOME set per
  mission order; transient systemd scope). Offline resolution verified on this
  host.
- `./gradlew --offline evictorVerifyJvm` and
  `./gradlew --offline evictorVerifierFaults` — the local JVM run of the
  verifier (both branches green) plus the executable fail-closedness
  regressions (halt exit 2 under blocked-call and blocked-hook faults; exit 1
  with `FAIL branch=artemis` under the branch-failure fault).
- Workflow YAML validation: actionlint, js-yaml, and pyyaml are all
  unavailable on this host and no installer network exists, so the gate is a
  justified structural check — a small pure-Python script verifying
  block-structure (indentation consistency, job/step key shapes) and mirroring
  the added step's shape against the sibling build step; GitHub's own workflow
  parse is the authoritative backstop on first push.
- Rust gates: `N/A — no Rust changed` (diff contains no `*.rs`, no
  `Cargo.toml`).
