# Design: bench-missing-cells

## Approach

Five independent extensions plus their shared equivalence principle —
six design decisions in all — anchored on one rule:
**contenders stay byte-equivalent** — every payload the axis introduces is
built identically (same canonical bytes, golden SHA-256-verified) in every
artifact fixture, with a fixture-side size assert before the marker fires.

Glossary (benchmarks/CONTEXT.md §1): **3 contenders** (rust-camel,
camel-standalone, camel-quarkus) expanding into **6 artifact fixtures**
per scenario (rust-lib, rust-cli, standalone-dsl, standalone-yaml,
quarkus-dsl-native, quarkus-yaml-native) in 2 fair pairings. This change
uses that canonical framing throughout.

**D1 — Payload axis (harness), two payload kinds.**
*Transport payloads* (T3 http-server): `bench-loadgen` gains
`--payload-size <bytes>` on `measure-a` and `measure-throughput`; body =
`0x62 'b'` repeated, truncated to size — deterministic, no RNG. This axis
measures HTTP/body-buffer scaling (reqwest/hyper copies) with compression
disabled (no content-encoding today; stated in the report), NOT
entropy-sensitive application processing. Only sizes {1024, 32768,
262144, 1048576} are valid; other values are rejected with a usage error.
*Structured payloads* (Protocol-B scenarios): timer-driven fixtures read
`BENCH_PAYLOAD_BYTES` (run.sh forwards env) and build the **canonical
JSON document** defined in D2 — byte-identical across runtimes, verified
by a golden SHA-256 logged as `BENCH_INPUT_SHA256=<hex>` before the ready
marker (harness goldens per class; unit tests in each Rust fixture).

**D2 — t2-json scenario.** Timer-driven Protocol B (T2 family). Route:
`set_body($canonical_json)` → `unmarshal("json")` → `filter(jsonpath
$.id == 'bench')` → `transform` (append field `"bench": true`) →
`marshal("json")` → marker. Canonical input per class, UTF-8, zero
whitespace, fixed field order `id`,`seq`,`fill`:
`{"id":"bench","seq":<tick>,"fill":"<K×'b'>"}` where `<tick>` is the
unpadded decimal tick number and `K = size − overhead(size, tick)` makes
the serialized document exactly the class size (1024/32768/262144/1048576
bytes). Rust builds it with `format!`; the CLI YAML fixture builds it with
a rhai `source` expression producing the same string; Java fixtures
(standalone, quarkus) use String.format with the same formula. Output
verification: **exact output length** (asserted in-fixture) plus **parsed
semantic equality** (parsed JSON has id="bench", original seq, fill, and
the appended field) — byte equality is NOT required on output because JVM
serde may reorder fields; the caveat applies to output bytes only, never
inputs (inputs carry the golden digest). Golden digests are keyed by
**(size, tick)** — tick=0 is the canonical self-test value; the loadgen
golden table covers (1024,0), (32768,0), (262144,0), (1048576,0). The
transform appends `,"bench":true` before the closing brace, so the exact
expected output length is **input_size + 13 bytes**, asserted in-fixture.

**D3 — split-aggregate scenario.** Timer-driven Protocol B, one cycle per
measurement. Two routes connected through `direct:` (a single
split→aggregate chain does NOT work — split already fans-in, so a
downstream aggregate would see one exchange and never complete):
- Route "outer": `timer:bench?repeatCount=1` → `set_body(canonical array
  of 100 items)` → `split(jsonpath $, sequential)` → each fragment sent
  to `direct:agg-in` (100 correlated fragment exchanges).
- Route "agg": `from("direct:agg-in")` → `set_header(correlation const)`
  → `aggregate(completion_size=100, force_completion_on_stop=false)` →
  on completion: assert collection size == 100 → marker
  `BENCH_ROUTE_READY items=100`.
Marker fires only on true bucket completion; an incomplete bucket emits
NO marker (the cell fails by the 30 s marker deadline — the desired
failure semantics). Camel equivalents use Splitter → direct →
Aggregator(constant correlation, completionSize=100). The exact
rust-camel mechanism for asserting the aggregated size at completion
(exchange property vs aggregate outcome) is pinned in the task from the
aggregator API (crates/camel-processor/src/aggregator.rs).

**D4 — Ratio CIs (M3 only).** New `aggregate-ratios` loadgen subcommand.
Inputs: two `m3-summary.json` paths. Validation gate — both summaries
must come from the SAME run (identical `measurement_order.json`
provenance identity), metric M3, equal round count, round indices
0..n−1; any mismatch is a hard error with the reason. Statistic: ratio of
medians-of-round-means. CI: paired bootstrap resampling round indices
JOINTLY (cells measured within the same randomized block share round
conditions) reusing the SplitMix64/BCa machinery, n_resamples=2000,
seeded → deterministic across invocations. Output:
`RATIO <cellA>/<cellB> point=<x.xxxx> lo=<x.xxxx> hi=<x.xxxx>`. An
`--independent` mode (unpaired resampling of each cell's round indices
separately, same seed determinism) is provided for cross-run
comparisons where same-run validation cannot hold — it prints an
`UNPAIRED` tag on the output line.

**D5 — Metric-family overhead A/B (lever study).** Contender: rust-cli
(T3 http-server) only. **Backend held constant**: both arms register the
Prometheus exporter ([observability.prometheus] present in both
Camel.tomls, same free port). Arm A: `[observability.metrics]
enabled=true, exchange=true, duration=true, components=true` (maximal
per-request family emission). Arm B: `[observability.metrics]
enabled=false` (master gate off; the non-disableable camel_errors_total
family still fires per ADR-0066 D5). M3, **5 rounds × 30 s** per arm (three observations cannot support a
credible BCa interval). The two arms are separate runs, so the ratio is
computed with `aggregate-ratios --independent` (unpaired, `UNPAIRED`
tagged — cross-run pairing would be dishonest), labeled **lever study,
not a contender row**. The separate question
"registered-collector vs NoOp-seed cost" is OUT of scope here (would be a
backend-cost study; noted in COVERAGE if asked).

**D6 — CI bench subset.** New `bench-smoke` job in ci.yml, ubuntu only,
`timeout-minutes: 10`: `cargo bench -p camel-bench --bench pipeline
--bench body_coercion -- --quick`. (Criterion bench targets are NOT
compiled by `cargo build --workspace` — the job compiles and runs them
itself.) The quick-mode command is FINAL
(`cargo bench -p camel-bench --bench pipeline --bench body_coercion --
--quick`); any argument rejection in this repo's criterion version fails
the CI task — no silent adaptation. Container
full-matrix stays manual (job comment says so).

## Affected crates

- `benchmarks/harness/loadgen`: `--payload-size` (cli.rs,
  cli_runtime.rs body builder + unit tests with golden digests),
  `aggregate-ratios` subcommand (new module reusing bca.rs).
- `benchmarks/scenarios/t2-json/` (new): 6 artifact fixtures (3
  contenders × 2 pairings; quarkus natives share JVM sibling src per
  suite convention).
- `benchmarks/scenarios/split-aggregate/` (new): 6 artifact fixtures.
- `benchmarks/scenarios/http-server/`: metrics-off Camel.toml variant
  (lever arm B).
- `benchmarks/harness/run.sh`: `BENCH_PAYLOAD_BYTES` forwarding; marker +
  protocol-map registration for both new scenarios (registered with the
  fixtures, phase 2 — not with the harness phase).
- `.github/workflows/ci.yml`: `bench-smoke` job.
- `benchmarks/COVERAGE.md`, `docs/benchmarks/`: matrix + addendum.

## Architecture boundaries

No production crate changes. The suite lives outside the hexagon
(benchmarks/ is not shipped); fixtures consume public builder/DSL APIs
exactly as users do (camel-builder marshal/unmarshal, camel-dsl YAML
split/aggregate). ADR-0066 (metrics levers) is consumed, not amended.
ADR-0033 exec-stub pattern reused for new CLI fixtures' Camel.toml.

## Phases

Ordered delivery phases; each independently verifiable:

1. **Harness extension** — transport payload axis + aggregate-ratios +
   canonical-body unit tests (pure loadgen, no fixtures, no run.sh
   scenario registration).
2. **New scenarios** — t2-json + split-aggregate: 6 artifact fixtures
   each, canonical JSON builders with golden digests, marker-contract
   green via local reduced runs, run.sh registration (marker + protocol
   maps).
3. **Lever study + CI + publication** — gauge A/B runs (5×30 s arms),
   bench-smoke job with local argument validation, COVERAGE.md update,
   v4 addendum with ratio CIs from published data.
