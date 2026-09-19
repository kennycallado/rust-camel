# Design: profilestrict

## Approach

Align the compiler to the already-canonical strict semantics instead
of relaxing the runtime (ADR-0075 fail-closed posture; the strict
unknown-profile predicate is spec-frozen in `config-loader-semantics`).

Today `camel-cli/src/compile/sources.rs` resolves each selected
profile by scanning the config table first, then every include table
(lenient fallback), and synthesizes a `<name>.profile.toml` fragment.
At boot, `camel_dsl::virtual_config::build_virtual_config` mirrors
camel-config's `apply_profile`: a config document that carries
`[default]` and lacks every selected profile section fails with
`MalformedVirtualConfig`. Both runtime consumers agree; the compiler
alone accepts.

Fix in `compile::sources`, inside the existing selected-profile
resolution block, in two ordered steps:

1. Per-profile chain-wide resolution stays first. A profile found
   nowhere (config nor includes) keeps reporting the frozen
   `UnknownProfile(name)` error in flag order.
2. A strict gate runs after the chain-wide per-profile resolution and
   fragment synthesis, before route-source expansion (placement
   preserves `UnknownProfile` precedence; on trip, resolution fails
   and the partially collected raw set is discarded — no output is
   produced): when selected profiles are non-empty,
   `has_profile_structure(&config, &selection.profiles)` is true, and
   `has_selected_profile(&config, &selection.profiles)` is false,
   resolution fails with a new `SourceError::IncludeOnlyProfiles`
   variant. Its payload carries the collective selected names in
   deduplicated selection order (when the gate trips, none of them is
   in the config). The gate reuses the canonical
   `camel_dsl::config_semantics` predicates by delegation (no local
   predicate copy), keeping the acceptance region byte-equal to the
   runtime's: error iff selected profiles are non-empty AND
   `[default]` present AND no selected profile in the config
   document.

Error text (local to `compile::sources`, STE, actionable): name the
profiles, state the rule — a configuration with `[default]` must
declare at least one selected profile itself — and give the remedy:
move at least one selected section into the configuration document.

Reachable outcomes after the fix:

- `[default]` + include-only profile → compile-time error, no artifact.
- `[default]` + at least one selected profile in config → unchanged
  (include-derived sections for the other profiles still synthesize;
  runtime accepts, boot succeeds).
- Flat config + include-only profile → unchanged lenient path.
- Total absence anywhere → unchanged `UnknownProfile`.

## Affected crates

- `camel-cli`: `src/compile/sources.rs` (gate + error variant + unit
  tests); `tests/config_compile_parity.rs` and
  `tests/fixtures/config-parity-project/` (new fixtures, locked
  stderr golden, resolved-config golden).
- `camel-dsl`, `camel-config`: no code change; their strict behavior
  is the reference the compiler mirrors.

## Architecture boundaries

Compile-time source selection only (data-plane boot inputs). No DSL
parsing, runtime lifecycle, component, or trailer/manifest changes.
The decision predicate stays single-sourced in
`camel_dsl::config_semantics`, per the config-loader-semantics
delegation mandate. References: ADR-0075 (self-contained artifact,
compile-time `--config`/`--profile` selection), CONTEXT-MAP entries
"Compile-time source selection" and "Virtual document store".

## Phases

Single-phase change (one gate, one error variant, tests, spec delta);
no `## Phase N` headings in tasks.md.

## Alternatives considered

- Relax the runtime to accept include-only profiles: rejected — it
  diverges the virtual store from the filesystem loader, whose strict
  rule and error strings are frozen by `config-loader-semantics`
  goldens, and weakens the fail-closed posture.
- Make synthesized profile fragments the authoritative runtime overlay
  (dissolves M-1 dual-derivation): rejected as out of scope — a
  redesign of fragment semantics for a fail-closed bugfix; it would
  also change boot behavior for currently-working inputs.
- Tighten per-profile (every selected profile must live in the config
  when `[default]` exists): rejected — stricter than the runtime's
  any-selected-profile acceptance, forbidding inputs the loader boots
  today (for example `--profile prod --profile canary` with `prod` in
  config and `canary` in an include).
