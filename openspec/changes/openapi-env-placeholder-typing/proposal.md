# Proposal: openapi-env-placeholder-typing

## Why

`camel openapi generate` reads route files raw (`std::fs::read_to_string`,
crates/camel-cli/src/commands/openapi.rs:38) and never runs the landed
env-interpolation canon (ac147d07, rc-93wct). Two live defects on main
(verified empirically):

1. Integer-typed `rest:` positions (`port: u16`, `success_status:
   Option<u16>`, nested `steps` int fields) carrying `${env:NAME:-default}`
   fail with an opaque serde error — `type mismatch: expected unsigned
   integer, found string` — that does not name the variable and reads the
   same with or without a default.
2. String and free-`Value` positions (`host`, `request_schema`, `parameters`,
   `response.schema`, `headers`) succeed and leak literal `${env:...}` tokens
   into the generated OpenAPI document — e.g. `"url":
   "http://${env:HOST:-0.0.0.0}:9090"` and `"type": "${env:T:-string}"` —
   producing documents no OpenAPI consumer accepts.

bd rc-gykds (P3, latent→active the moment a rest block carries any
placeholder). The read also bypasses the 16 MiB `read_route_file_capped`
limit every other route-file loader enforces.

## What Changes

Apply the landed tree-walk canon to the openapi generate surface (papal
verdict e_opus ses_f7fc45128ffemr7Y6ukK0iOdEC: PROCEED-WITH-CHANGES,
Option A):

- camel-dsl `yaml.rs`: new `extract_rest_blocks_from_file_with_env(path,
  lookup)` sibling of `load_from_file_with_env` that owns BOTH arms — reads
  via `read_route_file_capped` (pub(crate), forces camel-dsl placement),
  dispatches on extension mirroring discovery's `interpolate_for_parse`:
  `json` → `interpolate_env_with` splice → `serde_json::from_str`;
  otherwise → `interpolate_yaml_source` (tree-walk first, splice fallback)
  → `extract_rest_blocks`.
- camel-cli `openapi.rs`: `run_generate` collapses read + extension
  dispatch + interpolation into ONE sibling call with the default-only
  lookup (`&|_| None`). camel-cli contains zero interpolation logic.
- Spec delta: ADD requirement "OpenAPI generation interpolates env
  placeholders with tree-walk-first semantics" to `openspec/specs/dsl/spec.md`
  (9 scenarios: 8 papal-checklist + size-cap).
- Docs fold-in: `docs/src/cli/openapi-plugin.md` + `docs/src/getting-started/
  cli.md` gain a paragraph on default-only resolution semantics.

Excluded: no change to the interpolation engine (`env_interpolation.rs`,
`discovery.rs` — landed law, read-only reuse); no test-doc/mock-testkit
surface (sibling rc-4hexo); no ambient-env access (hermetic, ADR-0069 §4);
no new knob mechanism (rc-l7m7t composes later via the `_with_env` lookup).

## Acceptance criteria

- String-typed field with default resolves to the concrete default at
  generate time; no literal token appears in the document.
- Int/bool-typed position with placeholder fails generation (boot/LEAN/lint
  parity); no-default token in a value position fails naming the variable.
- `$${env:}`/`$$` escapes stay literal; comments never fail resolution;
  ambient process env is never consulted.
- JSON input parity: same three outcomes (string-resolve / int-fail /
  no-default-names).
- openapi generate reads honor the 16 MiB route-file cap.
- All 9 delta scenarios (8 papal-checklist + size-cap) covered by tests.

## Risk budget

Blast radius: one pub fn in camel-dsl + openapi.rs call sites + tests + docs.
Acceptable: generated documents become concrete at generate time (string
defaults resolved — the same tradeoff LEAN accepted under the canon).
Out of bounds: any engine edit, any ambient-env read, any mock-testkit file,
any behavior change for placeholder-free files within the size cap.
