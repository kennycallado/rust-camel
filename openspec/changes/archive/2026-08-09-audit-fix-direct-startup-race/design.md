# Design: audit-fix-direct-startup-race

## Approach

Adopt the existing `ConsumerStartupMode::Explicit` startup handshake — the same
pattern HTTP already uses (`camel-http/src/lib.rs:1407-1413`). Two behavioral
edits, zero data-structure changes:

1. `DirectConsumer` overrides `startup_mode()` → `Explicit`. The runtime's
   `spawn_consumer_task` will create a `StartupReceiver` and `await` it before
   completing `StartRoute`, instead of pre-resolving at spawn time.

2. `DirectConsumer::start()` calls `context.mark_ready()` after the
   registration block closes (lock guard dropped, insert committed) and before
   the event loop. This is the direct analog of HTTP calling `ctx.mark_ready()`
   after binding its listener.

The producer side is untouched. `poll_ready` still returns
`EndpointCreationFailed` when the registry has `None` — but with the handshake,
an auto-started producer route ordered after the consumer route will never
observe `None` because `start_context` starts routes sequentially by
`startup_order`.

## Affected crates

- **camel-component-direct**: `startup_mode()` override + `mark_ready()` call +
  2 new tests + CONTEXT.md note.
- **camel-component-api**: no changes — `ConsumerStartupMode` and
  `ConsumerContext::mark_ready()` already exist.
- **camel-core**: no changes — `spawn_consumer_task` already handles Explicit
  mode via `StartupReceiver`.

## Architecture boundaries

This change stays entirely within the Component layer. The runtime
(`camel-core`) already supports `Explicit` startup via rc-w1u9 — Direct is
simply opting in. No changes to the Runtime, DSL, Services, Languages, or
Functions layers. No ADR needed: this is a component adopting an existing
architectural decision, not a new one.

## Alternatives considered

- **Option A — `Poll::Pending` + waker in `DirectRegistry`**: REJECT. Reinvents
  `StartupSignal` at the data-plane layer. Hand-rolled waker bookkeeping across
  `DirectProducer` clones is lost-wakeup-prone. Entangles `fail_if_no_consumers`
  with a second meaning.

- **Option B — Register at `create_consumer()` time**: REJECT. Forces channel
  creation out of `start()`, leaving phantom senders (live `tx`, no `rx`) on
  rollback paths. Changes duplicate-consumer detection timing.

- **Option C — Runtime-wide readiness barrier**: REJECT. The per-route barrier
  already exists via the handshake + sequential `start_context`. Disproportionate
  lifecycle change for a single-component defect.

- **Hybrid — `fail_if_no_consumers`-driven Pending+timeout**: REJECT. Overloads a
  tri-state knob with two orthogonal concerns.
