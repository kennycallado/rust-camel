# Design: rhai-lazy-property-binding

## Approach

e_opus pre-flight ruling (mission 498, embedded in the park): true subtree
laziness is NOT achievable in rhai 1.27 without breaking the frozen script
surface (`rhai::Map` is a concrete `BTreeMap` with no lazy-node hook; a
custom-type wrapper breaks dot access, `type_of`, `is::map`, and the Map
method surface — follow-up bd rc-telbk). v0.56.0 was verified eager too, so
the complete fix for the measured regression is to remove the constant
overheads 0.57 added. Two mechanisms:

1. **Native leaf comparison in the write-back diff.** `scalars_equal`
   (src/transaction.rs) currently compiles and evaluates the script
   expression `"a == b"` through a rhai `Engine` for every scalar leaf of
   every container — hundreds of thousands of compilations per mutating
   evaluation on a multi-thousand-entry property. Replace it with direct
   kind-matched native comparison: `as_bool()`/`as_int()`/`as_float()`/
   `as_char()`/`as_str()` downcasts with `==`. `kind()` already guarantees
   both sides share the same kind before the scalar arm, so results are
   identical: same-type natives compare by value, NaN still counts as
   "differs" (`f64: PartialEq` is false for NaN, same as the engine's `==`),
   and a failed downcast stays conservatively "differs". The diff engine
   parameter disappears from `rhai_values_differ`/`map_deltas`; the frozen
   comparison `Engine` (and its construction) is deleted. No engine eval
   remains anywhere in change detection — asserted by counter test.

2. **One snapshot per container, shared by `Arc`.** Today each snapshot
   `rhai::Map` is deep cloned twice on the read path (once for the eval
   engine's reader maps, once for presence readers inside
   `register_snapshot_readers`) and three times on the mutating path (scope
   push, reader registration, presence). The mechanism is fixed, not
   either/or: `register_snapshot_readers` takes `Arc<rhai::Map>` per
   container; the value readers (`header(name)`/`property(name)`) clone the
   requested ENTRY out of the shared snapshot on call, and the presence
   readers do `contains_key` on the shared snapshot — registration performs
   zero deep copies (only `Arc` refcount bumps). The scope receives exactly
   one deep clone per container: the working copy the script mutates in
   place (an `Arc`-shared scope value would break reader semantics and the
   pre/post diff, so it is deliberately a real clone). Read path: one build,
   one working copy. Mutating path: one pre snapshot build, one working
   copy; the write-back diff compares the shared pre snapshot against the
   post-eval scope maps, so the snapshot is reused by change detection
   without a further copy. No caller pre-clones any more: `eval_sync` and
   `eval_mut_sync` pass each snapshot map exactly once.

Invariants preserved (e_opus): values, types, errors, error precedence
(body -> header entry -> property entry), write-back deltas, handles of
untouched entries, `json number` binding for `(i64::MAX, u64::MAX]`,
read-only surface guards, snapshot-reader semantics (readers return pre-run
values after in-script assignment).

## Affected crates

- `camel-language-rhai`: transaction.rs (native comparison, drop diff
  engine), lib.rs (snapshot plumbing, reader construction), tests (counters
  + regression guards), docs page, CHANGELOG.md. No other crate.

## Architecture boundaries

Languages-boundary only. The `Exchange`, `Value`, and `Body` contracts in
camel-api/camel-language-api are untouched; other language crates are
untouched; the JSON-faithful boundary (rc-141om refinement) is untouched.
Reference: CONTEXT-MAP.md Languages bounded context; spec
`language-value-boundary` requirements "Structured values round-trip
natively" and "Unrepresentable values are refused" stay verbatim.

## Counter instrumentation (test-only)

Counters are `#[cfg(test)]` thread-local atomics. Tests call
`eval_sync`/`eval_mut_sync` DIRECTLY on the test thread (never through the
`spawn_blocking` wrapper) and assert deltas around the call, which isolates
each test from parallel test threads.

- `DIFF_ENGINE_EVALS`: incremented where the diff previously evaluated
  script code; the native comparison never increments it. Test asserts 0
  after a one-key mutation of a 10,000-leaf property (mutating path).
- `CONVERT_NODE_VISITS`: incremented per `json_to_dynamic` node. It measures
  the INBOUND JSON converter only: fixtures use `Body::Json` bodies and
  JSON-map headers/properties, and the expected count is computed from the
  JSON node count of those fixtures (`Text`/`Xml`/`Empty`/`Bytes`/`Stream`
  bodies bypass `json_to_dynamic` and are out of the counter's scope).
  Tests assert exact-once per eval (read path AND mutating path) and exact
  5x over N=5 repeated evaluations.
- `SNAPSHOT_WORKING_COPIES`: incremented in the single named helper that
  produces the scope working copy (`working_copy(map) -> rhai::Map`).
  This is the structural copy-bound assertion — `CONVERT_NODE_VISITS`
  cannot see clones. Tests assert: read path 1 per container; mutating path
  1 per container (the pre snapshot build is counted separately by
  `CONVERT_NODE_VISITS`); reader registration contributes zero deep copies
  (`Arc::strong_count` check on the shared snapshot).

Counters live behind `#[cfg(test)]` like the existing `snapshot_conversions`
pattern (lib.rs `counted_body_to_dynamic`); none exist in non-test builds.

## Phases

Single-phase change: one coherent slice in one crate — diff mechanism,
snapshot plumbing, counter tests, regression guards, docs line, changelog
entry. No externally-visible type surface moves.
