# Design: canonical-path-fuzzing

## Approach

Mirror the proven `dsl_yaml` pattern (UTF-8 gate → parse → discard
result; panic = crash) three times in `camel-fuzz`:

- `dsl_json_harness(data)`: calls
  `camel_dsl::json::parse_json_with_threshold_and_security(s,
  DEFAULT_STREAM_CACHE_THRESHOLD, SecurityCompileContext::default())` —
  the same threshold and security arguments as the YAML harness.
- `dsl_template_harness(data)`: calls
  `camel_dsl::template::json::parse_json_templates(s)` and
  `parse_json_templated_routes(s)`. When both sections parse `Ok`, each
  instance whose `route_template_ref` matches a parsed template is
  materialized via `materialize_and_compile` with the instance's
  parameters and the default threshold and security arguments; an
  instance referencing a missing template is an ordinary rejection (no
  materialization attempted).
- `dsl_parity_harness(data)`: two parts. Panic coverage: the
  threshold-and-security parse variants of both front-ends run first,
  results discarded. Differential (directional, JSON ⊂ YAML): the text
  deserializes with `serde_json` to `RouteDslRoutes`; on `Err` the input
  is outside the overlap and the comparison skips it; on `Ok` the same
  text deserializes with the YAML serde front-end, and the harness
  panics on YAML `Err` or disagreement at the step layer — route count,
  per-route `id`/`from`, or the `{:#?}` rendering of the flattened
  `Vec<&RouteDslStep>`. This mirrors `parity_tests.rs`, which compares
  pre-converter step lists precisely because both front-ends funnel
  through the same `route_dsl_to_declarative_route` converter — a
  post-converter comparison would normalize away front-end divergence.
  The fuzz crate gains `serde_json` and the YAML serde front-end as
  dependencies (fuzz crate only; no production change).

Seeds (`fuzz/seeds/<target>/`): valid `dsl_json` seeds from real
canonical JSON — the JSON arms of parity test cases, camel-dsl JSON test
fixtures, and `schemas/dsl/route-schema.json`-valid documents;
malformed/adversarial `dsl_json` seeds are JSON-native fixtures; no
YAML→JSON conversions. `dsl_template` from template parser/materializer
test fixtures (valid, placeholder-heavy, malformed). `dsl_parity` from
documents valid in both formats plus malformed-in-both shapes. No
cross-target corpus sharing (each target keeps its own
`target-fuzz/corpus/<target>/`), matching the wrapper's existing
layout.

Wrapper (`scripts/xtask/src/fuzz.rs`): extend `KNOWN_TARGETS` to the
four names. All other wrapper logic (corpus/artifact dirs, seed copy,
tmin, guards) is already per-target.

CI (`.github/workflows/fuzz-smoke.yml`): keep one job; select the leg
set by ordered rules over the changed paths (union-combined): each
`fuzz/seeds/<target>/**` change selects its own target;
`yaml.rs` → `dsl_yaml` + `dsl_parity`; `json.rs` → `dsl_json` +
`dsl_parity`; `template/**` → `dsl_template` + `dsl_parity`; any other
trigger-matching path → all legs; `workflow_dispatch` → all legs. Run
legs serially — 60 s smoke each (dispatch MAY raise the per-leg budget,
300 s criterion closes there), shared caches, no matrix cache races.
The tmin panic-injection drill moves to the `dsl_json` leg (proves the
mechanics end-to-end for a new target; the sed pattern becomes
per-target). Job ceiling stays 20 min for PRs; dispatch ceiling is
documented at 30 min (validated by the evidence run).

## Affected crates

- `camel-fuzz` (`fuzz/`): three harness functions, three `[[bin]]`
  targets, three seed directories, and two new dependencies
  (`serde_json`, the YAML serde front-end) for the parity comparison
  (regular `[dependencies]` — `[[bin]]` targets cannot see dev-deps).
- `camel-xtask` (`scripts/xtask`): `KNOWN_TARGETS` extension only.
- `.github/workflows/fuzz-smoke.yml`: leg selection + serial loop +
  per-target drill parameterization.
- `camel-dsl`, `camel-api`: unchanged (pure test-surface addition).

## Architecture boundaries

Respects the DSL boundary: the harnesses sit in the excluded-from-workspace
`fuzz` crate and consume only public `camel_dsl` / `camel_api` APIs, the
same seam `dsl_yaml` already uses. No Runtime, Component, or Services
code is touched. The differential harness encodes an EXISTING contract
(the parity requirement between front-ends, ADR-0026 / ADR-0017), not a
new one.

## Alternatives considered

- Matrix jobs (one runner per target): rejected — parallel legs race the
  warm-path caches added in the previous change; serial legs share them.
- Amending rc-eba8 (mutants) to absorb JSON fuzzing: rejected per expert
  ruling — conflates fuzz and mutate activities; mutants stay sequenced
  behind this change so their corpus includes the canonical path.
- Fuzzing `rest.rs`/`openapi.rs`/`mcp.rs` now: rejected — distinct
  grammars and trust models; tracked as bd rc-rs2v, ranked by
  untrusted-input exposure when picked up.
