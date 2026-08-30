# Tasks: bench-missing-cells

## Phase 1: Harness extension

### Task 1.1: Transport payload axis in bench-loadgen

**Files:**
- `benchmarks/harness/loadgen/src/payload.rs` (new)
- `benchmarks/harness/loadgen/src/lib.rs` (modified — add `pub mod payload;`)
- `benchmarks/harness/loadgen/src/cli.rs` (modified — `--payload-size` flag on `measure-a` and `measure-throughput`)
- `benchmarks/harness/loadgen/src/cli_runtime.rs` (modified — body construction in `measure_a_async` (`:311`), `warmup_drive` (`:419`), throughput worker loop (`:523`); `payload_size_bytes` field in `ThroughputResult` and the M3 JSON output)
- `benchmarks/harness/loadgen/src/throughput.rs` (modified — `ThroughputResult` field)
- `benchmarks/harness/loadgen/Cargo.toml` (modified — add `sha2` dependency)

**Steps:**
1. Create `payload.rs` with `pub const VALID_PAYLOAD_SIZES: [usize; 4] = [1024, 32768, 262144, 1048576];`, `pub fn validate_payload_size(size: usize) -> Result<usize, String>` (error message names all four valid sizes), and `pub fn transport_body(size: usize) -> Vec<u8>` (returns `vec![b'b'; size]` — deterministic pattern fill, no RNG).
2. Add `sha2` to `[dependencies]` in loadgen `Cargo.toml` (workspace version pin style matching existing deps).
3. Wire `--payload-size <bytes>` into `cli.rs` for the `measure-a` and `measure-throughput` subcommands: parse, run through `validate_payload_size` (exit 2 with the usage error on invalid), pass to the runtime fns. Default when absent: 5-byte legacy `"bench"` body for throughput, `"ping"` for measure-a (exact current behavior unchanged).
4. In `cli_runtime.rs`, replace the hardcoded `.body("ping")` / `.body("bench")` call sites with a body selected by the optional payload size (None → legacy literals; Some(n) → `transport_body(n)`).
5. Add `payload_size_bytes: Option<u64>` to `ThroughputResult`; serialize it in the M3 JSON output (absent/null when legacy). Do NOT alter the pinned `BENCH_MEASURE_A_RESULT` stdout line format.

**Tests:**
- name: `transport_body_exact_sizes_and_goldens`
  setup: `payload.rs` exists with `transport_body`.
  action: build bodies for 1024, 32768, 262144, 1048576; check `len()`, that every byte is `b'b'`, and SHA-256 equals the four golden hex literals embedded in the test (worker generates them once via `python3 -c "import hashlib;print(hashlib.sha256(b'b'*N).hexdigest())"` and pastes).
  assert: all four sizes pass; digest mismatch fails with the size name.
  command: `cargo test -p bench-loadgen transport_body`
  expected: fails before implementation (module missing), passes after.
- name: `validate_payload_size_rejects_others`
  setup: `validate_payload_size` exists.
  action: call with 2048, 0, and 5000000.
  assert: each returns `Err` whose message contains all four valid sizes.
  command: `cargo test -p bench-loadgen validate_payload`
  expected: fails before, passes after.

**Acceptance:**
- `cargo test -p bench-loadgen --lib` green.
- `cargo run -p bench-loadgen -- measure-throughput --payload-size 2048 <url>` exits 2 naming the four sizes (verified manually in the task notes).
- `cargo fmt --check --all` and `cargo clippy -p bench-loadgen -- -D warnings` exit 0.

- [x] 1.1

### Task 1.2: Canonical JSON body builder + golden digests

**Files:**
- `benchmarks/harness/loadgen/src/payload.rs` (modified — add canonical JSON section)
- `benchmarks/harness/loadgen/src/lib.rs` (modified — re-export if needed)

**Steps:**
1. Add to `payload.rs`: `pub fn canonical_json_body(size: usize, tick: u64) -> String` building `{"id":"bench","seq":<tick>,"fill":"<K×'b'>"}` — UTF-8, zero whitespace, field order `id`,`seq`,`fill`, `<tick>` unpadded decimal; `K = size - overhead` where `overhead = "{\"id\":\"bench\",\"seq\":".len() + digits(tick) + ",\"fill\":\"".len() + "\"}".len()` so the serialized document is exactly `size` bytes.
2. Add `pub fn canonical_body_sha256(size: usize, tick: u64) -> String` (hex digest of the canonical body) and `pub const CANONICAL_SELFTEST_TICK: u64 = 0;`.
3. Export a golden table as a test-only constant list of `(size, tick, expected_sha256_hex)` for (1024,0), (32768,0), (262144,0), (1048576,0) plus one non-trivial tick (32768,7) — digests generated once by the worker (python3 hashlib over the exact formula) and pasted as literals.

**Tests:**
- name: `canonical_json_exact_sizes_and_digests`
  setup: builder exists.
  action: for each golden entry, build the body; assert `len() == size`, prefix `{"id":"bench","seq":` , all `fill` bytes are `b'b'`, and SHA-256 equals the golden hex.
  assert: all entries pass; any mismatch names the (size, tick) pair.
  command: `cargo test -p bench-loadgen canonical_json`
  expected: fails before, passes after.
- name: `canonical_json_k_formula_exactness`
  setup: builder exists.
  action: build (1048576, 123456789) and assert exact length 1048576 (exercises multi-digit tick arithmetic).
  assert: passes.
  command: `cargo test -p bench-loadgen canonical_json_k_formula`
  expected: fails before, passes after.

**Acceptance:**
- `cargo test -p bench-loadgen --lib` green including both new tests.
- `cargo fmt --check --all` and `cargo clippy -p bench-loadgen -- -D warnings` exit 0.

- [x] 1.2

### Task 1.3: aggregate-ratios subcommand

**Files:**
- `benchmarks/harness/loadgen/src/ratios.rs` (new)
- `benchmarks/harness/loadgen/src/lib.rs` (modified — `pub mod ratios;`)
- `benchmarks/harness/loadgen/src/cli.rs` (modified — `aggregate-ratios <cellA.json> <cellB.json> [--seed N] [--bci-resamples N] [--independent]` dispatch)
- `benchmarks/harness/loadgen/src/main.rs` (modified — subcommand entry)

**Steps:**
1. `ratios.rs`: `struct M3Summary { rounds: usize, per_round_means: Vec<f64>, run_id: String }` parser for m3-summary.json; `run_id` = the RUN-ROOT directory name (the summary file's parent's parent — published layout is `<run-root>/<cell-dir>/m3-summary.json`), read `measurement_order.json` at that run root, and require BOTH cells listed there; if the order file or the run root is absent the summary has missing provenance.
2. Validation fn `validate_pair(a: &M3Summary, b: &M3Summary, independent: bool) -> Result<(), String>` rejecting with a named reason: metric mismatch (file lacks `per_round_means`/has m2 fields), missing provenance, unequal rounds, empty/non-numeric means (parse errors surface as malformed-means), duplicate/missing/noncontiguous round indices (indices must be exactly 0..n−1 in `measurement_order.json` order). In default (paired) mode also reject `run_id` mismatch; `--independent` skips only the run-identity check.
3. Statistic: `ratio = median(a.means) / median(b.means)` (medians via existing `stats::median` on f64 — add an f64 median helper if only u64 exists).
4. CI: paired mode resamples round indices jointly (one index vector applied to both cells); `--independent` resamples each cell's indices separately. Reuse `bca.rs` SplitMix64 PRNG (`pub` the stream or add a small seeded-resampler in ratios.rs using the same algorithm); n_resamples default 2000; seed default 0. Percentile-method interval on the resampled ratio distribution (BCa acceleration is undefined for ratios — use percentile bootstrap and say so in the module doc).
5. Output (stdout, one line): `RATIO <A-name>/<B-name> point=<p.xxxx> lo=<p.xxxx> hi=<p.xxxx>` where name = the summary file's parent directory name (cell dir, `/`-free by layout, e.g. `http-server_rust-camel-lib` — NOT the `cell` field whose embedded slash would make the line ambiguous); append ` UNPAIRED` when `--independent`. Exit 0; validation failures exit 2 with `ERROR: <reason>` on stderr.

**Tests:**
- name: `ratio_point_and_ci_synthetic`
  setup: two temp m3-summary.json files, A means all 200.0 (5 rounds), B means all 100.0 (5 rounds), same run_id, valid measurement_order.json fixture.
  action: run the ratio computation in-process with seed 0.
  assert: point == 2.0 (within 1e-9), lo/hi bracket 2.0 within ±0.05, no UNPAIRED tag.
  command: `cargo test -p bench-loadgen ratio_point`
  expected: fails before, passes after.
- name: `ratio_deterministic_same_seed`
  setup: same synthetic inputs.
  action: compute twice with seed 0.
  assert: identical output strings.
  command: `cargo test -p bench-loadgen ratio_deterministic`
  expected: fails before, passes after.
- name: `ratio_rejects_cross_run_paired`
  setup: two summaries with different run_ids, otherwise valid.
  action: validate_pair in paired mode.
  assert: Err naming provenance/run-identity; `--independent` mode passes validation.
  command: `cargo test -p bench-loadgen ratio_rejects_cross_run`
  expected: fails before, passes after.
- name: `ratio_rejects_malformed`
  setup: five temp cases — m2-style summary (no per_round_means), missing measurement_order.json (no run root), means vec of length 3 vs 5, round indices [0,1,1,3,4], and a means vec containing a non-numeric JSON value plus an empty means vec.
  action: validate each.
  assert: each Err names its specific reason — metric, provenance, round count, round indices, or `means format` (the empty and non-numeric cases MUST produce the `means format` reason).
  command: `cargo test -p bench-loadgen ratio_rejects_malformed`
  expected: fails before, passes after.

**Acceptance:**
- `cargo test -p bench-loadgen --lib` green.
- Manual run recorded in task notes: `cargo run -p bench-loadgen -- aggregate-ratios benchmarks/results-published/20260723T161422Z/http-server_rust-camel-lib/m3-summary.json benchmarks/results-published/20260723T161422Z/http-server_camel-standalone-dsl/m3-summary.json --independent` prints a UNPAIRED RATIO line.
- `cargo fmt --check --all` and `cargo clippy -p bench-loadgen -- -D warnings` exit 0.

- [x] 1.3

## Phase 2: New scenarios

### Task 2.1: t2-json rust artifacts (lib + cli)

**Files:**
- `benchmarks/scenarios/t2-json/README.md` (new — canonical formula, golden digest table, JVM output-order caveat)
- `benchmarks/scenarios/t2-json/rust-camel-lib/Cargo.toml` (new — package `t2-json-rust-camel-lib`, member pattern per `t2-realistic-eip-rust-camel-lib`; `bench-loadgen` path dep; `[[bin]] name = "t2-json"`)
- `benchmarks/scenarios/t2-json/rust-camel-lib/.cargo/config.toml` (new — fixture-local target-dir, copied pattern)
- `benchmarks/scenarios/t2-json/rust-camel-lib/src/main.rs` (new)
- `benchmarks/scenarios/t2-json/rust-camel-cli/Camel.toml` (new — ADR-0033 exec stub + supervision, pattern from t2-realistic-eip Camel.toml)
- `benchmarks/scenarios/t2-json/rust-camel-cli/routes/t2-json.yaml` (new)
- `benchmarks/scenarios/t2-json/smoke/run.sh` (new — starts fixture, greps marker, per http-server smoke pattern)
- root `Cargo.toml` (modified — add t2-json fixture to the bench members list where t2-realistic-eip lives)

**Steps:**
1. `rust-camel-lib/src/main.rs`: read `BENCH_PAYLOAD_BYTES` (default 32768; validate via `VALID_PAYLOAD_SIZES`), build body with `canonical_json_body(size, 0)`; log `BENCH_INPUT_SHA256=<canonical_body_sha256(size,0)>`; route `timer:bench?repeatCount=1&delay=0` → `set_body(body)` → `unmarshal("json")` → `filter` (jsonpath `$.id == 'bench'`) → `transform` (rhai appending `"bench": true` field, marshaling handled next) → `marshal("json")` → `process` step asserting `body.len() == size + 13` and emitting `BENCH_ROUTE_READY bytes=<len>` (assert failure = panic before marker → cell fails). Binary name `t2-json`.
2. Transform step mechanism (pinned, design-D2-aligned native-eq surface): unmarshal produces `Body::Json(serde_json::Value)`; rhai binds bodies as TEXT only (`as_text()`), so the structured transform uses the native-eq surface per artifact — lib: Rust closure (`map_body`) inserting the `"bench": true` member into the parsed map (same idiomatic-surface deviation class t2-realistic-eip documents); CLI: `transform: {language: js}` whose script mutates `camel.body` and returns the MAP, NEVER a string. `marshal("json")` performs the single serialization — a transform that re-serializes internally would make `marshal` double-encode: forbidden. The final assert does BOTH checks: exact length `size + 13` AND parsed semantic equality (lib: parse via serde_json — `id == "bench"`, `seq` present, `fill` all `'b'`, `bench == true`; CLI: in-route length assert plus `"bench":true` substring check, per Pair-B DSL surface). Remedy if the length assert fires at first run (number formatting drifting `0` to `0.0` or similar): fix the transform to mutate the parsed structure without re-serializing — do NOT relax the +13 assert.
3. `rust-camel-cli`: YAML route mirroring the same steps (DSL forms from `crates/camel-dsl/src/yaml.rs:815-1128`). MANDATED FLOW — the route YAML carries TWO template tokens: the size constant `SIZE` in the `set_body` rhai `source` script (which builds the canonical document, asserts `body.len() == SIZE` before processing, and after marshaling asserts output length `SIZE + 13` plus substring `"bench":true`), and a `log` step `BENCH_INPUT_SHA256=GOLDEN` where `GOLDEN` is the digest token. The wrapper (smoke script and harness invocation) fulfills the env contract by TEMPLATE COPY: copy `routes/t2-json.yaml` to a temp file, `sed`-substitute `SIZE` with `${BENCH_PAYLOAD_BYTES:-32768}` and `GOLDEN` with the digest for that size from the scenario README's golden table, then run with `--routes <temp>`. The marker emits only when the in-route asserts pass (marker-suffix proves branch, per T2 contract). The CLI artifact thus reports the same `BENCH_INPUT_SHA256` value as the lib fixture for every class — the digest is a pure function of the substituted size.
4. Camel.toml: copy the exec-stub + supervision blocks from `benchmarks/scenarios/t2-realistic-eip/rust-camel-cli/Camel.toml`.
5. `smoke/run.sh`: `BENCH_PAYLOAD_BYTES=32768 timeout 30 cargo run -p <fixture-crate> --bin t2-json` → grep exactly 1 `BENCH_ROUTE_READY` + the SHA line; then the CLI variant via `camel run --config Camel.toml --routes routes/t2-json.yaml --no-watch` with the same greps; exit nonzero on any miss.
6. README: canonical formula, golden table (from Task 1.2 constants), JVM field-order caveat (inputs never differ; outputs assert length+semantics only).

**Tests:**
- name: `t2_json_lib_marker_contract` (smoke, not cargo)
  setup: fixture built in worktree.
  action: `bash benchmarks/scenarios/t2-json/smoke/run.sh` from the scenario dir.
  assert: exit 0; output shows exactly one `BENCH_ROUTE_READY bytes=32781` line for the lib run and one for the CLI run; `BENCH_INPUT_SHA256` present in BOTH runs and equal to the (32768,0) golden.
  command: `cd benchmarks/scenarios/t2-json && bash smoke/run.sh`
  expected: fails before fixtures exist, passes after.
- name: `t2_json_lib_per_class_marker`
  setup: fixture built.
  action: run the fixture with `BENCH_PAYLOAD_BYTES=1024`; expect exactly one `BENCH_ROUTE_READY bytes=1037` marker (per-class correctness). Companion unit test in the fixture crate builds the canonical body, corrupts one byte, and asserts the digest-check helper returns false.
  assert: marker count exactly 1 with bytes=1037; digest-check unit test passes.
  command: `cargo test -p t2-json-rust-camel-lib` (fixture unit tests) plus the smoke variant run.
  expected: fails before, passes after.

**Amendment (2026-08-29, e_gpt audit + e_glm re-bless fe24839c):** step 2's original text over-pinned rhai for the structured transform — unsatisfiable in this codebase (rhai is text-bound). Amended to a design-D2-aligned native-eq surface: lib closure + CLI js returning the map. Invariants unchanged. See scenario README §Transform mechanism.

**Acceptance:**
- `bash benchmarks/scenarios/t2-json/smoke/run.sh` exit 0.
- `cargo fmt --check --all`, `cargo clippy -p bench-loadgen -p <fixture-crate> -- -D warnings` exit 0.
- Root `cargo build --workspace` still green (fixture compiles as member).

- [x] 2.1

### Task 2.2: t2-json JVM artifacts (standalone + quarkus)

**Files:**
- `benchmarks/scenarios/t2-json/camel-standalone/pom.xml` (new — parent, modules dsl+yaml, assembly plugin, pattern from t2-realistic-eip)
- `benchmarks/scenarios/t2-json/camel-standalone/camel-standalone-dsl/pom.xml` (new)
- `benchmarks/scenarios/t2-json/camel-standalone/camel-standalone-dsl/src/test/java/com/rustcamel/bench/CanonicalBodyTest.java` (new)
- `benchmarks/scenarios/t2-json/camel-standalone/camel-standalone-dsl/src/main/java/com/rustcamel/bench/App.java` (new)
- `benchmarks/scenarios/t2-json/camel-standalone/camel-standalone-yaml/pom.xml` (new)
- `benchmarks/scenarios/t2-json/camel-standalone/camel-standalone-yaml/src/test/java/com/rustcamel/bench/CanonicalBodyTest.java` (new)
- `benchmarks/scenarios/t2-json/camel-standalone/camel-standalone-yaml/src/main/java/com/rustcamel/bench/AppYaml.java` (new)
- `benchmarks/scenarios/t2-json/camel-standalone/camel-standalone-yaml/src/main/resources/routes.yaml` (new)
- `benchmarks/scenarios/t2-json/camel-quarkus/settings.gradle.kts` (new — native subprojects only, v3.5 pattern)
- `benchmarks/scenarios/t2-json/camel-quarkus/camel-quarkus-dsl/build.gradle.kts` (new)
- `benchmarks/scenarios/t2-json/camel-quarkus/camel-quarkus-dsl/src/main/java/com/rustcamel/bench/BenchRoute.java` (new)
- `benchmarks/scenarios/t2-json/camel-quarkus/camel-quarkus-dsl/src/test/java/com/rustcamel/bench/CanonicalBodyTest.java` (new)
- `benchmarks/scenarios/t2-json/camel-quarkus/camel-quarkus-yaml/build.gradle.kts` (new)
- `benchmarks/scenarios/t2-json/camel-quarkus/camel-quarkus-yaml/src/main/resources/application.properties` (new)
- `benchmarks/scenarios/t2-json/camel-quarkus/camel-quarkus-yaml/src/main/resources/camel/routes.yaml` (new)
- `benchmarks/scenarios/t2-json/camel-quarkus/camel-quarkus-yaml/src/test/java/com/rustcamel/bench/CanonicalBodyTest.java` (new)
- `benchmarks/scenarios/t2-json/camel-quarkus/camel-quarkus-dsl-native/build.gradle.kts` (new — srcDir share)
- `benchmarks/scenarios/t2-json/camel-quarkus/camel-quarkus-yaml-native/build.gradle.kts` (new)

**Steps:**
1. Java canonical builder (one static class per artifact family, same formula): `static String canonicalJsonBody(int size, long tick)` with `K = size - overhead`; `static String sha256Hex(String)`; every JVM fixture reads its body size from `System.getenv("BENCH_PAYLOAD_BYTES")` defaulting to 32768 (same default as the harness) and logs `BENCH_INPUT_SHA256=<digest>` before processing. Unit tests (exact files, this task): `camel-standalone/camel-standalone-dsl/src/test/java/com/rustcamel/bench/CanonicalBodyTest.java` and `camel-standalone/camel-standalone-yaml/src/test/java/com/rustcamel/bench/CanonicalBodyTest.java` (Maven surefire runs them via `mvn test`) plus the two quarkus test files below — each asserts (32768,0) and (1024,0) digests equal the golden hex literals copied from Task 1.2's table.
2. Standalone dsl `App.java`: route `from("timer:bench?repeatCount=1&delay=0")` then `.process(builds canonical body, logs BENCH_INPUT_SHA256)`, `.unmarshal().json()`, `.filter().jsonpath("$[?(@.id == 'bench')]")`, a transform processor appending the `bench` field, `.marshal().json()`, completion `.process(assert len == size+13)` then `.log("BENCH_ROUTE_READY bytes=${body.length}")` — length via a header set by the assert processor (Simple `${header.benchOutLen}`), keeping the marker format `BENCH_ROUTE_READY bytes=<n>`.
3. Standalone yaml `routes.yaml`: same steps in Camel YAML DSL (camelCase); AppYaml.java loads it (pattern from t2-realistic-eip).
4. Quarkus dsl `BenchRoute.java` + yaml `routes.yaml`: same routes (Quarkus 3.20 BOM pins, `camel-jsonpath` + `camel-jackson` dependencies added to build.gradle.kts); `application.properties` per http-server pattern.
5. README caveat cross-ref: JVM output field order may differ — assert length + parsed semantics only (already in scenario README from 2.1).

**Tests:**
- name: `CanonicalBodyTest` (JUnit, per JVM artifact)
  setup: builder class in each artifact.
  action: build canonical body for (32768,0); assert length 32768 and SHA-256 == golden literal.
  assert: green.
  command: `(cd benchmarks/scenarios/t2-json/camel-standalone && mvn -q test)` and `(cd benchmarks/scenarios/t2-json/camel-quarkus && ./gradlew :camel-quarkus-dsl:test :camel-quarkus-yaml:test)` — run if JAVA_HOME+maven+gradle exist locally; otherwise record in task notes "JVM tests deferred to runner container" and the test files remain the contract.
  expected: compiles + green when run.

**Acceptance:**
- Java test files exist in all four JVM artifacts with the golden literal.
- If local toolchains present: mvn/gradle test green; else notes record the deferral.
- No rust code touched by this task.

- [x] 2.2

### Task 2.3: split-aggregate rust artifacts (lib + cli)

**Files:**
- `benchmarks/scenarios/split-aggregate/README.md` (new)
- `benchmarks/scenarios/split-aggregate/rust-camel-lib/Cargo.toml` (new — package `split-aggregate-rust-camel-lib`, pattern per t2-realistic-eip; `bench-loadgen` path dep; `[[bin]] name = "split-aggregate"`)
- `benchmarks/scenarios/split-aggregate/rust-camel-lib/.cargo/config.toml` (new)
- `benchmarks/scenarios/split-aggregate/rust-camel-lib/src/main.rs` (new)
- `benchmarks/scenarios/split-aggregate/rust-camel-cli/Camel.toml` (new)
- `benchmarks/scenarios/split-aggregate/rust-camel-cli/routes/split-aggregate.yaml` (new)
- `benchmarks/scenarios/split-aggregate/smoke/run.sh` (new)
- root `Cargo.toml` (modified — add split-aggregate fixture member)

**Steps:**
1. Canonical array body (extend Task 1.2's module in-fixture or inline in main.rs — inline: a JSON array whose 100 items are the strings `b0` through `b99` (item i is `"b" + i`) — deterministic, documented in README; assert total in a unit test with a pasted golden digest).
2. `rust-camel-lib/src/main.rs` two routes: outer `timer:bench?repeatCount=1&delay=0` → `set_body(array)` → `split` (expression selecting array items, sequential) → `to("direct:agg-in")`; agg route `from("direct:agg-in")` → `set_header("bench.correlation", const)` → `aggregate` (`completion_size=100`, `force_completion_on_stop=false`, strategy list-append). COMPLETION ASSERT (pinned mechanism): the aggregate step's completion steps receive the aggregated collection — assert `collection.len() == 100` on that payload AND set exchange property `bench.aggregated.size = 100`; the marker step (`BENCH_ROUTE_READY items=100`) fires from the completion path reading that property, so an incomplete bucket can never produce it. The exact completion-payload type as exposed by `crates/camel-processor/src/aggregator.rs` is bound in code with a doc comment naming the API used.
3. CLI YAML mirror using DSL `split` (`expression`, sequential) + `aggregate` (`strategy`, `correlation_key`, `completion_size: 100`, `force_completion_on_stop: false`) forms from `crates/camel-dsl/src/yaml.rs:931-1039`.
4. `smoke/run.sh`: run both artifacts, grep exactly one `BENCH_ROUTE_READY items=100` each.
5. Unit test: incomplete-bucket simulation — build the agg route wiring with a 99-item array and drive it directly (no timer); assert no completion callback fires within a short await window and no marker is logged.

**Tests:**
- name: `split_aggregate_array_golden`
  setup: array builder fn in fixture.
  action: build the 100-item array; assert exact string length and SHA-256 against pasted golden.
  assert: green.
  command: `cargo test -p split-aggregate-rust-camel-lib array_golden`
  expected: fails before, passes after.
- name: `incomplete_bucket_no_completion`
  setup: fixture crate unit test constructing the agg route wiring directly (no timer), with a captured log sink; 99 fragments prepared.
  action: drive 99 fragments through `direct:agg-in`; await 500 ms.
  assert: no completion callback fired, no marker line logged.
  command: `cargo test -p split-aggregate-rust-camel-lib incomplete_bucket`
  expected: fails before, passes after.
- name: `split_aggregate_marker_contract` (smoke)
  action: `bash benchmarks/scenarios/split-aggregate/smoke/run.sh`.
  assert: exit 0; exactly one `BENCH_ROUTE_READY items=100` per artifact.
  command: as stated.
  expected: fails before, passes after.

**Acceptance:**
- All three tests green; smoke exit 0.
- `cargo fmt --check --all`, `cargo clippy` on the fixture crate exit 0.
- Root `cargo build --workspace` green.

- [x] 2.3

### Task 2.4: split-aggregate JVM artifacts + harness registration (both scenarios)

**Files:**
- `benchmarks/scenarios/split-aggregate/camel-standalone/pom.xml` (new)
- `benchmarks/scenarios/split-aggregate/camel-standalone/camel-standalone-dsl/pom.xml` (new)
- `benchmarks/scenarios/split-aggregate/camel-standalone/camel-standalone-dsl/src/main/java/com/rustcamel/bench/App.java` (new — Splitter → direct:agg-in → Aggregate(constant correlation, completionSize=100) → assert CamelAggregatedSize → marker log)
- `benchmarks/scenarios/split-aggregate/camel-standalone/camel-standalone-yaml/pom.xml` (new)
- `benchmarks/scenarios/split-aggregate/camel-standalone/camel-standalone-yaml/src/main/java/com/rustcamel/bench/AppYaml.java` (new)
- `benchmarks/scenarios/split-aggregate/camel-standalone/camel-standalone-yaml/src/main/resources/routes.yaml` (new)
- `benchmarks/scenarios/split-aggregate/camel-quarkus/settings.gradle.kts` (new)
- `benchmarks/scenarios/split-aggregate/camel-quarkus/camel-quarkus-dsl/build.gradle.kts` (new)
- `benchmarks/scenarios/split-aggregate/camel-quarkus/camel-quarkus-dsl/src/main/java/com/rustcamel/bench/BenchRoute.java` (new)
- `benchmarks/scenarios/split-aggregate/camel-quarkus/camel-quarkus-dsl/src/test/java/com/rustcamel/bench/CanonicalArrayTest.java` (new)
- `benchmarks/scenarios/split-aggregate/camel-quarkus/camel-quarkus-yaml/build.gradle.kts` (new)
- `benchmarks/scenarios/split-aggregate/camel-quarkus/camel-quarkus-yaml/src/main/resources/application.properties` (new)
- `benchmarks/scenarios/split-aggregate/camel-quarkus/camel-quarkus-yaml/src/main/resources/camel/routes.yaml` (new)
- `benchmarks/scenarios/split-aggregate/camel-quarkus/camel-quarkus-yaml/src/test/java/com/rustcamel/bench/CanonicalArrayTest.java` (new)
- `benchmarks/scenarios/split-aggregate/camel-quarkus/camel-quarkus-dsl-native/build.gradle.kts` (new)
- `benchmarks/scenarios/split-aggregate/camel-quarkus/camel-quarkus-yaml-native/build.gradle.kts` (new)
- `benchmarks/harness/run.sh` (modified — declare `BENCH_PAYLOAD_BYTES="${BENCH_PAYLOAD_BYTES:-32768}"` alongside the other env defaults (~:168), then `SCENARIO_MARKER` entry for t2-json DERIVED from the same variable: `["t2-json"]="BENCH_ROUTE_READY bytes=$((BENCH_PAYLOAD_BYTES + 13))"` so marker and fixture can never diverge under a non-default class; split-aggregate → `BENCH_ROUTE_READY items=100` (class-independent); `SCENARIO_M2_PROTOCOL` entries `t2-json=B` and `split-aggregate=B`; export `BENCH_PAYLOAD_BYTES` to fixture processes the way `BENCH_HTTP_URL` is forwarded; verify artifact-set map needs no change — these are 6-artifact sets like t2/t3)

**Steps:**
1. JVM split-aggregate routes (standalone + quarkus, dsl + yaml): Camel Splitter with a 100-item JSON array body (built by the Java canonical-array builder with its own golden unit test mirroring 2.2), `.to("direct:agg-in")`; aggregate route with constant correlation Expression, `completionSize(100)`, completion processor asserting exchange property `CamelAggregatedSize == 100` then logging the marker.
2. Java unit test `CanonicalArrayTest` (both standalone and quarkus sides): array string digest == golden literal.
3. `run.sh` registration: add the two scenarios to `SCENARIO_MARKER` and `SCENARIO_M2_PROTOCOL`; verify `resolve_all_cells` accepts them via `--dry-run`.
4. Env forwarding: `run.sh` declares `BENCH_PAYLOAD_BYTES="${BENCH_PAYLOAD_BYTES:-32768}"` with the other defaults and forwards it to fixture processes the way `BENCH_HTTP_URL` is forwarded (`:168` pattern); the t2-json marker entry is computed from that same variable (single source of truth — no divergence possible).

**Tests:**
- name: `dry_run_resolves_new_scenarios`
  setup: run.sh registrations in place.
  action: `bash benchmarks/harness/run.sh --dry-run --scenarios=t2-json,split-aggregate --metric=m1+m2`.
  assert: exit 0; output lists cells for both scenarios across the 6 artifacts; no "unknown scenario" failure.
  command: as stated.
  expected: fails before registration, passes after.
- name: `CanonicalArrayTest` (JUnit)
  setup: Java canonical-array builder + golden literal in each test file.
  action: digest check as in 2.2 pattern.
  assert: green when toolchain present, else deferred-note per 2.2 (owner bd rc-f4po).
  command: `(cd benchmarks/scenarios/split-aggregate/camel-standalone && mvn -q test)` and `(cd benchmarks/scenarios/split-aggregate/camel-quarkus && ./gradlew test)`.
  expected: green when run.
- name: `cross_runtime_digest_equality` (smoke, in t2-json smoke/run.sh)
  setup: all runnable artifacts identified (rust pair always; standalone jars when `java -jar` artifacts exist from a prior `mvn package`; quarkus when a built runner exists).
  action: smoke script runs every runnable artifact with `BENCH_PAYLOAD_BYTES=32768`, captures each `BENCH_INPUT_SHA256` line.
  assert: every captured digest equals the (32768,0) golden; the rust-lib and rust-cli runs are MANDATORY (their absence fails the smoke); JVM artifacts participate whenever runnable.
  command: `cd benchmarks/scenarios/t2-json && bash smoke/run.sh`
  expected: fails before fixtures, passes after.

**Tests (additional):**
- name: `default_marker_unchanged_without_env`
  setup: run.sh edits in place.
  action: `env -u BENCH_PAYLOAD_BYTES bash -c 'BENCH_PAYLOAD_BYTES="${BENCH_PAYLOAD_BYTES:-32768}"; echo "BENCH_ROUTE_READY bytes=$((BENCH_PAYLOAD_BYTES + 13))"'`
  assert: prints `BENCH_ROUTE_READY bytes=32781` (default class preserved for existing invocations).
  command: as stated.
  expected: fails if the default drifts, passes after.
- name: `m3_duration_default_unchanged`
  setup: run.sh edits in place.
  action: `env -u M3_DURATION_SECS bash -c 'M3_DURATION_SECS="${M3_DURATION_SECS:-50}"; echo $M3_DURATION_SECS'`
  assert: prints `50`.
  command: as stated.
  expected: passes after the Task 3.1 edit (test lives here to pin both harness defaults together).

**Acceptance:**
- dry-run resolves both scenarios over 6 artifacts each.
- `bash -n benchmarks/harness/run.sh` (syntax) passes; shellcheck if the repo lints run.sh (check for existing lint; if none, bash -n suffices).
- No behavior change for existing scenarios (dry-run of `--scenarios=http-server` unchanged).

- [x] 2.4

## Phase 3: Lever study + CI + publication

### Task 3.1: Metric-family lever A/B run

**Files:**
- `benchmarks/scenarios/http-server/rust-camel-cli/Camel.toml.metrics-on` (new — arm A: `[observability.prometheus]` port 18191 + `[observability.metrics] enabled=true, exchange=true, duration=true, components=true`)
- `benchmarks/scenarios/http-server/rust-camel-cli/Camel.toml.metrics-off` (new — arm B: same prometheus block + `[observability.metrics] enabled=false`)
- `benchmarks/scenarios/http-server/http-server-cli-wrapper.sh` (modified — honor `BENCH_CAMEL_TOML` env: use `$BENCH_CAMEL_TOML` as the `--config` argument when set, default unchanged)
- `benchmarks/harness/run.sh` (modified — line ~:209 `M3_DURATION_SECS=50` becomes `M3_DURATION_SECS="${M3_DURATION_SECS:-50}"` so 30 s arms are selectable via env; no other behavior change)
- `benchmarks/results/lever-study-metrics-<timestamp>/` (new — run artifacts, NOT committed to git: add to .gitignore results pattern if results/ already ignored; only the summary section below is committed)
- `docs/benchmarks/2026-08-29-benchmark-v4-addendum.md` (new — lever-study section)

**Steps:**
1. Add the `BENCH_CAMEL_TOML` passthrough to the wrapper (default path unchanged when env absent).
2. Verify `[observability.prometheus]` nesting under `[default]` profile (ADR-0066 / demo gotcha: top-level `[observability]` is swallowed by `[default]` — nest correctly in both arm files).
3. Run arm A: `BENCH_CAMEL_TOML=<scenario-dir>/Camel.toml.metrics-on M3_DURATION_SECS=30 bash benchmarks/harness/run.sh --scenarios=http-server --metric=m3 --rounds=5` (the env override added in this task makes 30 s arms selectable; default 50 s unchanged for everyone else). run.sh runs the scenario's artifacts; use only the rust-cli cell from the produced m3-summary.json (no artifact filter exists — that is fine).
4. Run arm B identically with the metrics-off config.
5. Compute: `cargo run -p bench-loadgen -- aggregate-ratios <runA>/http-server_rust-camel-cli/m3-summary.json <runB>/http-server_rust-camel-cli/m3-summary.json --independent` → capture the UNPAIRED RATIO line.
6. Write the lever-study section in the addendum: method (arms, backend constant, ADR-0066 levers, 5×30 s, UNPAIRED caveat), the RATIO line verbatim, interpretation bounds (what the delta does and does not isolate — family emission cost with backend held constant; not collector-vs-NoOp), and the raw-artifacts pointer.

**Tests:**
- name: `lever_ab_produces_ratio` (manual, recorded in addendum)
  setup: both arms' m3-summary.json exist.
  action: the aggregate-ratios command above.
  assert: one `RATIO <A>/<B> point=<p> lo=<l> hi=<h> UNPAIRED` line with finite lo < point < hi.
  command: as stated.
  expected: pass at execution time.
- name: `wrapper_env_passthrough`
  setup: wrapper modified.
  action: run wrapper with `BENCH_CAMEL_TOML` pointing at arm-A file; grep the spawned `camel run` argv in wrapper's debug output (or strace-free: the wrapper echoes its command in verbose mode — add a one-line verbose echo if absent).
  assert: `--config` shows the arm file.
  command: local wrapper invocation with a stub camel binary (PATH-injected echo script).
  expected: fails before, passes after.

**Acceptance:**
- Addendum contains the verbatim UNPAIRED RATIO line with lo/hi.
- Wrapper default behavior unchanged (existing smoke/ logs still valid pattern).
- `bash -n` on wrapper passes; `cargo fmt/clippy` untouched (no rust changes here except none).

- [x] 3.1

### Task 3.2: CI bench-smoke job

**Files:**
- `.github/workflows/ci.yml` (modified — new `bench-smoke` job)

**Steps:**
1. Local validation FIRST (one run, excerpt recorded in task notes): `cargo bench -p camel-bench --bench pipeline --bench body_coercion -- --quick`. This command is FINAL — if criterion rejects the argument combination in this repo's version, the task FAILS and reports back (no silent adaptation).
2. Add job: `bench-smoke` — `runs-on: ubuntu-latest`, `timeout-minutes: 10`, `steps: checkout + rust-toolchain (match existing unit-tests job setup, including any cache config) + run the exact command from step 1`, with a job comment stating the container full-matrix stays manual (`benchmarks/harness/run-all.sh`).
3. Place the job after `quality` in the file.

**Tests:**
- name: `ci_yaml_valid`
  setup: ci.yml modified.
  action: `python3 -c "import yaml;yaml.safe_load(open('.github/workflows/ci.yml'))"`.
  assert: parses; job contains `timeout-minutes: 10`.
  command: as stated.
  expected: fails before, passes after.
- name: `bench_command_validated_locally`
  setup: worktree toolchain.
  action: `cargo bench -p camel-bench --bench pipeline --bench body_coercion -- --quick`.
  assert: exit 0 within 10 minutes; both bench names appear in criterion output.
  command: `cargo bench -p camel-bench --bench pipeline --bench body_coercion -- --quick`
  expected: pass at execution time (this run IS the validation).

**Acceptance:**
- ci.yml parses; job present with timeout.
- The pinned command's local run log excerpt recorded in task notes (last lines: bench names + "ok" summary).

**Local validation record (2026-08-30, worktree host):**
```
Running benches/body_coercion.rs ... exit segment:
  integration/body_coercion/pipeline_mixed_contracts  time: [1.4372 µs 1.4597 µs 1.4654 µs]
Running benches/pipeline.rs ... exit segment:
  integration/pipeline/filter_choice_splitter_log     time: [3.8368 µs 3.9444 µs 3.9713 µs]
```
Command: `cargo bench -p camel-bench --bench pipeline --bench body_coercion -- --quick` — exit 0, criterion 0.8.2 accepted, build 3m06s. Full excerpt in commit 999da446 review record.

- [x] 3.2

### Task 3.3: v4 addendum ratio CIs + COVERAGE update

**Files:**
- `docs/benchmarks/2026-08-29-benchmark-v4-addendum.md` (modified — add ratio-CI table section beside the lever study from 3.1)
- `benchmarks/COVERAGE.md` (modified)

**Steps:**
1. Compute paired ratio CIs for the published v4 run's key ratios (the ones in v4 §"Key ratios"): for each pair, `cargo run -p bench-loadgen -- aggregate-ratios benchmarks/results-published/20260723T161422Z/<cellA>/m3-summary.json benchmarks/results-published/20260723T161422Z/<cellB>/m3-summary.json` (same run — paired mode, no flag). Cells come from the same published run directory, so pairing validation holds.
2. Add the addendum table: `| Comparison | point | 95% CI (lo–hi) |` with the RATIO lines' values, plus a methods paragraph (paired round-block bootstrap, percentile interval, n=2000, seed 0) and the honest note that CI width reflects 5-round resolution.
3. COVERAGE.md updates: (a) new rows/cells for t2-json and split-aggregate (mark `?` or `✓ local-smoke` per the cell-state vocabulary — measured cells only get `✓ vN` after a published run; local-smoke status goes in the cell text pending first container run); (b) payload-axis note on T3 row (axis exists, classes listed, not yet matrix-wide); (c) lever-study note (not a contender row); (d) versioning line per the file's convention (`:159-170`).
4. Keep every non-empty cell compliant with the no-bare-cells rule (`:16-18`).

**Tests:**
- name: `published_ratio_cis_reproducible`
  setup: addendum written.
  action: re-run two of the aggregate-ratios commands; diff against the addendum table values.
  assert: identical (determinism).
  command: the two commands.
  expected: pass.
- name: `coverage_no_bare_cells`
  setup: COVERAGE.md updated.
  action: script check — parse matrix table rows; every cell is `✓`, `✗ won't-measure: <text>`, or `? <condition>`.
  assert: zero bare cells.
  command: `python3` one-liner in task notes performing the check.
  expected: fails before if any bare cell introduced, passes after.

**Acceptance:**
- Addendum table lists every v4 key-ratio with CI.
- COVERAGE.md parses with zero bare cells; new scenarios represented.
- No changes to published v1-v4 report numbers.

- [x] 3.3
