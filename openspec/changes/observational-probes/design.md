# Design: observational-probes

## Context and honest starting point

Step 3 of ADR-0072's staged direction. The recon corrected the scope the
way step 2's did: the observational weave already exists and is specified
(`Declarative intercept application` requirement: `divertCopyTo` delivers
a pre-send copy while the real send continues; camel-core composes the
copy stage at `step_compilers/endpoints.rs`; `InterceptRules` validates
targets; the runner applies rules before route registration per the
Stage A freeze). The genuine gaps are the arrival-order assertion, the
missing driver lock on the weave, and author documentation.

## Decision 1 — the probe mechanism is `divertCopyTo`, unchanged

No `probes:` grammar, no new core surface. A probe is a mock endpoint
that receives a divert copy of a real send. Rationale: the grammar,
validation (source must not be `mock:`, target must be `mock:` with a
non-empty path, exactly one of `skipTo`/`divertCopyTo`), and runtime
composition all exist and are canon-specified. A parallel declaration
form would be a second way to say one thing (single-way principle,
ADR-0072 per-tier grammar clause).

## Decision 2 — global arrival index on every recorded exchange

`MockComponent` owns a component-wide monotonic counter
(`Arc<AtomicU64>` in the component state, shared into each endpoint
inner at creation). The record path fetches-and-increments once per
arrival AND pushes the index into a per-endpoint `Vec<u64>` while
holding the per-endpoint `received` lock, in the same critical section
as the exchange push — per-endpoint index order then holds by
construction, and cross-endpoint order is unaffected (the merge sorts
by index). Retention is bounded (`max_retained`, default 10 000,
`retain=N` override): the truncation branch pops the paired index in
lockstep with the exchange pop, inside the same lock. `sequence:` thus
observes retained arrivals only — consistent with step-2 `CountBound`,
which samples the same retained state. Public accessor returns the
ordered index list (and count-paired snapshots where needed).

- `get_received_exchanges()` stays `Vec<Exchange>` — zero breakage to
  the step-2 expectation surface.
- `reset()` (the existing clear path — no second clearing method)
  clears per-endpoint vectors; the global counter stays monotonic
  (post-reset arrivals keep increasing — stable semantics, no index
  reuse).
- One `fetch_add(1, Relaxed)` per record: contention-free in practice
  (mock endpoints are test apparatus), negligible cost.

## Decision 3 — `sequence:` grammar and semantics

Top-level document key: a list of `mock:` refs (normalized to bare
endpoint names exactly like `expects` keys; duplicates allowed — the
same endpoint may appear for consecutive arrivals; minimum two entries —
a single-entry sequence is a count assertion in disguise and is rejected
as a document error).

Semantics — filtered complete interleaving: collect every arrival at the
LISTED endpoints (each as `(global_index, endpoint_name)`), order by
global index, and require the resulting projection to equal the declared
list exactly. Arrivals at unlisted endpoints are ignored, so authors can
narrow the assertion to the probes they care about while `expects`
handles the rest.

Evaluation point: post-settle, after per-endpoint expectation
evaluation, same assertion phase (quiescence model from the step-2
design — no polling). Failure is verdict-class (exit 1) with a message
naming the first divergence: position, expected name, actual name
(when the projection ran out of arrivals, the message says so).

Error family (exit 2, document errors, existing style):

- `SequenceTooShort` — fewer than two entries.
- `SequenceBadRef` — an entry lacking the `mock:` scheme or naming an
  empty endpoint path.

`camel run` SHALL NOT read the block (non-interference clause).

## Decision 4 — concurrency honesty

Cross-endpoint order is meaningful only between causally-ordered sends.
Two branches racing to different probes produce a happened-order the
assertion will faithfully report — nondeterministically. The testing
guide states this plainly: assert `sequence:` only over sends ordered by
the route (sequential steps, reply chains). No runtime enforcement; the
settle window guarantees quiescence, not causality.

## Decision 5 — second ADR-0072 amendment

A dated amendment records: step 3 delivered as divert-copy probes plus
the arrival-sequence assertion; the Context's probe framing was again
wider than the landed need (the weave predated the ADR — 2026-08-23
declarative-intercepts change); and the ADR-0064 §5 gate for mutating
weaving reads precisely as: `skipTo` exists only as a test-document
construct and never in the production route DSL.

## Decision 6 — wave E coexistence

Fence (from bd rc-fw67o): no itest, no camel-cli outside
`src/commands/test/**`, no camel-component-http, no camel-matchers
edits, camel-test untouched unless a re-export is demanded during
review. Overlap with rc-5yon is confined to `camel-test/src/lib.rs`
regions we do not plan to touch.

## Trade-offs

- Optimized for: minimal new surface (one grammar key, one counter, one
  accessor), spec honesty, zero breakage of the step-2 assertion
  surface.
- Deprioritized: kit-sugar ergonomics (documented pattern instead),
  causal-safety enforcement (guidance instead), sequence wildcards or
  partial-order forms (YAGNI — add when a real test needs them).
