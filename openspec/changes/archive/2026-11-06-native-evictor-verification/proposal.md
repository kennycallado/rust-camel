# Proposal: native-evictor-verification

## Why

The JMS bridge now sets `timeBetweenExpirationCheckMillis` /
`connectionCheckInterval` explicitly (default 30000 ms, bd rc-bazi7, follow-up
to mission 382). The commons-pool2 evictor thread therefore runs inside the
native-image build for the first time. Native builds are CI-owned; this host
cannot run them. Nobody has verified that the evictor starts, fires, and shuts
down cleanly inside the SubstrateVM image — on either broker branch
(ActiveMQ Classic `PooledConnectionFactory`, Artemis `JmsPoolConnectionFactory`).
A native-image reachability or shutdown regression would surface as a hung or
leaking bridge in production, not as a test failure.

A prerequisite finding shapes this change: tests in the existing native build
job do NOT execute inside the native image. The Quarkus Gradle plugin runs the
plain JVM `test` task during the native build (`BeforeTestAction` merely sets a
`native.image.path` system property), and the repo has no in-image JUnit
harness (`org.graalvm.junit:junit-platform-native` is absent and cannot be
fetched offline). Suite membership alone cannot prove in-image behavior, so
the verification must be code that the native image actually executes.

## What Changes

- Add `PoolEvictorVerifier` — a plain `main()` verification class in
  `bridges/jms`'s test sourceset (not JUnit, no new dependencies). It drives
  the production pool classes of BOTH broker branches with the same knob
  setters as the production builders (mirrored; wiring pinned by the existing
  `JmsClientFactoryPoolKnobsTest`), using a short profile (1000/1000 ms,
  within the validated knob range) and stub delegates. Per branch it asserts,
  fail-closed and deadline-bounded: the evictor thread starts, idle eviction
  fires within an interval-relative window (not just eventually), and `stop()`
  shuts the evictor down with no pooled connections left. Any breach prints a
  captured diagnostic (pool state, stub state, thread dump) and exits
  non-zero; a watchdog bounds the process exit itself.
- Stub layer (test sourceset, hand-rolled — Mockito does not work in
  native-image tests): a stub `javax.jms.ConnectionFactory`/`Connection`
  implementing the full JMS 2.0 surface (the `createContext` overloads are
  abstract in the pinned API), plus a real `ActiveMQConnection` subclass built
  over a stub `Transport` — required because `activemq-pool`'s
  `PooledConnectionFactory.wrap()` casts the delegate connection to
  `ActiveMQConnection` (bytecode-verified). No broker, no network.
  An independent JVM probe of this construction has already reproduced the
  full Classic lifecycle without a broker: start retains one connection
  (`createConnectionOnStartup=true`), the explicit borrow adds a second
  (retained 2), eviction drops to 0 with two recorded transport stops.
- Three gradle tasks in the existing `bridges/jms` build script:
  `evictorVerifyJvm` (JavaExec — the local JVM evidence), and
  `evictorVerifyNativeImage` + `evictorVerifyNative` which compile the
  verifier into a small second native image with `--no-fallback` and the
  single-source neutral args from `native-build-args.env`, then run it and
  fail on a non-zero exit or a missing branch PASS line. No new dependencies;
  the bridge's own build tasks are untouched.
- One named step in the existing `jms-bridge-native-tests` job
  (`.github/workflows/bridge-jvm-tests.yml`) that runs `evictorVerifyNative`
  after the bridge image build. No new job, no forked build configuration.

Explicitly excluded: bridge runtime behavior changes, Java production-source
changes, dependency bumps, `Cargo.toml`/`Cargo.lock` changes (a peer mission
owns the lock), pipeline restructuring, new broker image versions.

## Acceptance criteria

- `evictorVerifyJvm` passes locally on this host for both branches — JVM proof
  that the mirrored knob wiring produces start/eviction/shutdown behavior.
- The same verifier runs inside a native image on the existing CI native path;
  only that leg proves in-image evictor reachability and clean shutdown.
- A regression is a red CI job with the captured diagnostic; deadline breaches,
  image-build failures, and hangs all fail the job.
- Pool knobs, defaults, ranges, and all production code are unchanged; the
  gradle and YAML gates pass.

## Risk budget

Acceptable: the second small image build costs CI minutes; `-Ob` bounds it.
Not acceptable: a job that cannot fail (skips, assumptions, unbounded waits,
fallback-mode images that silently run on the JVM), changes to bridge runtime
behavior or pool defaults, or lockfile edits.

Affected crates: none (no Rust changes). bd: rc-bazi7.
