# ADR-0026: JSON Canonical Route Authoring Format

**Date:** 2026-06-26
**Status:** Accepted
**Issues:** rc-iq7

## Context

rust-camel supports two route authoring formats: YAML and JSON. Both deserialize
into the same AST and compile into `DeclarativeRoute` via the shared
`route_dsl_to_declarative_route` function
(`crates/camel-dsl/src/yaml.rs`, `crates/camel-dsl/src/json.rs`). The shared AST
types are `RouteDslRoutes`/`RouteDslRoute`/`RouteDslStep`
(`crates/camel-dsl/src/route_ast.rs`); both formats consume them directly (no
format-specific aliases).

Discovery supports both formats by extension
(`crates/camel-dsl/src/discovery.rs`); the CLI defaults to `routes/*.yaml`
(`crates/camel-cli/src/commands/run.rs`) and JSON requires explicit `.json`
glob — a deliberate ergonomic default, not an assertion of YAML primacy.

The existing canonical runtime contract (`CanonicalRouteSpec`) is minimal by design
and NOT a full authoring DSL (`crates/camel-api/src/runtime.rs`, ADR-0011/0016).

SDKs, programmatic route generators, IDE tooling, and machine-driven workflows all
speak JSON natively. YAML is the better human authoring format but a worse machine
contract (implicit-type quirks like the Norway problem, no native schema validation
in popular tooling, ambiguous block scalars).

## Decision

JSON is the canonical **full route authoring format** for SDKs, generators, schema
validation, IDE completion, and machine workflows. YAML remains a supported human
convenience derived from the same AST.

Concretely:

1. Shared code names use `RouteDsl*` (not `Yaml*`). Both formats lower through the
   same `route_dsl_to_declarative_route` function.
2. The project publishes a generated full-DSL JSON Schema at
   `schemas/dsl/route-schema.json` plus TypeScript types under `schemas/ts/`.
   Both are regenerated via `cargo xtask schema`; drift is detected by
   `cargo xtask schema --check` (CI gate, non-mutating).
3. New verbs MUST land with JSON schema + tests + examples first. YAML parity must
   follow before release.
4. Discovery supports both formats when configured; default CLI glob remains
   `routes/*.yaml`; JSON requires explicit `.json` glob. No API or doc implies
   YAML is the "primary" format beyond human-ergonomics defaults.
5. Error messages carry input format context via `InputFormat` boundary annotation
   (`crates/camel-dsl/src/input_format.rs`): messages are prefixed with
   `"YAML DSL error: "` or `"JSON DSL error: "` so users know which parser failed.

## Consequences

- ADR-0017 (snake_case naming) applies to DSL keys in both JSON and YAML.
- ADR-0011/0016 remain about the minimal runtime canonical contract, not authoring
  format. `CanonicalRouteSpec` and the full-DSL JSON Schema are distinct artifacts.
- Maintainers must enforce JSON-first verb landing in code review until tooling
  automates the check.
- YAML users are not demoted: feature parity is release-blocking, but no longer
  implicit-primacy.
- Future: hosted stable schema URL (post-1.0); SDKs in target languages derive from
  the schema.

## References

- Spec: `docs/superpowers/specs/2026-06-24-json-canonical-route-format.md` (oracle-blessed loop 3, e_gpt; preserved on main worktree, gitignored — if absent, see bd rc-iq7 epic notes for summary).
- bd epic: rc-iq7 (tracks all sub-tasks and merged commits).
- Implementation commits: d652b4e2 (A1 rename), 9dc7b494 (B1 parity), 65d0d5d4 (B2+B3), 29cab318 (C1-C3 schema+TS), 301b0cfb (D1 format-aware errors).
- Supersedes: rc-l6e (closed 2026-06-24; 4 gaps absorbed into B1/B2/B3/A1).
