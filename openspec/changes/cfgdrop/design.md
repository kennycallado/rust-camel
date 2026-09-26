# Design: cfgdrop

## Context

Loader flow under fix (`crates/camel-config/src/config.rs`):

1. `load_from_file_inner` parses the root document, extracts/strips
   `include` keys, and turns each include into a typed-layer config
   source (`pre_sources`, lowest priority; root wins).
2. `build_from_toml_value_inner` resolves `CAMEL_PROFILE`, emits the
   rc-cflo warning (profile-like unknown tables, no active profile
   only), defensively strips a root `include` key, computes
   `has_profile_structure`, then applies the strict or lenient
   selection (`select_profile_sections`) and builds the typed config.
3. Env allowlist overrides apply LAST, at the typed layer — they can
   never trip a document-shape check placed before selection.

`select_profile_sections` (camel-dsl canonical helper): with
`[default]` present, base = `[default]`, selected profiles deep-merge
on top, and `*value = base` — every other root key is discarded. With
no `[default]` and no selected profile section, the flat document is
kept as-is (nothing is dropped).

## Decision: reject-with-error (not warn, not merge)

Disposition audit against house precedent:

| Class | Existing policy | Source |
|---|---|---|
| Selected profile absent while `[default]` present | hard error `Unknown profile: {}` | `apply_profile` |
| Profile-like unknown root table, no active profile | warn (rc-cflo) | `build_from_toml_value_inner` |
| Misplaced shapes at compile time (bean plugins, WASM) | hard error | `compile::policy::reject_config_assets` |
| Invalid values (`runtime_journal.path` empty) | hard error | `validate` |

Known config keys misplaced at the root of a profile-structured
document can never take effect — there is no "activate later" reading
( unlike an unselected `[qa]` sibling, which warn-class serves). The
documented contract agrees: `docs/src/configuration/schema.md` states
the top-level fields "live directly under `[default]`"; the bare
root-level form is the flat (profile-less) document. Misuse that is
silently ignored is the fail-loud violation class; reject matches
precedent.

Why not merge: merging root known keys into the selection base would
change `select_profile_sections` (canonical helper), alter resolved
config bytes on every consumer, break the frozen parity goldens
(`Resolution behavior is frozen by parity goldens` — resolved bytes
and error strings SHALL NOT change), and invent a third document shape
the docs never promised. Rejection is additive: the canonical helpers,
the compile path, and all parity goldens stay untouched.

## The guard

In `build_from_toml_value_inner`, inside the `has_profile_structure`
branch, before `apply_profile`:

- Collect root-table keys (any value type — tables and scalars are the
  same silent-drop class) that are members of `KNOWN_TOP_LEVEL_KEYS`.
- Non-empty → `ConfigError::Message` naming every offending key and
  the accepted shapes: move the key(s) under `[default]` (or the
  selected profile section), or remove the profile sections to use a
  flat document.
- `include` is already stripped before the check; `default` and
  selected profile names are not in `KNOWN_TOP_LEVEL_KEYS`, so
  sections never trip it. Unknown root tables fall through to the
  rc-cflo policy (warn when no profile is active), unchanged.

## Non-goals / edges verified

- **Flat include beside `[default]` main**: includes become
  `pre_sources` at the typed layer; they never merge into the
  pre-selection root value, so the documented pattern is unaffected.
- **`from_toml_value_with_env`** (virtual-store seam): receives a tree
  already selected at compile time; no structure, no guard.
- **Compile path**: `camel compile` parses documents directly
  (`compile::sources`); root `routes` participates in documented
  compile-time pattern accumulation. Parity fixtures
  (`config-parity-project/Camel*.toml`) mix root `routes` with
  `[default]` and only ever flow through compile — they never reach
  the camel-config filesystem loader. The camel-cli compile goldens
  stay byte-identical.
- **camel-config parity battery, case h** (found in implementation):
  `parity_golden_tests::parity_section_routes_replace_toplevel` used
  an INLINE Rust fixture mixing root `routes`/`timeout_ms`/`watch`
  with `[default]` and its golden (`case_08.json`) locked the silent
  discard itself (`watch=false` proved the root keys were dropped).
  A repo scan of committed `.toml` files missed it — inline string
  fixtures need a source scan too. Disposition: the live
  section-over-section `routes` replacement semantic is already
  locked by the profile deep-merge case (`[default]` routes replaced
  by `[production]` routes), so case h converts to an error lock —
  the mixed document now asserts the rejection with its full error
  `Display` locked byte-for-byte (`case_08_error.txt`, the battery's
  error-lock convention). The frozen-parity requirement is carried as
  MODIFIED accordingly (bd rc-9spgd).
- **Pre-existing asymmetry** (compile reads root `routes` for
  discovery while the runtime loader rejects the same shape) is
  recorded as a deferral with follow-up context, not silently
  absorbed.
- Repo scan: zero non-compile committed TOML documents mix root known
  keys with profile structure, so no example, benchmark, or test
  fixture regresses.

## Tests

In `crates/camel-config/src/config_tests/profile_loading_tests.rs`
(the loader-semantics battery; same style as the cfgempty-era tests):

1. Root `[runtime_journal]` + `[default]` → error naming
   `runtime_journal`, `[default]`, and the flat alternative (gh#52
   case).
2. Root scalar `log_level` + `[default]` → error (scalar class).
3. Root `[runtime_journal]` + `[prod]` selected (no `[default]`) →
   error (structure via selection).
4. Nested `[default.runtime_journal]` still loads and takes effect.
5. Flat document with root `[runtime_journal]` (no profile structure)
   still loads and takes effect.
6. rc-cflo warn behavior re-locked by existing ergonomics tests
   (no changes expected there).
