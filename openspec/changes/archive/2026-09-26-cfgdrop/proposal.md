# Proposal: cfgdrop — reject root-level config keys discarded by profile selection

## Why

GH #52 side note (bd rc-zbyyv): when a document carries a `[default]`
table (or a selected `[<profile>]` section), the filesystem loader
keeps ONLY the walked sections — `select_profile_sections` replaces
the whole document with `[default]` deep-merged with the selected
profiles. A top-level `[runtime_journal]` table outside any section is
SILENTLY discarded; the journal only takes effect nested as
`[default.runtime_journal]`. The rc-cflo warning cannot catch this
class: it deliberately excludes `KNOWN_TOP_LEVEL_KEYS` (a
`[runtime_journal]` next to `[default]` is a real config section, not
a profile-like table), so known keys are dropped with zero signal.
Silent config dropping violates the fail-loud house rule.

## What Changes

- `build_from_toml_value_inner` (camel-config) gains a reject-with-error
  guard: when the document has profile structure (`[default]` present
  or a selected profile section present — the strict path that is
  about to discard root-level keys) AND the document root carries any
  key from `KNOWN_TOP_LEVEL_KEYS`, loading fails with a
  `ConfigError::Message` naming the offending key(s) and the accepted
  shapes (move the key under `[default]` / the selected profile
  section, or remove the profile sections to use a flat document).
- Flat documents (no profile structure) are unchanged: root-level keys
  ARE the configuration there. Include files are unchanged: they are
  processed into config sources at the typed layer and never enter the
  pre-selection root value, so the documented `[default]`-main +
  flat-include pattern keeps working.
- Unselected profile-like tables (the rc-cflo class) keep their
  existing warn policy, untouched.
- The compile path (`camel compile`) is untouched: it parses documents
  directly and its parity goldens stay byte-identical.

## Capabilities

Delta on `config-loader-semantics` (ADDED requirement): the spec
canonizes profile-section selection semantics for this exact loader;
the new consumer-local rejection policy joins it as a new requirement.

## Impact

- `crates/camel-config` (`src/config.rs` guard + tests in
  `src/config_tests/profile_loading_tests.rs`).
- Docs: `crates/camel-config/README.md` and
  `docs/src/configuration/schema.md` gain one clarifying line each
  (root-level config keys are flat-document-only; mixing with
  `[default]`/profile sections is rejected).
- Affected specs: `config-loader-semantics` (ADDED requirement).
- bd rc-zbyyv.
