# Design: rabbitmq-component

## Approach

Native AMQP 0-9-1 component through lapin 4.12.0
(`default-features = false`, exactly the features `tokio`,
`rustls--ring`, and `rustls-native-certs`, so the rustls backend is
**ring**. The bare lapin `rustls` feature selects the rustls default
provider, aws-lc, so the component does not use it; the P1 connection task
verifies the feature/dependency combination that yields ring before
anything else lands). Master ruling (P1, task 1.3): the gate is that the
component adds no new aws-lc edge. The features that the component selects
through lapin choose ring only. aws-lc-rs that shared workspace
dependencies (`camel-component-api`, `camel-auth`, workspace `rustls`
defaults) already make reachable is outside this change and is tracked in
bd `rc-2bofp`. The whole-graph `cargo tree -i aws-lc-rs` result is not a
gate. lapin auto-recovery stays OFF: a
`RabbitConnectionManager` per named broker (jms pool shape) owns the
`lapin::Connection`, re-establishes it with the project
`NetworkRetryPolicy` (unlimited attempts, 5 s initial, x2, 30 s cap —
jms defaults), and hands out channels. Producers take one channel each;
consumers take one channel per concurrent consumer slot.

User-facing pattern copies camel-jms: brokers map in
`[components.rabbitmq.brokers]` (Camel.toml), `RabbitMqBundle`
(`ComponentBundle::config_key() == "rabbitmq"`) registers the component
in the camel-bundles cascade behind a `rabbitmq` feature, camel-cli
gains `rabbitmq = ["dep:camel-component-rabbitmq",
"camel-bundles/rabbitmq"]` inside `flavor-regular`, and the
feature-profile golden is regenerated. URI metadata flows through the
`UriConfig` descriptor (jms `metadata.rs` pattern) with a name-parity
unit test; schema-check picks it up.

Delivery semantics copy kafka/mqtt: the consumer acks **only after the
route completes** (`send_and_wait` Ok → `basic_ack`; Err →
`basic_nack` requeue=false by default, `requeueOnFailure` opts into
requeue). Delivery tags are channel-scoped; the manager stamps a
connection generation counter and drops acks/nacks for pre-reconnect
tags (double-ack protection) — a dropped stale tag means the broker
redelivers, so the component is at-least-once and consumers tolerate
duplicates. Consumer start establishes the connection under the retry
policy first (not-ready until connected), then runs the passive queue
check; a producer publishing while disconnected fails the exchange with
a bounded reconnect-wait error, never an unbounded hang. Request/reply
(P4) uses direct reply-to `amq.rabbitmq.reply-to` consumed no-ack on
the reply channel (publisher confirms do not cover direct reply-to —
replies publish without confirms), a `DashMap` correlation table with
one-shot senders, bounded `replyTimeout` (30 s default), late replies
dropped; a failed replier route sends no reply.

Secrets: broker passwords never traverse `Debug`/`Serialize`
(ADR-0051); the component redacts broker URLs in errors and logs.
Health: `AsyncHealthCheck` probe with 5 s timeout reporting Unhealthy
on failure (kafka `health.rs` shape) — lands in P1 with a
probe-timeout unit test. Metrics follow component-metrics-emission:
`camel_component_operations_total{component,operation,outcome}` —
publish from P1 (producer), consume from P2 (consumer).

Docker tier (ADR-0069 vocabulary): no testcontainers RabbitMQ module
exists and the scenario harness has no rabbitmq adapter (demand-gated),
so integration tests are per-crate `tests/*.rs` binaries with a shared
`tests/common` fixture that runs `docker run -d -p 127.0.0.1:<port>:5672
-e RABBITMQ_DEFAULT_USER=rmq -e RABBITMQ_DEFAULT_PASS=rmq
rabbitmq:3.13-alpine`, probes readiness with a real AMQP handshake,
and tears down with `docker rm -f`. The `rmq` credentials are
FIXTURE-ONLY, never application secrets: the official image's default
`guest` user only authenticates from the container loopback, so a host
connection through the docker bridge gateway requires a named default
user. Two binary activation rules: with
`RABBITMQ_ITEST` unset, broker-dependent tests do not run and print an
explicit notice naming `RABBITMQ_ITEST=1`; with `RABBITMQ_ITEST=1` and
docker unavailable, they panic with an `infra-unavailable` message
(ADR-0069 §7, no silent skip). This mission runs the tier locally in
the worktree gate sweep; a dedicated CI integration job is filed as a
future bd under rc-ca8z.

Anti-lost guard: every URI option appears in the metadata descriptor
and config schema only in the phase whose task implements it — the jms
dead-option debt (options parsed but unused) must not recur. The
metadata parity unit test asserts the descriptor option set equals a
per-phase const list, and each listed option carries at least one
behavior test.

## Affected crates

- `crates/components/camel-rabbitmq` (new, package
  `camel-component-rabbitmq`): component, config, connection manager,
  producer, consumer, headers mapping, health, metadata, bundle,
  Docker-tier tests.
- `crates/camel-bundles`: `rabbitmq` feature + cascade registration.
- `crates/camel-cli`: optional dep, `rabbitmq` feature, membership in
  `flavor-regular`, regenerated `default-deptree.txt` golden.
- Docs: `CONTEXT-MAP.md`, component `CONTEXT.md`, README, mdBook
  guide + example (P5).
- No core/api crate changes.

## Architecture boundaries

Components layer only. The crate depends on `camel-component-api`,
`camel-api`, `camel-health`-shaped check traits via camel-api, tokio,
lapin. It registers through `ComponentBundle` (camel-bundles cascade),
never touches Runtime/DSL internals. Data plane = exchanges through the
standard Producer/Consumer seams; control plane = Camel.toml config
resolution. Hexagonal boundaries are enforced by the existing
`hexagonal_architecture_boundaries_test`.

## Phases

### Phase 1: Foundation + producer
- **Goal:** crate skeleton through working basic publish against a real
  broker; component visible in the regular CLI.
- **Dependencies:** lapin 4.12.0 ring-TLS verification; jms patterns
  (config/bundle/metadata/masking); ADR-0051.
- **Externally-visible types/interfaces:** package
  `camel-component-rabbitmq`, `RabbitMqBundle`,
  `RabbitConnectionManager`, scheme `rabbitmq:`, URI options
  `broker`, `queue`, `routingKey`, `persistent`, `contentType`.
- **Deliverable:** compiling crate wired into camel-bundles + regular
  CLI; shared Docker fixture; health check; publish metrics; README
  stub. Publisher confirms are ON from P1 with a fixed 5 s internal
  bound (deterministic failure detection); the `confirmTimeout` option
  only surfaces in P3.
- **Exit-criteria:** unit tests (config resolution, masking, URI
  metadata parity, health probe timeout) green; Docker publish test
  green under `RABBITMQ_ITEST=1` (verified with raw lapin `basic_get` —
  no consumer exists yet); `cargo build --workspace` + fmt + clippy +
  schema-check + feature-profile goldens green. Gating scenarios:
  "default exchange publish target", "single broker resolves
  implicitly", "ambiguous broker selection errors", "persistent
  default publish", "password never traverses Debug", "broker down
  reports Unhealthy", "publish outcome counted" (failure case: publish
  to a missing exchange), "publish while disconnected fails bounded",
  "component-selected TLS stack adds no aws-lc edge", "fixture round
  trip", "unset gate prints notice and does not run", "gated tier
  without docker panics", "metadata parity holds at every phase exit"
  (restated at every later phase exit).

### Phase 2: Consumer + delivery semantics
- **Goal:** consumer with ack-after-route, reject-on-failure, prefetch,
  concurrency, reconnect-safe tags.
- **Dependencies:** P1 connection manager (generation counter),
  readiness pattern.
- **Externally-visible types/interfaces:** URI options `prefetch`
  (default 10), `concurrentConsumers` (default 1), `requeueOnFailure`
  (default false); header mapping `rabbitmq.redelivered`.
- **Deliverable:** consumer + unit seams (ack/reject decision, stale
  tags) + consume metrics + Docker tests (round trip, ack-only-after-
  success, DLX routing on reject, redelivery after mid-flight stop).
- **Exit-criteria:** gating scenarios green: "retry policy owns
  reconnect", "consumer not ready until connected", "ack follows route
  completion", "concurrent consumers register on the queue", "failed
  route rejects to DLX", "requeue opt-in", "redelivered flag maps to
  header", "free-form header round trips", "redelivery after
  mid-flight stop", "stale tag after broker restart is dropped",
  "consume outcome counted", "metadata parity holds at every phase
  exit". Branch releasable (only implemented options surfaced).

### Phase 3: Producer reliability + topology
- **Goal:** confirmed, mandatory publishing and topology safety.
- **Dependencies:** P1 producer; lapin confirms API.
- **Externally-visible types/interfaces:** URI options `confirmTimeout`
  (default 5 s), `mandatory` (default false), `autoDeclare` (default
  false), declare options (`exchangeType`, `durableQueue`,
  `queueArguments` x-args passthrough).
- **Deliverable:** confirm wait with bounded timeout, basic.return
  mapping to route error, passive queue-exists check (fail fast),
  opt-in declare, 406-conflict fails route start; Docker tests
  (unroutable+mandatory, conflicting declare).
- **Exit-criteria:** gating scenarios green: "confirm timeout fails the
  exchange", "unroutable mandatory publish fails", "missing queue fails
  fast", "conflicting declare fails route start", "autoDeclare creates
  topology", "metadata parity holds at every phase exit". AGENTS.md
  QUALITY GATES green.

### Phase 4: Request/reply
- **Goal:** InOut producer + consumer reply path over direct reply-to.
- **Dependencies:** P2 consumer, P3 producer; verified hypothesis: no-ack
  consume on `amq.rabbitmq.reply-to`, replies without confirms.
- **Externally-visible types/interfaces:** URI option `replyTimeout`
  (default 30 s), InOut exchange pattern on producer; consumer replies
  to inbound `replyTo`/`correlationId`.
- **Deliverable:** correlation map with one-shot channels, bounded
  timeout, late-reply drop; Docker tests (round trip, timeout, late
  reply, no reply on failed route).
- **Exit-criteria:** gating scenarios green: "reply round trip", "reply
  timeout fails within the bound", "late reply is dropped", "failed
  route sends no reply", "metadata parity holds at every phase exit";
  no reply leak (map drains on timeout/cancellation).

### Phase 5: Docs + hardening
- **Goal:** operator-facing documentation + full gate sweep.
- **Dependencies:** P1-P4 behavior frozen.
- **Externally-visible types/interfaces:** README, component
  CONTEXT.md, CONTEXT-MAP entry, mdBook guide + runnable example.
- **Deliverable:** docs carrying defaults, divergences (auto-declare
  OFF, reject-no-requeue), exclusions table.
- **Exit-criteria:** doc-build + full AGENTS.md gate list green;
  holistic review APPROVE.

## Exclusions

| Item | Disposition |
| --- | --- |
| Streams | rejected: different protocol extension surface; future bd under rc-ca8z if demand |
| Transactions (tx publish/ack) | rejected v1: interleaves with ack-after-route semantics |
| Batching publish | rejected v1: no route-level batch concept yet |
| Manual ack mode | rejected: contradicts ack-after-route contract |
| Hot credential rotation | future bd under rc-ca8z: requires connection refresh design |
| AMQP 1.0 | `amqp:` scheme reserved; separate fe2o3-amqp component later |
| Quorum-queue controls | rejected v1: declare x-args passthrough covers operators |
| Exclusive / server-named queues | rejected: consumer lifecycle mismatch (stop/rejoin semantics) |
| Dynamic topology (runtime re-declare) | rejected: declare happens at route start only |

## Alternatives considered

- `amqp:` scheme — rejected (owner decision): Camel `amqp:` is 1.0-only;
  keep the name reserved.
- testcontainers generic image — rejected: new dep tree for one image;
  raw `docker run` fixture is smaller and CI-explicit.
- lapin auto-recovery ON — rejected: bypasses NetworkRetryPolicy
  observability and generation tracking (stale-tag safety).
- Camel consumer default auto-declare ON — rejected (owner decision):
  accidental wrong-type queue creation; passive check + opt-in declare.
- JMS bridge reuse — rejected: JVM boot + AUTO_ACKNOWLEDGE handoff is
  the known limitation this component exists to remove.
