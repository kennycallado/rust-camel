# Design: openapi-env-placeholder-typing

## Approach

Apply the landed tree-walk interpolation canon (ac147d07, rc-93wct) to the
`camel openapi generate` surface by reusing the existing seam read-only,
exactly as the route loader does. No engine change, no new semantics.

New public sibling in `crates/camel-dsl/src/yaml.rs`:

```rust
pub fn extract_rest_blocks_from_file_with_env(
    path: &Path,
    lookup: &dyn Fn(&str) -> Option<String>,
) -> Result<Vec<RouteDslRest>, CamelError>
```

The sibling owns BOTH arms (papal changes #1/#2). Internally: read via
`read_route_file_capped` (16 MiB parity; `pub(crate)` in util.rs:14 is why
this fn must live in camel-dsl), then dispatch on `path.extension()`
mirroring discovery's `interpolate_for_parse` (discovery.rs:286-293):

- `json`: whole-text splice `interpolate_env_with(&content, lookup)` — an
  unresolved no-default variable surfaces with the loader's verbatim wording
  `Environment variable '{var}' not set (required by {path})` — then
  `serde_json::from_str::<RouteDslRoutes>` → `.rest`. The spliced text goes
  straight to serde_json (no YAML round-trip), so typing outcomes match the
  YAML arm: string defaults resolve, int positions fail, no-default names
  the variable.
- everything else (`yaml | yml | <no ext>`): `interpolate_yaml_source`
  (tree-walk first, legacy splice fallback, `Err(var_name)` on no-default)
  → `extract_rest_blocks`, with path-annotated errors.

YAML-shape semantics come free: a substituted leaf keeps STRING typing, so
int-typed positions (`port: u16`, `success_status: Option<u16>`, nested step
ints) fail serde — boot/LEAN/lint parity — and no-default tokens surface
naming the variable.

`crates/camel-cli/src/commands/openapi.rs` `run_generate` collapses read +
extension dispatch + interpolation into ONE sibling call:
`extract_rest_blocks_from_file_with_env(Path::new(&args.file), &|_| None)`.
The local `read_to_string` and extension `match` block disappear; camel-cli
contains zero interpolation and zero scanner logic.

Free-`Value` positions (`request_schema`, `parameters`, `response.schema`,
`headers`) need no special handling: interpolation runs on the document
before serde builds the `Value`, so `${env:T:-string}` resolves to the
concrete string (test assertion only).

Docs fold-in: `docs/src/cli/openapi-plugin.md` and
`docs/src/getting-started/cli.md` gain a short paragraph — placeholders in
`rest:` blocks resolve default-only at generate time; string positions take
the concrete default, int/bool positions error (canon parity); ambient env
is never read.

## Affected crates

- camel-dsl: new pub fn `extract_rest_blocks_from_file_with_env` (~30 lines)
  + unit tests: string-resolve, int-fail parity, no-default names variable,
  escape-literal, comment-hermetic, round-trip-fragile fallback
  (tree-walk→splice, mirroring the loader scenario), JSON-arm outcomes,
  16 MiB cap rejection.
- camel-cli: `openapi.rs` call sites swap to the interpolating path (~15
  lines) + command-level tests (YAML + JSON arms, server-URL concretization,
  error wording).
- docs: two paragraphs.

## Architecture boundaries

DSL-crate authoring surface only (route-file parsing), consistent with the
loader precedent — no Runtime, Components, Services, Languages, or Functions
involvement; the generator never instantiates any runtime object. Hermeticity
per ADR-0069 §4 (hermeticity is primary): the default-only lookup `&|_| None` never reads the
process environment — `camel openapi generate` stays deterministic. The
lookup-injectable signature keeps the rc-l7m7t doc-env layering composable:
a future caller can pass a real lookup with zero rework. YAML/JSON parsing
follows ADR-0017/0026 DSL parsing (same `RouteDslRoutes` AST either way).
Exclusion zones respected: `env_interpolation.rs`, `discovery.rs`,
`test/runner.rs`, `document.rs`, and mock-testkit specs untouched.

## Alternatives considered

- Emit placeholder fields string-typed in the schema — REJECTED: schema
  drift; contradicts the canon (int/bool positions are errors, never
  silently re-typed).
- Hard error on token detection without interpolation — REJECTED (papal):
  opaque diagnostics, no variable naming, duplicates the scanner the seam
  owns; Option A yields the same error class as canon-consequent behavior.
- Interpolate inside `extract_rest_blocks` itself — REJECTED: silently
  changes behavior for every future caller; the explicit `_with_env`
  sibling keeps interpolation opt-in at the call site, matching the loader
  pattern.

Single-phase change — no `## Phases` section; tasks.md carries a flat task
list.
