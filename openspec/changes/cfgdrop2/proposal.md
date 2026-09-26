# Proposal: cfgdrop2

## Why

Mission 288 (cfgdrop, rc-zbyyv) landed the fail-loud rejection of
root-level known config keys that profile selection would silently
discard. Its review (e_glm + r_glm) filed two P2 deferrals that leave
the fail-loud rule incomplete and the two front doors of one document
disagreeing:

1. rc-3d76f — `camel compile` still ACCEPTS root-level known keys
   beside profile sections while the filesystem loader rejects them:
   the same `Camel.toml` compiles but `camel run` / `camel job` errors.
2. rc-t2j3r — an unknown root TABLE beside an ACTIVE profile is still
   silently dropped by `select_profile_sections`; the rc-cflo warn only
   fires when NO profile is active. A typo like `[obsevrability]`
   vanishes without a trace. The fix must NOT reject genuinely
   profile-shaped names (`[staging]` beside `CAMEL_PROFILE=prod`) —
   the policy must distinguish typos from unselected profiles.

## What Changes

- **Compile/runtime parity (rc-3d76f)**: one disposition per document
  on both paths.
  - Compile (`camel-cli compile::sources`) mirror-rejects root-level
    known keys beside profile structure, naming the keys and accepted
    shapes — EXCEPT `routes`.
  - Runtime (`camel-config` loader) exempts root `routes` beside
    profile structure and gives it the compile-identical overlay
    semantic (root `routes` is the base; `[default]`, then the
    selected profile section, replaces it). `routes` is live at
    runtime (`discover_routes_with_threshold`), so this is real
    effect, not tolerance of a dead key.
- **Near-miss root tables (rc-t2j3r)**: an unknown root table within
    Levenshtein distance ≤ 2 of a long (≥ 8 chars) known top-level key
    is rejected naming the probable intended key — on BOTH paths. Far
    names keep unselected-profile semantics (silent beside an active
    profile, rc-cflo warn without one).
- Shared policy helper (known-key set + near-miss predicate) exported
    from `camel-config`; `camel-cli` imports it (dependency exists).
- Battery extensions (camel-config loader battery 311+, camel-cli
    compile tests) + explicit cross-path parity asserts feeding the
    SAME document text to both dispositions.
- `case_08_error.txt` regenerates (the error stops naming `routes`);
    compile parity goldens stay byte-identical (fixtures mix only root
    `routes` + profile sections).

Excluded: the virtual-store seam (`from_toml_value_with_env` —
receives pre-selected trees by construction), rc-1sapt (third 288
deferral, not this mission), any change to
`camel_dsl::config_semantics` canonical helpers, gh#52 communication.

## Acceptance criteria

- Same document ⇒ same disposition: every shape class (root known
  non-routes key, root `routes`, near-miss table, far unknown table,
  flat document) compiles and boots identically, locked by parity
  asserts.
- Both bds' semantics implemented fail-loud; `[staging]`-style
  unselected profiles never rejected or warned beside an active
  profile (negative lock).
- camel-config battery ≥ 311 + extensions, 0 failed; compile parity
  byte-identical for unchanged semantics.
- Gates: fmt, clippy legs, doc build, schema check green.

## Risk budget

- The runtime `routes` exemption narrows 288's landed rejection —
  accepted, because parity is the mission goal and the overlay is
  byte-compatible with compile's locked semantic. Spec deltas carry
  the MODIFIED requirements with scenario names preserved.
- False-positive risk of the near-miss matcher is bounded by the
  length ≥ 8 target subset (plausible short profile names cannot
  match); `[staging]`-class names stay ≥ 3 edits from every target.
- camel-cli surface (`compile/sources.rs` + tests) sits in the
  camel-cli-compile zone leased by mission 290 (r5routesrv) — textual
  overlap is unlikely (different files); flagged in the park report
  for landing-order coordination.

Bd: rc-3d76f, rc-t2j3r (discovered-from rc-zbyyv). Affected crates:
camel-config, camel-cli.
