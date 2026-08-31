T2 t2-json fixture for the `node-native` contender: a zero-dependency
Node script (ESM, no `package.json` — the committed script IS the
artifact) implementing the scenario's protocol-B contract. It reads
`BENCH_PAYLOAD_BYTES` (default 32768, validated against the payload
axis 1024|32768|262144|1048576; invalid values abort before any
marker), builds the canonical JSON document of exactly SIZE bytes
(`{"id":"bench","seq":0,"fill":"<K×'b'>"}` — the JS port of
bench-loadgen's `payload::canonical_json_body`, tick = 0), logs
`BENCH_INPUT_SHA256=<digest>` (must equal the scenario's committed
golden — INPUT parity across every contender), then performs the same
route as every fixture: unmarshal (`JSON.parse`) → filter
(`id == "bench"`) → transform (insert `"bench": true` into the parsed
map) → marshal (the single serialization, `JSON.stringify`) → output
assert (exact SIZE+13 length AND parsed semantic equality) → marker
`BENCH_ROUTE_READY bytes=<len>` exactly once. The +13 delta is exactly
the inserted `,"bench":true` member; an assert failure exits non-zero
BEFORE the marker, so a missing or wrong marker means the cell failed.
INPUT parity is what the digest proves — cross-runtime OUTPUT
byte-parity is NOT claimed (serializers differ in member order; see
the scenario README caveat). The route fires once
(`repeatCount=1&delay=0` semantics); the script then idles like the
rust fixture until the smoke/harness kills it. Run standalone:
`BENCH_PAYLOAD_BYTES=32768 node route.mjs`.
