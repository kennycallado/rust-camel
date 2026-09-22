# Proposal — sedamatch

## Why

`is_no_active_consumers_gate` (crates/components/camel-component-seda/src/lib.rs)
classifies any `EndpointCreationFailed` whose text CONTAINS "has no active
consumers" or "has no active subscribers" as the SEDA pre-enqueue gate.
Foreign components emit `EndpointCreationFailed` messages too, and a foreign
message that happens to carry the gate wording becomes a false gate. The job
send loop then fails fast and skips the bounded retry window that the
jobreplay spec requires for every non-gate `EndpointCreationFailed`
(bd rc-3px7o, castrict-rpt finding).

The false positive is not hypothetical: any component that reports consumer
liveness with similar wording (brokers, topics, subscription groups) collides
with the substring test. The first spec draft proposed exact-shape message
parsing; the spec-blessing expert REJECTED it — reconstructing a type from
message bytes is still text sniffing. The discriminator must travel WITH the
error.

## What Changes

Typed provenance classification, following the retryclass (11be1863) and
rediserr (db512039) doctrine — the rediserr source-chain walk is the direct
precedent:

- camel-api gains `EndpointCreationFailedWithSource(String, #[source]
  OpaqueErrorSource)`, mirroring the `ProcessorErrorWithSource` precedent
  on every alias axis: Display stays `Endpoint creation failed: {0}`,
  `variant_name()` aliases to `EndpointCreationFailed` (doTry
  catch-by-variant unchanged), `classify()` reports `endpoint`. The
  source handle is opaque — private inner, no public Clone, pointee-only
  `source()` — so a genuine marker cannot be extracted and replayed into
  a fabricated error; cloning the top-level error preserves provenance
  (manual Clone in camel-api arc-clones the private inner).
- camel-component-seda owns a CRATE-PRIVATE typed `NoActiveConsumersGate`
  rejection (Single/Fanout) that implements `std::error::Error` — private
  so foreign code cannot forge gate provenance; its Display is
  non-canonical diagnostic text. A single crate-private constructor
  builds the gate error as the new variant with the typed rejection in
  the source chain; the outer detail stays byte-identical to the current
  wording. All three gate sites route through it.
- `is_no_active_consumers_gate` matches the new variant and probes the
  source chain by a bounded downcast walk (8 hops, Arc-wrapper unwrap
  included; boundary tested). Message text plays NO part in
  classification.
- `is_direct_startup_race` = plain `EndpointCreationFailed` OR the new
  variant whose source is not the seda gate. camel-cli delegation
  unchanged.
- camel-dsl's `on_exceptions` kind value `"EndpointCreationFailed"`
  matches both variants (family grouping — a documented exception to the
  variant-exact `ProcessorErrorWithSource` precedent, authorized by a dsl
  delta; no new kind value).
- Spec deltas: `seda-component` (classification contract), new
  `error-taxonomy` capability (variant alias contract), `dsl` (kind
  grouping).

## Acceptance Criteria

1. A gate classification requires the typed rejection in the source chain —
   no `EndpointCreationFailed` text, including byte-exact imitations of the
   canonical wording, classifies as the gate.
2. Every negative case (foreign wording, decorated text, byte-exact
   imitation, other seda wordings) reports `is_direct_startup_race` true
   (retryable).
3. Genuine single and fanout gate rejections (all three sites) fail fast,
   with detail text byte-identical to the current wording.
4. Alias guarantees hold: doTry variant name, `classify()`, and Display of
   the gate rejection are unchanged.

## Risk Budget

Low-to-medium: one shared-variant addition (additive, `#[non_exhaustive]`,
alias-guarded) plus one crate's classification rewire. No wording drift.
Worst case: a matcher outside the swept set treats the new variant via its
wildcard arm — the sweep enumerates every non-test matcher; aliasing keeps
the generic systems (doTry, classify, metrics families) indifferent.
