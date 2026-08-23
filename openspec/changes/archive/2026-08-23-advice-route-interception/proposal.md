# Proposal: advice-route-interception

## Why

The only supported unit-testing pattern bakes `to: mock:` endpoints into
production route sources. Under `camel run` this costs a full Exchange clone,
lock, and retention (up to 10k exchanges per endpoint) on every message, and
couples test instrumentation to production code. ADR-0064 (accepted 2026-08-23)
pins the fix direction: production routes stay clean; tests intercept send
points at boot time (Java Camel `adviceWith` is inspiration, not conformance —
ADR-0046). This change is Stage A of that contract: the camel-core primitive.
The declarative `*.test.yaml` surface is Stage B (rc-7f0n, separate change).

## What Changes

- New `InterceptRules` model in camel-core: ordered rules, exact-URI match,
  first-match-wins (duplicates permitted, order preserved). Two actions:
  `SkipTo` (replace the send with a `mock:` URI) and `DivertCopyTo`
  (`WireTapService`-composed copy to a `mock:` URI: detached when admitted,
  inline with back-pressure when saturated; the real send proceeds and its
  outcome returns verbatim). Action targets outside the `mock:` scheme are
  rejected at rule construction.
- Rules are applied during route compilation at the `To` send point
  (`EndpointsCompiler`), before component resolution: a skipped URI never
  resolves its real component, so lean-boot tests can exercise routes that
  reference unregistered heavy components.
- `DivertCopyTo` reuses the WireTap isolation pattern: a copy-send failure is
  logged and swallowed; the real path's `PipelineOutcome` is never corrupted
  (ADR-0024).
- seda send-side interception is in scope (ADR-0064 §6 permanent fence):
  `to: seda:x` wrap/substitution happens at the enqueue producer. Consumer
  side, fanout-partial semantics, and post-queue observation stay out.
- Rules are accepted only until the first successful route registration or
  context start, whichever occurs first, and frozen afterwards — race-free
  because route compilation is owned by the sequential controller actor.
  Setting rules later is rejected; stop/restart does not unfreeze.
  Data-plane Tower decoration only — no RuntimeBus/RuntimeQuery involvement
  (ADR-0002/0045), no new crate (ADR-0055).

Explicitly excluded: `from:` endpoint interception, `WireTap` step
interception, URI wildcards, post-start rule mutation, the `*.test.yaml`
surface (Stage B), and any CLI wiring.

## Acceptance criteria

- Exact-URI first-match-wins matching with ordered rules; non-`mock:`
  targets rejected at construction (verified by tests).
- `SkipTo` replaces the producer and never resolves the real component
  (provable with a route whose real scheme has no registered component).
- `DivertCopyTo` composes `WireTapService(copy target)` before the real
  producer: clone to the copy target (detached when admitted, inline with
  back-pressure when saturated), real outcome returned verbatim; failing
  copy (`poll_ready` or `call`) is logged and swallowed; failure to resolve
  the copy target at compile time is a hard error.
- `to: seda:x` divert delivers both the copy and the real queue message.
- Setting rules after the first route registration or start (whichever
  first) is rejected with a clear error; hot-reload recompiles apply the
  same frozen rules.

## Risk budget

Highest blast radius: divert error isolation leaking into the real path. This
must be covered by explicit tests in this change. Interception is opt-in and
absent from default boots, so the no-rules path must remain byte-identical in
behavior. bd rc-3bq5 (epic rc-7roi). Affected crates: camel-core,
camel-processor.
