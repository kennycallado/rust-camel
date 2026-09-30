# Design: nondsl-channel-fuzzing

## Approach

Extend the established cargo-fuzz pattern (rc-a456 / rc-fvah) with three
channel-focused targets. Every input is a DSL document; each harness is
a thin `&[u8]` → UTF-8 → parse-and-discard wrapper in `fuzz/src/lib.rs`,
mirroring `dsl_yaml_harness`/`dsl_json_harness`. Because the JSON and
YAML front-ends both lower channel blocks on parse
(`json.rs:43-45`, `yaml.rs` equivalents call `expand_rest_into` /
`expand_mcp_into` / `check_duplicate_route_ids`), each channel harness
feeds the same bytes to BOTH front-ends (JSON-first, same mechanics as
`dsl_parity`), so one corpus exercises both authoring paths.

- `dsl_rest_harness`: parse both front-ends. Reaches
  `lower_all_rest_to_routes` internals: duplicate (host,port,verb,path)
  detection, template ambiguity (§6.3/§7.2), `parse_path_template`,
  media validation, binding lowering.
- `dsl_mcp_harness`: parse both front-ends. Reaches
  `lower_all_mcp_to_routes`: `validate_mcp_name`,
  `validate_tool_input_schema`, percent-encoding of schemas/URIs into
  `from` URIs, bind/TLS/caps parameter lowering.
- `dsl_openapi_harness`: mirror the camel-cli `run_generate` caller's
  validated-generation stage (not its file loading or env resolution).
  Extract the original `rest` AST without lowering — YAML leg via
  `camel_dsl::yaml::extract_rest_blocks`, JSON leg via
  `serde_json::from_str::<RouteDslRoutes>` then `.rest` — then run
  `lower_all_rest_to_routes` + `check_duplicate_route_ids` on the blocks
  and, only when validation passes, call
  `generate_openapi(&blocks, "fuzz", "0.0.0")`, discarding the result.
  Note the front-end parse APIs return compiled `Vec<RouteDefinition>`
  (no `.rest` access), which is why extraction goes through these
  dedicated paths. Validation gates mean generate's input domain is
  "rest blocks that lowered cleanly": the reachable warning paths are
  weak-stub schemas and duplicate `(path, verb)` across different
  listeners (per-listener §6.3 scoping), not unknown verbs (rejected
  at lowering).

## Entry-point audit and ranking (decision doc §3.1 principle)

1. **rest** (1921 LoC) — control-plane, remote-pushable (same channel as
   §3.1 rank 1/2). Lowering output constructs HTTP listeners
   (host/port/verb/path); cross-block validation is the last line before
   listener registration. Highest exposure.
2. **mcp** (1449 LoC) — same control-plane channel. Tool
   `input_schema` (arbitrary `serde_json::Value`) and resource URIs
   become percent-encoded runtime routing keys; bind addresses, TLS
   paths, caps, `security_policy` propagation; tools are exposed to MCP
   clients at runtime.
3. **openapi** (900 LoC) — **audit falsifies the bd hypothesis**: no
   external-document ingestion path exists (camel-dsl `openapi.rs` is
   generation-only; camel-cli `commands/openapi.rs` exposes only
   `run_generate`). Input trust = operator rest blocks; residual
   exposure is the generated document being served to untrusted
   consumers (doc-integrity/availability). Ranked 3rd on the input-trust
   axis; still fuzzed — defense in depth (§3.1).

## Affected crates

- `fuzz/` (camel-fuzz): 3 harness fns + contract tests, 3 `[[bin]]`
  entries, 3 `fuzz_targets/*.rs`, `fuzz/seeds/{dsl_rest,dsl_mcp,dsl_openapi}/`.
- `scripts/xtask`: `KNOWN_TARGETS` += the three names (wrapper
  validation only).
- `scripts/fuzz-legs.sh`: `ALL_TARGETS` += three (canonical order:
  `dsl_yaml dsl_json dsl_template dsl_parity dsl_rest dsl_mcp
  dsl_openapi`); classify rules `crates/camel-dsl/src/rest.rs` →
  `dsl_rest dsl_openapi`, `crates/camel-dsl/src/mcp.rs` → `dsl_mcp`,
  `crates/camel-dsl/src/openapi.rs` → `dsl_openapi`; because every new
  harness drives BOTH front-ends, the `json.rs` and `yaml.rs` rules
  additionally select `dsl_rest dsl_mcp dsl_openapi` alongside their
  existing legs; self-test cases extended. fuzz-smoke.yml needs no
  structural change (legs come from the script; its existing
  `Cargo.toml`/`Cargo.lock` triggers stand unchanged).

No production crate changes.

## Architecture boundaries

All three channels live in the DSL layer (camel-dsl); harnesses consume
public parse/generate APIs only — no Runtime/Component boundaries are
crossed. The data/control-plane split is untouched: fuzz inputs are
control-plane documents; the invariant is "never panic, always terminate
under the documented budget" (§3.2 of the decision doc). Reuses
`arbitrary`-free raw-bytes pattern of the existing text-parser targets.
ADR-0026 (JSON full-DSL authoring) is why both front-ends are driven;
fuzz isolation follows the fuzzing-mutation adoption decision (§2 disk
traps, §3.3 corpus policy).

## Alternatives considered

- Separate per-front-end targets (rest_json + rest_yaml + …): rejected —
  six targets double corpus/CI cost for no coverage gain; JSON is valid
  YAML so one JSON-shaped corpus drives both paths (dsl_parity
  precedent).
- Driving `lower_all_rest_to_routes` directly on `arbitrary`-derived
  ASTs: rejected — bypasses the real entry point (front-end parse),
  loses deserializer coverage, adds structured-input machinery §3.2
  reserves for non-text grammars (SSRF).
- Deferred openapi target until an ingestion path exists: rejected — bd
  acceptance requires all three channels; generation-over-operator-data
  still warrants defense-in-depth fuzzing.

Single-phase change (coherent slice, no milestone grouping needed).
