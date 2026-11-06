## ADDED Requirements

### Requirement: JMS pool evictor lifecycle is verified inside the native image by CI

CI SHALL execute a pool-evictor verification as code running inside a native
image (`--no-fallback`, neutral args from the single-source env file) on the
existing JMS-bridge native build path, and the verification SHALL cover both
broker branches — ActiveMQ Classic (`PooledConnectionFactory`) and Artemis
(`JmsPoolConnectionFactory`) — asserting per branch: (a) the evictor thread
starts, (b) idle eviction fires no earlier than the configured idle floor —
proven by event-correlated physical-close timestamps against each connection's
check-in origin — and within the configured interval plus scheduling
tolerance, and (c) `stop()` shuts the evictor down with no pooled connections
left, validating the observed retained baseline rather than an assumed count. The verification SHALL
use the same knob setters as the production builders (mirrored; the wiring
itself stays pinned by the existing knob tests), a short knob profile inside
the validated range, and stub delegates without a broker, Mockito, or any new
dependency. Every blocking step SHALL be deadline-bounded, a watchdog SHALL
guarantee unconditional process termination (`Runtime.halt`, immune to blocked
shutdown hooks, with lock-free diagnostics that never call pool getters), and
every breach SHALL fail the run with a captured diagnostic (pool state, stub
state, thread dump). The
verification SHALL contain no skip, assume, or disable path, and the CI step
running it SHALL fail the job on a non-zero exit or a missing branch PASS
line.

#### Scenario: Native-image leg proves evictor start, interval eviction, and clean shutdown

- **GIVEN** the existing JMS-bridge native tests job has built the bridge
  native image with the pinned GraalVM and the single-source native build args
- **WHEN** the evictor verification native image is built with `--no-fallback`
  and executed for the activemq branch and the artemis branch
- **THEN** each branch reports the evictor thread present, its observed
  retained baseline evicted no earlier than the idle floor — proven by
  event-correlated physical-close timestamps — and within the interval
  window, and after `stop()` no evictor thread and no pooled connections
- **AND** exit code is 0 with both branch PASS lines; any breach, image-build
  failure, or missing PASS line turns the CI job red with the captured
  diagnostic in the job log

#### Scenario: JVM leg proves the same stubs and mirrored wiring drive eviction without a broker

- **GIVEN** the verifier configured with idle timeout 1000 ms and expiration
  check 1000 ms, applied through the same pool classes and setter sequence as
  the production builders, with stub delegates
- **WHEN** `evictorVerifyJvm` runs on this host
- **THEN** both branches pass the same START, RETAINED (observed baseline,
  ≥ 1 connection retained), EVICT, and SHUTDOWN proofs, demonstrating the
  behavior the native leg re-proves inside the image

#### Scenario: Failure is red with diagnostics, never silent

- **GIVEN** a branch where the evictor does not start, eviction fires outside
  the interval window, shutdown does not complete, or the verifier hangs
- **WHEN** the verification runs on any leg
- **THEN** the corresponding bounded step fails, the verifier prints the
  captured diagnostic (pool state, stub state, elapsed times, thread dump) and
  exits non-zero, and the containing CI job fails
- **AND** a watchdog halts the verifier process at its global budget even if a
  library call or shutdown hook blocks indefinitely, so the step can never
  hang

#### Scenario: Production pool behavior is unchanged

- **GIVEN** the diff for this change
- **WHEN** the pool knobs, their defaults (30000), their validated range
  ([1000, 86400000]), the production builders in `JmsClientFactory`, and every
  other production source file are compared against the prior commit
- **THEN** they are unchanged; the change touches only the test sourceset, the
  bridge's existing build script (additive gradle tasks), and one CI step
