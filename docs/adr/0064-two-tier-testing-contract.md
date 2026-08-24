# ADR-0064: Two-Tier Testing Contract

**Date:** 2026-08-23
**Status:** Accepted
**Ratified:** e_glm, acting for the maintainer, 2026-08-23
**Origin:** bd rc-379d (epic rc-7roi)
**Amends:** none
**Cross-refs:** ADR-0002 (CQRS RuntimeBus — control-plane ceiling), ADR-0004 (hot-reload atomic pipeline swap), ADR-0024 (PipelineOutcome), ADR-0042 (Arc compiled-steps snapshot), ADR-0045 (camel-core architecture charter), ADR-0046 (Camel is inspiration, not conformance), ADR-0055 (publish topology), ADR-0062 (reserved test suffix), prior rulings: e_opus 2026-08-18 (`docs/reviews/2026-08-18-camel-mock-expansion-ruling.md`), e_opus 2026-08-23 (this ADR)

## Context

rust-camel has one in-process test surface today. `camel test` boots a lean
`CamelContext`, loads route documents, delivers `direct:` inputs, settles
traffic, and evaluates mock expectations (`crates/camel-cli/src/commands/test/runner.rs`).
The `mock-testkit` spec (`openspec/specs/mock-testkit/spec.md`) is its live
contract.

This surface has no name that separates it from the different job of testing a
route against a real adapter with a full runtime. Without that name, feature
requests push the lean boot toward a general runtime. One such request
(rc-5s8c) proposed adding `http:` to the lean boot; it never landed. That
direction is wrong: it grows the in-process boot into a second, half-formed
runtime, and it hides the growth inside `register_component` lines that no
reviewer gates.

Two prior facts frame this decision:

- The 2026-08-18 ruling fixed the mock as a **sink**, not an in-out simulator,
  and rejected any inspection channel that threads component state into the
  CQRS read side. That ruling forbids widening the data plane through the
  control plane (ADR-0002 / ADR-0045). Route interception (AdviceWith) was
  confirmed out of scope for the component and parked for the test surface.
- The lean boot is real but narrow. `boot_context()` registers exactly five
  components: `mock`, `direct`, `timer`, `log`, `seda`
  (`runner.rs:56-67`). It registers no bean registry, discards producer
  replies in the delivery loop (`runner.rs:168`), and registers no dataformat.

This ADR names the two jobs, pins the boundary between them, and gives the
route-interception feature (AdviceWith) a staging frame so it can land without
reopening the plane question. It binds the unit tier only. It sketches the
integration tier without binding it.

## Decision

### 1. Two tiers, one boundary

rust-camel testing has two tiers. The boundary between them is fixed by two
axes: the **inbound stimulus** that drives the route under test, and the
**runtime profile** that hosts it.

- **Unit tier** (`camel test`): a lean in-process boot. Stimulus is `direct:`
  only. Runtime profile is the closed lean component set below. No external
  transport, no bundle config, no full runtime.
- **Integration tier** (sketched in section 4, not bound here): a full runtime
  boot with real adapters and transport-boundary assertions.

The two-tier framing is the load-bearing decision. Every other rule in this
ADR follows from it.

### 2. The closed lean component set

The unit-tier lean boot registers exactly this set:

```text
{ direct, log, mock, seda, timer }
```

This set is **closed** and pinned by this ADR. It mirrors the current
`boot_context()` reality (`runner.rs:56-67`). An addition to the set requires
an amendment to this ADR. The amendment is the gate. A silent
`register_component` line in the runner is not a valid way to grow the set,
because it grows the unit-tier runtime profile with no review.

`timer:` stays in the set as a deterministic in-process source. It carries no
producer, so it is inert to the send-point interception in section 5. A
`timer:` route that never quiesces is a test-authoring concern (bound it with
`repeatCount`), not a reason to evict the component.

### 3. The creep rule (the test that rejects new components)

For any proposed unit-tier capability, apply this test:

> Does this capability require an inbound stimulus other than `direct:`, or a
> component outside the closed lean set? If yes, it belongs in the integration
> tier, not in the `camel test` lean boot.

This is the test that rejects `http:` in the lean boot (rc-5s8c). `http:` is
both outside the closed set and an external transport stimulus. It fails the
test on both clauses. The rule is orthogonal to set membership: a component in
the set (for example `seda:`) passes the set clause, and a route driven only
by a `direct:` producer passes the stimulus clause, even when that route reads
from `seda:`.

### 4. Integration tier — SKETCH (non-binding)

> **This section is a non-binding sketch.** It records intended direction so
> that the boundary in section 1 has a defined other side. It does **not**
> ratify vocabulary, assertion shape, or API. The binding integration-tier
> contract requires a future ADR. Do not cite this section as settled
> contract.

Intended shape, for orientation only:

- **Embedded mode only.** The integration tier boots a full runtime in-process
  (bundles, `Camel.toml`) and drives routes through real adapters.
- **Standalone mode is out of scope.** Testing a separately deployed
  `camel run` process over IPC is out of scope until the frozen no-IPC
  invariant is explicitly amended by a future ADR. The 2026-08-18 ruling
  rejected inventing an IPC control surface for test assertions; that rejection
  stands here.
- **Citrus-inspired vocabulary** (send / receive-with-timeout / sleep /
  validate against logical endpoints), used as inspiration, not conformance
  (ADR-0046). receive-with-timeout assertions sit at transport boundaries.
- **Partner behavior simulation** (rc-i2qf, re-parented under epic rc-kk69) is
  integration-tier work.

### 5. AdviceWith — staging context (enabled here, not implemented)

This ADR enables route interception (AdviceWith). It does not implement it.
Interception stays a **data-plane Tower decoration**. It is never routed
through the RuntimeBus or a `RuntimeQuery`; that path breaks the ADR-0002 /
ADR-0045 control-plane ceiling (the 2026-08-18 ruling, fact 2). The intercept
is baked into the compiled-steps snapshot (ADR-0042 / ADR-0004), so it applies
atomically and is not mutated after `context.start()`.

**Stage A — boot-time interception (camel-core).** Boot-time `InterceptRules`
plus a Tower send-point wrapper installed before `context.start()`. Match is
exact-URI, first-match-wins. Two divert kinds:

- **divert** (WireTap-style): the intercept copies the exchange to a mock sink
  as an outcome-isolated side-effect. A failure in the mock copy MUST NOT
  corrupt the real path's `PipelineOutcome` (ADR-0024). The copy is a side
  branch, exactly like WireTap.
- **skip**: full producer replacement. The real producer never runs.

Stage A send-point scope includes `seda:` send-side (see section 6). It
excludes `seda:` consumer-side and any post-queue assertion semantics (also
section 6).

**Stage B — declarative intercepts.** An intercept `block` in `*.test.yaml`
(rc-7f0n), building on the reserved test-suffix contract (ADR-0062).

**Stage C — deprecation of inline `to: mock:` in production routes.** A
scope-aware lint that warns, then errors, mirroring the ADR-0045 §4 discipline
(rc-car5). Inline `mock:` stays legitimate in pure test-fixture routes that
`camel run` never loads. Migration is lazy. There is no flag-day.

No new crate is introduced by any stage (ADR-0055 publish topology).

### 6. The seda send-side carve-out (pinned, fenced — not revocable)

The maintainer requires `seda:` send-side interception inside Stage A scope.
This deviates from the original ruling, which excluded `seda:` until demand
appeared. Demand has appeared (maintainer instruction, 2026-08-23), and the
mechanics are verified safe:

- `to: seda:x` enqueues from the producer's `call()`: `tx.send().await` under
  `blockWhenFull=true`, else `tx.try_send`
  (`crates/components/camel-component-seda/src/lib.rs:761-831`). The producer
  is a `Service<Exchange>` wrapped in `BoxProcessor::from_fn`
  (`crates/components/camel-component-seda/src/lib.rs:514-528`). Wrapping that
  producer is mechanically identical to wrapping any other producer.

The carve-out is **pinned and fenced**, not a revocable knob. Fenced OUT of
scope, as hard boundaries:

- **`seda:` consumer-side interception** (`from: seda:`). Consumer forwarding
  runs in a separate background task (`forward_envelope`, `lib.rs:727`); it is
  not a producer send point.
- **Fanout-partial semantics** across the subscribers map. Fanout is
  all-or-nothing at the producer and has no single interception point that
  represents "one subscriber".
- **Post-queue (processed-side) assertion semantics under divert.** A divert
  records the exchange **pre-queue** (before `tx.send`). A test that asserts
  processed-side state under divert observes intake, not consumer output. This
  gap is why the fence exists.

The carve-out is not revocable by convenience. A future change that wants
consumer-side, fanout-partial, or post-queue seda interception amends this
ADR, the same gate as any lean-set change. Removing the send-side carve-out
passes the same gate. Revocability would reintroduce the ambiguity that the
closed-list discipline exists to remove.

## Consequences

### Positive

- The boundary is a named test, not a habit. `camel test` cannot silently
  grow into a second runtime.
- AdviceWith has a staging frame that never touches the control plane. Each
  stage lands on its own review, with divert error-isolated per ADR-0024.
- The seda send-side carve-out is explicit and fenced, so the safe part ships
  without dragging in the unsafe part.

### Negative

- The closed set forces an ADR amendment for any new unit-tier component. This
  is intended friction. It is the cost of keeping the lean boot lean.
- The integration tier stays a sketch, so integration testing has no bound
  contract until a future ADR lands. Accepted: binding it now would ratify
  vocabulary before the design work is done.

### Known unit-tier gaps (follow-ups under epic rc-7roi, not fixed here)

These are current lean-boot limits. This ADR names them so they are tracked,
and does not specify their solutions:

- **rc-07qh:** the lean boot registers no bean registry, so routes with
  `bean:` steps fail under `camel test`.
- **rc-66c5:** the delivery loop discards producer replies
  (`producer.oneshot()`, `runner.rs:168`), so InOut routes cannot assert
  replies.
- **rc-24e5:** no explicit dataformat registration in the lean boot
  (investigation).

> Amendment (2026-08-24): the rc-07qh (bean registry) gap was closed by
> bean-test-registry, and the rc-66c5 (reply capture) gap was closed by
> reply-capture. Stage C's warn-phase lint (rc-car5) completes the §5 warn phase:
> `camel lint` now warns `R-MOCK-IN-PRODUCTION` on inline `mock:` sends in
> route files, with fixture-path and test-document exemptions.

## Alternatives considered

- **`http:` in the lean boot (rc-5s8c):** rejected by the creep rule.
  It is outside the closed set and an external transport stimulus.
- **AdviceWith through the RuntimeBus / RuntimeQuery:** rejected. It breaks
  the ADR-0002 / ADR-0045 control-plane ceiling and forces endpoint state into
  the projection read model (2026-08-18 ruling, fact 2).
- **Porting the Java AdviceWith API shape verbatim:** rejected. Camel is
  inspiration, not conformance (ADR-0046).
- **arming / lazy-recording of mock traffic:** rejected. It subsidizes the
  anti-pattern of inline `mock:` in production routes instead of retiring it.
- **A `record=false` URI parameter:** rejected. It is a per-endpoint knob for
  a route-level concern.
- **A `[components.mock]` config knob:** rejected. It is a config surface for
  a decision that belongs in the route or test document.
- **Standalone integration mode (test a deployed `camel run` over IPC):**
  rejected here. Out of scope until the no-IPC invariant is explicitly amended.

## Concession

Bounded-retention hardening in `camel-mock` is permissible as **pure
hardening** if a live production memory cost appears. This is a cap on
retained exchanges, nothing more. It MUST NOT introduce record semantics or
any inspection channel. The sink identity from the 2026-08-18 ruling holds.

## Self-grill record

**Questions generated:**

1. [glossary] Does "two-tier" or "creep rule" collide with an existing
   CONTEXT-MAP term or an established ADR concept?
2. [sharpen] "Inbound stimulus" is fuzzy. Does adding `seda:` to the closed
   set contradict the creep rule, given `from: seda:` is itself an inbound
   stimulus?
3. [scenario] Under divert, what does a post-queue seda assertion observe, and
   does that break the mock-sink contract?
4. [cross-ref] Does the seda producer actually wrap like any other producer,
   and is `timer:` truly inert to send-point interception?

**Answers (with citations):**

1. [glossary] No collision. CONTEXT-MAP has no "two-tier" or "creep rule"
   entry; the control-plane ceiling it does define (`CONTEXT-MAP.md:132`,
   Synchronous-projection CQRS, ADR-0002 / ADR-0045) is the ceiling this ADR
   respects, not one it renames. The intercept-as-snapshot claim aligns with
   PipelineOutcome (`CONTEXT-MAP.md:156`, ADR-0024).
2. [sharpen] No contradiction. Set membership and the creep rule are
   orthogonal. `seda:` is IN the closed set, so it passes the set clause; a
   route driven by a `direct:` producer that then sends to `seda:` uses no
   stimulus other than `direct:`. `http:` fails both clauses; that is why the
   rule rejects it. Wording sharpened to "external transport inbound stimulus."
3. [scenario] A divert records the exchange pre-queue, before `tx.send`
   (`crates/components/camel-component-seda/src/lib.rs:761-831`). A
   processed-side assertion under divert would observe intake, not consumer
   output. This does not break the sink contract; it is fenced OUT explicitly
   in section 6 so no test relies on it.
4. [cross-ref] Verified. The seda producer is a `Service<Exchange>` returned
   as `BoxProcessor::from_fn`
   (`crates/components/camel-component-seda/src/lib.rs:514-528`); wrapping it
   is identical to wrapping any producer. `timer:` carries no producer (it is a
   `from:` source), so a send-point wrapper never sees it — inert, no scope
   surprise.

**Open question escalated to the human, resolved at ratification (2026-08-23):**
whether the seda carve-out should be **revocable** or **permanently
pinned-and-fenced**. Ruling: **permanently pinned-and-fenced**. Send-side is
IN; consumer-side, fanout-partial, and post-queue are OUT. Any change to the
fence, in either direction, requires an ADR amendment, the same gate as a
lean-set change. A revocable carve-out would create two contract classes
inside one ADR, one gated by amendment and one revocable by convenience, and
would reintroduce the ambiguity the closed-list discipline removes.

**Outcome:** confirm (seda carve-out pinned-and-fenced; open question resolved
at ratification, see above)
**Self-grill mode:** self-grill-proposals skill
