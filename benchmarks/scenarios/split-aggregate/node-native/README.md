T2 split-aggregate fixture for the `node-native` contender: a
zero-dependency Node script (ESM, no `package.json` — the committed
script IS the artifact) implementing the scenario's protocol-B
contract. The input is FIXED: the canonical array body of exactly 591
bytes — a compact JSON array whose 100 items are the strings `b0`
through `b99` (item `i` is `"b" + i`), the JS port of bench-loadgen's
`payload::canonical_split_aggregate_array`. Unlike t2-json there is no
env contract: `BENCH_PAYLOAD_BYTES` is ignored here exactly as the rust
fixture ignores it (the array is size- and tick-independent). The
fixture logs `BENCH_INPUT_SHA256=<digest>` (must equal the scenario's
committed golden `123444b4…` — INPUT parity across every contender),
unmarshals (`JSON.parse`), then splits SEQUENTIALLY: one fragment after
another, each dispatched to `direct:agg-in` and its response fully
processed before the next dispatch (`parallel(false)` semantics — the
`await` in the split loop IS the split scope). The aggregation route is
hand-rolled in ~30 lines of plain JS — correlation buckets keyed by the
constant `bench.correlation` header, `CollectAll` list-append,
completion at exactly 100 fragments, and the pending sentinel
(`CamelAggregatorPending=true`, empty body) for fragments 1..99; this
hand-rolled coordination is the honest point of the contender, so no
orchestration library and zero dependencies. The completion assert
verifies the collection length (100) AND consistency with
`CamelAggregatedSize`, stamps `bench.aggregated.size = 100`, and the
marker `BENCH_ROUTE_READY items=100` fires ONLY from that path, exactly
once; sentinels flow through the same guard steps and never emit. An
assert failure exits non-zero BEFORE the marker, so a missing or wrong
marker means the cell failed. The route fires once
(`repeatCount=1&delay=0` semantics); the script then idles like the
rust fixture until the smoke/harness kills it. Run standalone:
`node route.mjs`.
