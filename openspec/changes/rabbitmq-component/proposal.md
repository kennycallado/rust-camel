# Proposal: rabbitmq-component

## Why

rust-camel has no native RabbitMQ support. RabbitMQ is the most deployed
AMQP 0-9-1 broker; today a rust-camel route can reach it only through the
Java-bridge JMS path (JVM boot, bridge pool, AUTO_ACKNOWLEDGE handoff).
bd `rc-hpk35` (P2) tracks this gap as the first Track C1 interop
connector of the `rc-ca8z` roadmap (owner filing-gate exception recorded
in the bd description, 2026-10-08).

A native component removes the JVM: one lapin connection per named
broker, Tower-native lifecycle, and the kafka/mqtt delivery contract
(ack only after the route completes) that the JMS bridge cannot offer.

## What Changes

New crate `camel-component-rabbitmq` (scheme `rabbitmq:`, reserved
`amqp:` stays free for a future AMQP 1.0 component), shipped in the
REGULAR CLI flavor like jms:

- Named brokers in `Camel.toml` `[components.rabbitmq.brokers]`;
  URI `rabbitmq:<exchange>?queue=&routingKey=` (`default` = empty
  exchange).
- Producer: basic publish with header/property mapping, persistent
  delivery by default; publisher confirms and mandatory routing in P3.
- Consumer: prefetch, `concurrentConsumers`, ack after route completes,
  reject without requeue on failure (Camel parity; DLX takes poison),
  `requeueOnFailure` opt-in, redelivered header, stale delivery-tag
  handling across reconnects.
- Topology: passive queue check at consumer start (fail fast), opt-in
  `autoDeclare` (documented divergence from Camel's consumer default).
- Request/reply over direct reply-to with bounded `replyTimeout` (P4).
- lapin 4.12.0, `default-features = false`, tokio + rustls (ring
  backend, never aws-lc); lapin auto-recovery OFF — the project
  `NetworkRetryPolicy` owns reconnection (mqtt/jms pattern).
- Secrets masked per ADR-0051; health check per kafka pattern; Docker
  integration tier per ADR-0069 vocabulary.

Excluded (each rejected-with-reason or filed as future bd, see design):
streams, transactions, batching, manual ack mode, hot credential
rotation, AMQP 1.0, quorum-queue controls, exclusive/server-named
queues, dynamic topology.

Affected crates: `camel-component-rabbitmq` (new), `camel-bundles`,
`camel-cli` (feature `rabbitmq` in `flavor-regular`, golden deptree
regeneration), `CONTEXT-MAP.md` + component `CONTEXT.md` + guide docs.

## Acceptance criteria

- `rabbitmq:` producer and consumer round-trip against a real RabbitMQ
  (Docker fixture), ack observed only after route completion.
- Consumer fails fast when the queue is absent; conflicting active
  declare fails route start (406).
- Failed route → message rejected without requeue by default; DLX
  routing proven in Docker tier; requeue honored when opted in.
- Request/reply round-trips via direct reply-to with bounded timeout;
  late replies dropped.
- All AGENTS.md quality gates green in the worktree; schema-check
  carries the new component metadata; flavor-regular includes the
  component and `cargo build --workspace` stays green.
- Branch releasable at each phase exit; no parsed-but-unused URI
  options at any commit.

## Risk budget

Acceptable: new dependency tree (lapin + rustls ring) audited by
`cargo audit`; regular-flavor binary size growth (like jms).
Out of bounds: a new aws-lc edge selected by camel-component-rabbitmq's
own dependencies (shared pre-existing reachability: bd `rc-2bofp`);
core-crate changes;
breaking any existing gate; merging to main (master lands).
