# Design: cfgdrop2

## Approach

Three surgical edits on top of the landed cfgdrop guard
(`camel-config/src/config.rs`, `build_from_toml_value_inner`), plus
one compile-side mirror. No change to
`camel_dsl::config_semantics` canonical helpers.

**1. Shared root-key policy module (camel-config).** New `pub mod
root_key_policy` in camel-config: `is_known_top_level_key(name)` over
the existing `KNOWN_TOP_LEVEL_KEYS` set, `is_root_routes_exception()`
constant, and `near_miss_root_table(name) -> Option<&'static str>` —
`Some(target)` when `name` is NOT a known key, the entry is
table-valued at the root, and Levenshtein(name, target) ≤ 2 for a
target in the length-≥ 8 subset of `KNOWN_TOP_LEVEL_KEYS`
(`runtime_journal`, `idempotent_repo`, `drain_timeout_ms`,
`watch_debounce_ms`, `observability`, `stream_caching`,
`datasources`, `supervision`, `components`, `cache_repo`,
`timeout_ms`, `log_level`, `languages`, `security`, `platform` —
the ≥ 8 filter keeps short plausible profile names like
`job`/`bind`/`dev`/`qa` out of match range). Bounded
in-place Levenshtein, no dependency. Unit tests over the matcher
table (typo matrix × far-name matrix).

**2. Runtime guard rework (camel-config).** In the
`has_profile_structure` branch:
- Discarded set = known keys MINUS `routes`.
- Near-miss set = root table keys hitting
  `near_miss_root_table`; each rejected naming the probable intended
  key ("looks like a misspelling of '<target>'"), same accepted-shapes
  guidance as the 288 error.
- Root `routes` exception: before `apply_profile`, lift the root
  `routes` value out of the tree; after selection, if the selected
  tree lacks `routes`, reinsert the lifted value. This gives exactly
  compile's overlay semantic (`compile::sources` lines 766–781: root
  `routes` is the base, then `[default]`, then each selected profile
  section replaces) because `merge_toml_values` array-replacement
  already makes any declaring section win. Flat documents untouched.
- rc-cflo warn: exclude near-miss names from the `profile_like` warn
  set (they hard-error in the guard now); warn semantics otherwise
  unchanged.
- `case_08_error.txt` regenerates (mixed root
  `routes`+`timeout_ms`+`watch`+`[default]` now names only
  `timeout_ms`, `watch`).

**3. Compile mirror (camel-cli `compile::sources`).** After the
config document parses (beside `policy::reject_config_assets`), when
`has_profile_structure(&config, &selection.profiles)` (canonical
predicate): reject root keys that are known-but-not-`routes`, and
near-miss root tables, via the imported `camel_config::root_key_policy`
helpers. Error strings stay consumer-local (canonical-helpers rule):
same key-naming + accepted-shapes class as the loader. Root `routes`
keeps its documented pattern-accumulation role untouched — compile
parity goldens stay byte-identical (fixtures mix only root `routes` +
profile sections; verified by scan in Task 2, any stray fixture
converts to an error lock and is reported).

**4. Parity asserts.** A shared fixture-matrix test: the same
document strings drive (a) `CamelConfig::from_file_with_profile` and
(b) the compile guard path, asserting disposition equality per shape
class. In-crate battery extensions cover each class individually;
cross-path equality is asserted in the camel-cli test (camel-cli
already depends on camel-config).

## Affected crates

- camel-config: policy module (new, pub), guard rework, routes
  overlay survival, battery extensions, `case_08_error.txt` regen,
  README + `docs/src/configuration/schema.md` notes.
- camel-cli: `compile::sources` guard, compile tests + parity
  asserts, fixture scan.
- camel-dsl: NOT touched (canonical helpers unchanged).
- openspec: `config-loader-semantics` delta (MODIFIED the 288
  requirements carrying scenario names + ADDED near-miss requirement).

## Architecture boundaries

Config layer only (camel-config + the compile config front door in
camel-cli). No Runtime, DSL semantics, Component, or Language
surfaces. The canonical-semantics single-source rule is respected:
the near-miss predicate is name policy, not a `toml::Value`
transform, so it lives in camel-config (the policy owner) rather than
camel-dsl; both consumers import the one copy (no SYNC fork).

## Phases

Single phase — two implementation tasks with a hard seam (camel-config
first: the policy module + guard + battery; camel-cli second: mirror
+ parity asserts consuming the pub helper).
