# split-aggregate scenario

Tier-2 split + aggregate scenario (OpenSpec change `bench-missing-cells`,
task 2.3 rust artifacts; task 2.4 registers the JVM artifacts and the
harness wiring). The route is the one-to-many EIP surface: a canonical
array is SPLIT into fragments, every fragment is aggregated back into
one collection under a constant correlation key, and the marker fires
only from the aggregator's completion path.

```
timer:bench?period=10&repeatCount=10000&delay=0
  -> set_body      (canonical array, exactly 591 bytes)
  -> BENCH_INPUT_SHA256 log
  -> unmarshal json   (Body::Text -> Body::Json: the parsed 100-item array)
  -> split            (json_array items, SEQUENTIAL — no parallel)
     -> to direct:agg-in   (per fragment, "b0".."b99")

direct:agg-in
  -> set_header  (bench.correlation = "bench" — constant correlation key)
  -> aggregate   (completion_size=100, force_completion_on_stop=false,
                  strategy collect_all — the list-append strategy)
  -> completion assert  (collection length == 100 AND consistent with
                         CamelAggregatedSize; set bench.aggregated.size = 100)
  -> marker      BENCH_ROUTE_READY items=100
```

Assert failure kills the route BEFORE the marker — a missing or wrong
marker means the cell failed.

## Canonical array body

Inline in every fixture (deterministic, fixed): a JSON array whose 100
items are the strings `b0` through `b99` — item `i` is `"b" + i` —
serialized compactly (no whitespace):

```
["b0","b1",...,"b99"]        # exactly 591 bytes
```

| bytes | SHA-256 (tick-independent) |
|---:|---|
| 591 | `123444b475c48473309ed966eb69896c6725429021a5a5d2e0eaa0a77a159316` |

Every artifact logs `BENCH_INPUT_SHA256=<digest>` before splitting; a
run's input bytes are verified post-hoc against the table. The digest
is a pure function of the fixed array — inputs NEVER differ between
contenders. The rust fixture pins the literal in
`split_aggregate_array_golden`; JVM artifacts (task 2.4) carry a
golden-literal test of their own.

## Completion mechanism (pinned)

The aggregator completes when the bucket holds exactly 100 fragments
(`completion_size`). No timeout and `force_completion_on_stop=false`:
an incomplete bucket has NO completion path at all, and a pending
bucket is dropped (not flushed) on stop.

The completion payload is the `tower::Service<Exchange>` response of
`camel_processor::AggregatorService::call`
(`crates/camel-processor/src/aggregator.rs`):

- COMPLETED bucket → aggregated `Exchange` with body
  `Body::Json(Value::Array)` (`AggregationStrategy::CollectAll`
  appends every fragment body into one JSON array) and the property
  `CamelAggregatedSize` (bucket length, JSON u64).
- PENDING bucket → sentinel `Exchange` with property
  `CamelAggregatorPending=true` and an empty body.

Because this aggregate compiles as a plain mid-route step, the pending
sentinel flows through the SAME subsequent steps as the completion.
Every completion step therefore guards on the properties:

- lib: `completion_assert` returns the sentinel untouched when
  `CamelAggregatorPending` is set, else asserts collection length 100
  (and consistency with `CamelAggregatedSize`) and sets
  `bench.aggregated.size = 100`; `emit_ready_marker` logs ONLY when
  that property is present with value 100.
- CLI: a `filter` (rhai `property("CamelAggregatedSize") == 100`)
  gates the completion steps — a js script asserts the aggregated
  array length (rhai reads text bodies only), `set_property` stamps
  `bench.aggregated.size = 100`, and the marker log interpolates the
  property (`${exchangeProperty.bench.aggregated.size}`).

An incomplete bucket can never produce the marker — pinned by the
`incomplete_bucket_no_completion` unit test (99 fragments through the
real `direct:agg-in` consumer, 500 ms await window, no completion, no
marker in the captured log).

## Artifacts

| artifact | path | pair |
|---|---|---|
| `split-aggregate` (embedded lib) | `rust-camel-lib/` — programmatic routes, closure completion assert | A |
| `split-aggregate` (CLI + YAML) | `rust-camel-cli/` — `camel run` + YAML routes | B |

Pair A exercises the public `CamelContext` + `RouteBuilder` API (no
route-file parsing); Pair B pays the CLI bootstrap + YAML parse for an
identical route topology. No template tokens: the canonical array is
fixed, so both smoke runs use the route file as-is.

## Smoke

```
bash benchmarks/scenarios/split-aggregate/smoke/run.sh
```

Builds the lib fixture (debug) and `camel` (release, reused if
present), runs each artifact, and requires per artifact: exactly one
`BENCH_ROUTE_READY items=100` and exactly one
`BENCH_INPUT_SHA256=123444b4...`. All kills are PID-scoped (no wide
pkill). Exit 0 on full pass.

## Scope

Six artifact fixtures (rust-camel-lib, rust-camel-cli, camel-standalone
dsl+yaml, camel-quarkus dsl+yaml-native pair) exercising the one-to-many
EIP surface (split -> direct -> aggregate completion_size=100). The
scenario is registered in the harness (`SCENARIO_MARKER`,
`SCENARIO_M2_PROTOCOL` = B) and measured like any T-family cell; local
smoke proves the rust pair's marker contract, JVM digests are pinned by
CanonicalArrayTest per family and re-verified in the runner container
(bd rc-f4po).


## DSL/runtime gotchas (discovered during bring-up)
- **Correlation pairing asymmetry (deliberate)**: Pair A (Java DSL)
  correlates on `header(BENCH_CORRELATION_HEADER)` — setHeader is
  load-bearing; Pair B (YAML) correlates via `correlationExpression
  constant` — setHeader is inert there but kept so the step sequences
  stay identical. One correlation class either way: a single bucket of
  100 fragments.


- **Silent zero-fragment split**: `camel-api/src/splitter.rs` returns an
  empty fragment list for any non-`Body::Json(Array)` body — a split on
  `Body::Text` is a silent no-op with success semantics. Both fixtures
  `unmarshal: json` before splitting (lib `main.rs`, CLI route step 1).
  Filed as bd (usability bug, discovered-from rc-fz2).
- **`header` vs `correlation_key`**: the aggregate DSL requires `header`
  and correlates by it on the builder path; `correlation_key` is the
  optional canonical-path spelling. The CLI route carries both with the
  same constant.
