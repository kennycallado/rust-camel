# Proposal: profilestrict

## Why

`camel compile` accepts a selected profile whose section exists only in
an include when the configuration document carries `[default]`. The
compile succeeds and embeds a synthesized `<name>.profile.toml`
fragment, but the artifact fails at boot: the virtual-store assembly
(`camel_dsl::virtual_config::build_virtual_config`) mirrors
camel-config's strict `apply_profile` rule and rejects the same input
with `MalformedVirtualConfig: unknown profile`. The filesystem loader
rejects it too (`Unknown profile: <name>`). The compiler is the only
lenient consumer, so a green compile can produce a dead artifact. This
violates the fail-closed posture of ADR-0075's compile-time source
selection. Found as I-2 in the multidoc Task 2.1 review (bd rc-86m92);
tracked as bd rc-f2bdx.

## What Changes

- `camel-cli` `compile::sources` gains a strict compile-time mirror of
  the canonical unknown-profile rule: when the configuration document
  carries `[default]` and none of the selected profile sections exists
  in the configuration document itself, compilation fails before any
  output, even when a section exists in an include.
- The existing per-profile chain-wide `UnknownProfile` error keeps
  precedence: a profile absent from the whole chain still reports the
  frozen per-profile form.
- Flat configurations (no `[default]`, no profile structure) keep the
  lenient path — a profile section supplied by an include remains
  legal and boots, matching the filesystem loader.
- No runtime change and no camel-config change: both already enforce
  the strict rule; the compiler aligns to them.
- Spec delta on `config-loader-semantics` locks the mirror.
- Tests: unit tests in `compile::sources`, plus compile/parity
  integration tests and fixtures with locked stderr and resolved-config
  goldens.

Excluded: making profile fragments the authoritative runtime overlay
(dissolves M-1 dual-derivation) — a separate redesign, not this fix.

## Acceptance criteria

- Compiling `[default]`-carrying config with an include-only selected
  profile exits non-zero, names the offending profile(s), and writes
  no artifact.
- Mixed multi-profile selection stays acceptable: `[default]` +
  `[prod]` in config + `[canary]` only in an include compiles and
  boots, matching the filesystem loader.
- The partial multi-profile absence golden
  (`partial_absence_error.txt`) stays byte-identical.
- Flat config + include-only profile still compiles; the artifact's
  resolved configuration matches the filesystem loader golden.
- The gate decides through `camel_dsl::config_semantics`
  `has_selected_profile` / `has_profile_structure` delegation, with no
  local predicate copy.
- `openspec validate profilestrict --type change` passes.

## Risk budget

Low. One additive compile-time gate in `camel-cli`; runtime, loader,
trailer, and manifest stay untouched. Acceptance-region regression is
guarded by the existing and new goldens. Any change to frozen error
strings is out of bounds.
