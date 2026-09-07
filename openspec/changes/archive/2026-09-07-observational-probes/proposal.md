# Proposal: observational-probes

## Why

ADR-0072 prescribes the unit tier's step identity through observational
probes (step 3 of its staged direction). Recon shows the weave itself
already exists end to end: `intercepts` with `divertCopyTo: mock:X`
delivers a pre-send copy to a mock endpoint while the real send continues
(camel-core composes the copy stage; the mock auto-creates the endpoint;
`expects` already asserts on any mock endpoint, including divert targets).
What is missing for true step identity:

1. No cross-endpoint arrival-order assertion exists anywhere in the kit —
   per-endpoint counts and bodies cannot express "the route visited
   probe-1 before probe-2".
2. The divert-copy weave has zero driver-level tests in
   `camel-cli/src/commands/test/driver_tests.rs` — the canonical scenario
   in the spec ("divert copies to the mock while the real endpoint
   receives traffic") is not locked by an executable driver test.
3. The probe pattern (divert + expects + order) is undocumented for test
   authors.

## What Changes

- camel-mock: stamp a global arrival index (component-wide monotonic
  counter) on every recorded exchange; expose per-endpoint arrival
  indices for assertion.
- camel-cli `test`: new top-level `sequence:` document grammar — a list
  of `mock:` endpoint refs in expected arrival order; evaluated
  post-settle as a filtered complete interleaving over the listed
  endpoints; failure is a verdict-class assertion (exit 1) naming the
  first divergence.
- Driver tests locking the divert-copy weave end to end, plus sequence
  pass/fail/error-family tests.
- Docs: testing guide gains the probe pattern and the `sequence:`
  reference; ADR-0072 gains a second dated amendment recording step 3's
  delivery shape.

Explicitly excluded:

- No new `probes:` grammar section — `divertCopyTo` IS the probe
  mechanism; one way to say one thing.
- No camel-test kit sugar API — a convenience wrapper would add a second
  path to the same capability; the document grammar suffices.
- `skipTo` (mutating interception) is untouched. The ADR-0064 §5 gate is
  clarified, not relaxed: intercepts exist only in the test document,
  never in the production route DSL.
- No itest/scenario-tier changes; no camel-matchers changes.

## Acceptance criteria

- Two causally-ordered divert probes asserted via `sequence:` pass, and
  the reversed declaration fails with exit 1 naming the first divergence.
- The divert-copy weave (copy recorded AND real send proceeds) is locked
  by a driver test.
- Global arrival indices are strictly increasing across concurrent
  endpoints; per-endpoint record order is preserved.
- `sequence:` grammar errors (single entry, non-`mock:` ref) fail as
  document errors (exit 2); `camel run` ignores the block.
- All repo quality gates pass; no behavior change for documents without
  `sequence:`.

## Risk budget

- The mock's record path gains one atomic increment — acceptable.
- `camel-test/src/lib.rs` is shared with wave E's pending rc-5yon
  (staged listeners) — different regions; a trivial rebase for whoever
  merges second. This change touches it only if re-export surface grows
  (it need not).
- Sequence semantics under concurrency are documented as
  happened-order-only: we assert the recorded interleaving, and test
  authors are told to assert order only between causally-ordered sends.
  No runtime enforcement of causal safety.
