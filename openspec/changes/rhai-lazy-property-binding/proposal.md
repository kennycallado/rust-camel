# Proposal: rhai-lazy-property-binding

## Why

Downstream `camel-cache` measured a 2.7-3.5x regression from 0.56 to 0.57 on a
while-loop that holds multi-thousand-entry maps/arrays in exchange properties
and touches one 128-key chunk per iteration (1000 members 256ms -> 691ms;
1200: 628ms -> 1762ms; 5001: 8.3s -> 29.2s; bd rc-m01r9).

The mission premise ("0.56 was O(1) string handoff, 0.57 went eager") was
refuted by the mandatory e_opus pre-flight consultation: v0.56.0 also
deep-converted every property through `json_to_dynamic` on every evaluation
(verified against `git show v0.56.0:crates/languages/camel-language-rhai/src/lib.rs`).
The regression is a constant factor added by 0.57 code:

1. The write-back change detection evaluates script code per scalar leaf:
   `scalars_equal` (transaction.rs) compiles and evaluates `"a == b"` through a
   rhai `Engine` for every scalar leaf of every container, so diffing a
   multi-thousand-entry property costs hundreds of thousands of script
   compilations per mutating evaluation.
2. Extra deep `rhai::Map` clones per evaluation: the read path clones each map
   twice (engine + presence readers), the mutating path three times (scope,
   readers, presence).

True subtree-level laziness was ruled NOT achievable in rhai 1.27 without a
semantic compromise (no lazy-node hook on `rhai::Map`; a custom-type wrapper
breaks dot access, `type_of`, `is::map`, and the Map method surface; follow-up
bd rc-telbk). The complete fix for the measured regression is to remove the
0.57-added overheads so each evaluation pays conversion once per value, not a
script compile per leaf.

## What Changes

- Replace script-code leaf comparison in the write-back diff with native,
  type-matched comparison (same results for Bool/Int/Float/Char/String;
  NaN still differs; `Other` stays conservatively "differs").
- Hold each snapshot map once: readers share one snapshot instead of cloning
  per reader registration; scope and presence readers stop duplicating maps.
- No cache, no camel-core change, no semantic movement: same values, types,
  errors, error order, write-back deltas, and handles for untouched entries.
- Spec delta on `language-value-boundary`: per-evaluation conversion and
  native-comparison requirements (canonical scenario names kept verbatim).
- Docs: one-line rule on the Rhai property-binding page; one `perf` entry in
  `CHANGELOG.md`.

Explicitly excluded: true lazy conversion (bd rc-telbk), any cache keyed on
property identity, changes to `camel-language-simple`, MiniJinja, JavaScript,
or the JSON-faithful boundary.

## Acceptance criteria

- Write-back diff never evaluates script code (test-only counter; zero for a
  mutating script writing one key of a property with 10,000 scalar leaves).
- Each evaluation converts each bound value exactly once (node-visit counter
  equals total node count; not 2x or 3x).
- Repeated evaluations do not accumulate copies (N=5: counter is exactly 5x
  the per-eval count; at most one working copy and one snapshot per container
  per evaluation).
- Regression guards stay green: chunk read returns native map, write-back
  lands exactly the mutated key, `(i64::MAX, u64::MAX]` integers keep the
  `json number` binding, read-only surface guards still reject mutation.
- Benchmark reproduction of the downstream workload reaches 0.56-level timing
  within 10% (manual gate, no wall-clock assertions in CI).

## Risk budget

Acceptable: internal-only refactors of the rhai crate's snapshot/diff plumbing
with identical observable behavior. Out of bounds: semantic movement, lossy
conversion, cache invalidation complexity, camel-core API changes, wall-clock
CI assertions.

Affected crates: `camel-language-rhai`. Bd: rc-m01r9, rc-telbk (follow-up).
