# Proposal: configunify

## Why

The config-loader semantics — TOML deep-merge (`merge_toml_values`),
profile-section selection (`apply_profile` / `apply_profile_lenient` /
`select_profile_sections`), and include-key stripping plus the ordered
include walk (`extract_includes`) — exist in three hand-maintained
mirrors:

1. `camel-config` `config.rs` (`pub(crate)` filesystem loader),
2. `camel-cli` `compile/sources.rs` (compile-time source selection:
   ordered include walk, route-overlay replace semantics, strict
   unknown-profile rule),
3. `camel-dsl` `discovery.rs` virtual-store path (runtime assembly for
   compiled artifacts, guarded by SYNC comments).

The mirrors exist only because the dependency direction
(camel-config → camel-dsl) historically forbade sharing `pub(crate)`
helpers. Drift between them silently diverges the effective
configuration of compiled artifacts from `camel run` (bd rc-io2zl).

## What Changes

- New canonical module `camel_dsl::config_semantics` holding the shared
  TOML primitives: deep merge (tables recursive, arrays replace),
  generalized ordered profile-section selection, ordered section walk
  and include-declaration collection, include-key stripping, and the
  structure/presence predicates. The strict/lenient selection
  transformation is canonical; consumer error policies (camel-config's
  `"Unknown profile: {}"`, camel-dsl's `MalformedVirtualConfig` text)
  remain local and byte-identical.
- `camel-config` deletes its private copies and delegates (existing
  camel-config → camel-dsl edge; no Cargo.toml change).
- `camel-dsl` virtual-store assembly consumes the canonical helpers;
  SYNC comments on these helpers die.
- `camel-cli` `compile/sources.rs` consumes the canonical helpers for
  the include walk, profile-section route overlay, and unknown-profile
  rule where behavior is proven identical.
- Virtual config assembly (~230 lines, `VirtualConfigRefs` through the
  merge helpers) moves from `discovery.rs` (2650+ lines) into a new
  `virtual_config.rs` module — pure move, no behavior change.
- Parity golden tests lock the effective resolved configuration for a
  representative matrix (profiles, includes, env overrides, virtual
  store) across filesystem loader and compiled-artifact runtime —
  authored BEFORE the refactor against current behavior.
- Ride-along rc-0omir: audit the 9 camel-cli tests that spawn nested
  cargo for `CARGO_TERM_COLOR=always` sensitivity; apply the
  feature_profiles env-pin + ANSI-strip pattern or document immunity.
- Ride-along rc-khjnb: CI compile-gate step
  `cargo check -p camel-test --features integration-tests --tests`
  before "Test (full workspace)".

Excluded: `clean_integer`/`clean_i64` mirror (env int probing, separate
rule pair), camel-http and docs/adr+docs/audits zones, any change to
the artifact/trailer format, any behavior change to resolution
semantics (error strings included).

## Acceptance criteria

- Exactly one canonical implementation of each hoisted semantic; the
  two other mirrors delegate or are deleted.
- No SYNC comments remain on config-loader semantics (the
  `clean_integer` pair keeps its own).
- Pre-refactor golden fixtures of resolved effective config pass
  unchanged after the refactor (filesystem loader, virtual store,
  compiled artifact).
- Existing green suites stay green: `cargo test -p camel-config -p
  camel-dsl`, camel-cli full suite, `feature_profiles` golden deptree.
- CI gains the gated-test compile-gate step with rc-8g35d/rc-khjnb
  reference; rc-0omir closed with a per-test verdict.

## Risk budget

Highest risk is silent behavioral drift during hoisting — bounded by
authoring parity goldens against the PRE-refactor code first. Error
message strings are observable behavior and must not change. camel-core
must not be touched. If the hoisting boundary proves contested (e.g.
route-overlay consumption cannot be proven identical), fall back to
keeping that narrow piece local and document it — escalation budget:
1 expert consultation.

Bd: rc-io2zl (main), rc-0omir + rc-khjnb (ride-alongs).
Affected crates: camel-dsl, camel-config, camel-cli, .github/workflows/ci.yml.
