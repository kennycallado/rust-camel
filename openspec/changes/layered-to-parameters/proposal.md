# Proposal: layered-to-parameters

## Why

`BuilderStep::LayeredTo` is endpoint-bearing — `RouteDefinition::to_uri_metadata`
(`crates/camel-core/src/lifecycle/application/route_definition.rs:520-523`) returns its URI in
the same `Some` arm as `To` — but both parameter gates in `camel-builder` reject it:

1. the call-time predicate `endpoint_slot_bears_uri`
   (`crates/camel-builder/src/lib.rs:1206-1219`), which records the misuse
   `.parameters() called with no pending endpoint step` the moment
   `.parameters()` follows `.to_layered()`; and
2. the apply-time slot match in `apply_parameter_assignments`
   (`crates/camel-builder/src/lib.rs:1247-1263`), which accepts only
   `To | WireTap | Enrich | PollEnrich` as URI-rewrite slots.

So

```rust
route.from("direct:start").to_layered("mock:b", layers).parameters(map)
```

fails with a `RouteError` claiming there is no pending endpoint step — false
(`to_layered` IS the pending endpoint step) and unhelpful. Found by
mission-510's holistic review. (bd rc-vf7vu, P3.)

The parameter rewrite is purely textual on the authored URI
(`EndpointUri::try_from_uri_and_params` → canonical string) and orthogonal to
the attached layer factory, so accepting it for `LayeredTo` cannot change
layer semantics.

## DSL parity answer (deliverable 3)

**The YAML/DSL lowering cannot express `parameters` on a layered `to:` — the
construct does not exist in the DSL surface.** Evidence:

- `crates/camel-dsl/src` and `crates/camel-config/src` contain zero references
  to `to_layered`, `LayeredTo`, or any layers-on-`to` field (repo-wide grep, 0
  hits).
- Every DSL `to` lowering produces `BuilderStep::To(uri)`:
  `CanonicalStepSpec::To { uri }` (`compile.rs:471`),
  `DeclarativeStep::To(ToStepDef { uri })` (`compile.rs:1051`, `:1654`).
  `ToStepDef` carries only `uri`; the YAML `ToStep` (`route_ast.rs:644`) that
  accepts `parameters` likewise lowers to `BuilderStep::To`.

Therefore there is **no parity constraint forcing the builder to keep
rejecting**: the DSL cannot accept or reject what it cannot express.
`to_layered` is a builder-only (programmatic) API, and the builder must ACCEPT
— this is deliverable 2's acceptance fix, not a message-only fix. No
`blocked: dsl-parity`.

## What Changes

- `endpoint_slot_bears_uri`: accept `BuilderStep::LayeredTo { .. }` in its
  `matches!` set. Without this, misuse is recorded at `.parameters()` call
  time and the apply-time arm below is unreachable for this case.
- `apply_parameter_assignments` slot match: add
  `Some(BuilderStep::LayeredTo { uri, .. }) => uri`, merging into the inner
  URI exactly like `To` (`EndpointUri::try_from_uri_and_params`).
- No other arm changes: `WireTap`, `Enrich`, `PollEnrich`, `From` keep their
  behaviour byte-identically; the `None`/out-of-range arm and the defensive
  `Some(other)` arm keep their messages (the latter already names the step via
  `canonical_step_name`, which returns `to_layered` for `LayeredTo`,
  `lib.rs:1578`).

Note on surface: the order bounded this as "one match arm", but the observed
repro cannot pass with only the apply-time arm — the call-time predicate fires
first and its misuse aborts `build()` before `apply_parameter_assignments`
runs. The minimal correct fix is the same one-line shape added to both
gates; no DSL changes, no endpoint-slot plumbing, no new variant, no rewrite
of `apply_parameter_assignments` beyond the arm.

## Tests (deliverable 4)

In `crates/camel-builder/tests/parameters_builder_tests.rs`, matching the
existing style:

- `parameters_apply_to_layered_to` — `.to_layered("mock:b", layer)` +
  `.parameters(foo=bar)` compiles, `build()` succeeds, and the lowered
  `LayeredTo` step's URI carries the merged parameter (`mock:b?foo=bar`).
- `parameters_on_plain_to_unchanged` — the existing `To` behaviour still holds
  through the shared merge path (regression guard).
- `parameters_on_non_endpoint_step_still_rejected` — a `Log` step still
  yields `CamelError::RouteError` after the fix.

Reproduce-first: the layered test is written and shown failing before the fix
(red run pasted in the park), then passing after (green run).

## Non-goals

- No new builder API, no changes to `LayeredTo`'s shape or its layers
  semantics.
- No rewriting of `apply_parameter_assignments` beyond the arm.
- No touching the other endpoint-bearing steps' tests.
- The call-time misuse message for genuinely non-endpoint steps (e.g. `Log`)
  is left unchanged — for those steps "no pending endpoint step" is accurate.
