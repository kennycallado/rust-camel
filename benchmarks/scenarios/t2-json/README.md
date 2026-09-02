# t2-json scenario

Tier-2 JSON transform scenario (OpenSpec change `bench-missing-cells`,
tasks 2.1 rust artifacts / 2.2 JVM artifacts). The route is the T2
"realistic EIP" shape applied to the canonical JSON document:

```
timer:bench?period=10&repeatCount=10000&delay=0
  -> set_body      (canonical JSON document, exactly SIZE bytes)
  -> BENCH_INPUT_SHA256 log
  -> unmarshal json   (Body::Text -> Body::Json: the body IS the parsed value)
  -> filter           (jsonpath / id == "bench")
  -> transform        (insert "bench": true into the PARSED map; return the map)
  -> marshal json     (the SINGLE serialization: serde_json::to_string)
  -> output assert    (exact SIZE+13 length AND parsed semantic equality)
  -> marker           BENCH_ROUTE_READY bytes=<len>
```

The +13 output delta is exactly the inserted `,"bench":true` member.
Assert failure (length or semantics) kills the route BEFORE the marker —
a missing or wrong marker means the cell failed.

## Canonical input formula

`{"id":"bench","seq":<tick>,"fill":"<K×'b'>"}` — UTF-8, zero whitespace,
fixed field order `id`,`seq`,`fill`, `<tick>` as unpadded decimal, and
`K = SIZE - overhead` where overhead = prefix (20) + tick digits + infix
(9) + suffix (2), so the serialized document is EXACTLY `SIZE` bytes.
Self-test tick is `0`. Implemented once in `bench-loadgen`'s
`payload` module (`canonical_json_body`, `canonical_body_sha256`); every
fixture — rust or JVM — consumes that contract (JVM fixtures carry a
byte-identical Java port plus golden-literal tests).

`BENCH_PAYLOAD_BYTES` (env) selects the size, default 32768, validated
against the payload axis: `1024 | 32768 | 262144 | 1048576`.

## Golden digests (SHA-256, tick in parentheses)

| size | tick | digest |
|---:|---:|---|
| 1024 | 0 | `5abe5f00068356cad4e72f4d5e5e0a5d15d4a5cc9df8d0f22e22bf1448891b0f` |
| 32768 | 0 | `a0db69e1146a29b0b25ca22435e51f39e271ecb1ac4ec1cee0ead3212eae10e9` |
| 262144 | 0 | `02adf20f21dc63217c9dc2e26b82101f96dbf311af5fbbf86e818e63d7171e27` |
| 1048576 | 0 | `9d4da9b244b6d12bed15d624ce426099da3126422285ecc584b9d3fff93a3abd` |
| 32768 | 7 | `995f33e2cb370cdd8179ca80a49f921ec48af1d6558ee23f5b98d8e67624f1f8` |

Every artifact logs `BENCH_INPUT_SHA256=<digest>` for its (size, tick)
before processing; a run's input bytes are verified post-hoc against the
table. The digest is a pure function of (size, tick) — inputs NEVER
differ between contenders.

## Rust artifacts

| artifact | path | pair |
|---|---|---|
| `t2-json` (embedded lib) | `rust-camel-lib/` — programmatic route, closure filter/transform | A |
| `t2-json` (CLI + YAML) | `rust-camel-cli/` — `camel run` + templated YAML route | B |

### Transform mechanism (pinned)

The transform operates on the PARSED structured body and returns the
MAP — `marshal("json")` is the only serializer. A script that
re-serializes internally would make the downstream marshal double-encode
(or no-op on text) — forbidden.

Language reality in this codebase (recorded by task 2.1): rhai binds
`body` as TEXT ONLY, so after `unmarshal("json")` (body =
`Body::Json(serde_json::Value)`) rhai would see `""`. Hence:

- **lib (Pair A):** Rust closures — the same idiomatic-surface deviation
  `t2-realistic-eip` documents. Filter closure reads the structured
  value; `map_body` inserts the member; `marshal("json")` serializes.
- **CLI (Pair B):** the transform step uses `language: js`
  (`camel.body` exposes the structured body; the expression inserts
  `bench = true` and returns the map, lowered to `Body::Json`). rhai
  remains in the TEXT domains: input build/assert before unmarshal and
  output assert after marshal. In-route asserts use `script:` steps —
  their throws propagate as route errors; expression-position throws
  are swallowed to a null body by the compiler and are not trusted for
  enforcement.

### CLI template tokens

`routes/t2-json.yaml` carries TWO tokens and is never run directly —
smoke/harness performs a TEMPLATE COPY:

```bash
sed -e "s/SIZE/${BENCH_PAYLOAD_BYTES:-32768}/g" \
    -e "s/GOLDEN/<(size,0) digest from the table>/g" \
    routes/t2-json.yaml > /tmp/t2-json.yaml
camel run --config Camel.toml --routes /tmp/t2-json.yaml --no-watch
```

## Caveat: JVM output field order (task 2.2)

JVM JSON serializers may emit object members in a different order than
serde_json. Inputs never differ (golden table), and OUTPUTS are
compared by exact LENGTH + PARSED SEMANTICS ONLY — never by output byte
equality or digest across runtimes.

## Smoke

```bash
bash benchmarks/scenarios/t2-json/smoke/run.sh
```

Runs the lib fixture at `BENCH_PAYLOAD_BYTES` (default 32768 → exactly
one `BENCH_ROUTE_READY bytes=32781`), the CLI artifact at 32768 via
template copy (same greps + identical input digests), and the lib at
1024 (per-class marker `bytes=1037`). Exit nonzero on any miss.
