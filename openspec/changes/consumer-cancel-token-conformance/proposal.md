# Proposal: consumer-cancel-token-conformance

## Why

Five route consumers create their consumer-lifetime cancellation token
with `CancellationToken::new()`. A self-created token is orphaned. On
route stop the Runtime cancels the token it owns through
`ConsumerContext::cancel_token()` (camel-core supervision chain;
ADR-0007 keeps consumer lifecycle Runtime-owned). The orphan never
observes that signal.

Impact is uneven:

- kafka (`crates/components/camel-kafka/src/consumer.rs:65`) and
  wasm-source (`crates/components/camel-component-wasm/src/source_consumer.rs:78`):
  the consumer loops are deaf to the context token. Supervision aborts
  them at the stop timeout (about 10 s for kafka). The kafka commit-drain
  tail never runs. At-least-once delivery is at risk.- redis (`crates/components/camel-redis/src/consumer.rs:153`): both poll
  loops (pubsub, queue) listen only to the self-created token. Same
  defect class as kafka.
- jms (`consumer.rs:446`) and cxf (`consumer.rs:151`): the loops already
  select `ctx.cancelled()` in parallel. The defect is contract
  non-conformance only. Route stop works today.

bd issue: rc-ibwa. An e_glm investigation (2026-08-29, two rounds)
corrected the original premise: production sites are one per component.
Earlier counts (5/7/2/11) included test code. master
(`supervision.rs:74`) and mqtt producer (`producer.rs:134`) are accepted
patterns. cron, direct, and container already conform.

## What Changes

- kafka, jms, cxf: `Consumer::start` takes the consumer-lifetime token
  from `ctx.cancel_token()`.
- redis: `start` links `ctx.cancel_token().child_token()`. Local `stop()`
  keeps its local scope. The design explains why a direct replacement is
  wrong here.
- wasm-source: `start()` overwrites the token created in `new()`.
- One new test per consumer. Each test cancels the context token and
  observes loop exit.

Excluded: master supervision and mqtt producer (accepted patterns);
wasm `producer.rs:141` and `bean.rs:52` (no `ConsumerContext` exists
there; bd rc-515m tracks the `ComponentContext` token accessor);
the camel-core supervision select race (pre-existing Runtime behavior).

## Acceptance criteria

- All five consumers acquire the consumer-lifetime token via
  `ctx.cancel_token()` in `Consumer::start`.
- Each consumer loop observes route-stop cancellation without waiting
  for the stop-timeout abort.
- Existing consumer tests pass. Each consumer gains one test that
  cancels the context token and observes loop exit.
- Per-request child tokens keep their behavior.
- `design.md` records master and mqtt producer as accepted patterns.

## Risk budget

Low. Each production change is one to three lines. Two behavior shifts
are expected and correct: kafka `stop()` on route stop changes from
`Err(join)` to `Ok`; wasm guests observe cancellation earlier on route
stop, which matches the Runtime-owned consumer lifecycle that ADR-0007
establishes. Out of bounds: API changes to `ComponentContext`,
supervision select fairness in camel-core.
