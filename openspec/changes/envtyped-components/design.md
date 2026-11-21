# Design: envtyped-components

## Approach

One seam, zero per-bundle changes. The resolver
(`resolve_tree_with_provenance`, `crates/camel-config/src/config.rs:3324`)
already records provenance for the whole tree, including `components.*`
(`placeholder_e2e.rs:190` proves the walk reaches `components.redis.url`).
The problem is purely reachability: provenance is dropped after
`CamelConfig` deserialization, which succeeds whenever only component blocks
carry placeholders (`ComponentsConfig.raw` accepts any string).

1. **Reachable provenance.** `ComponentsConfig` gains a
   `#[serde(skip)]` per-block store `provenance: HashMap<String, ProvenanceSet>`
   (paths relative to `components.<key>`, split from the whole-tree set by a
   `pub(crate)` splitter that requires the prefix
   `[Key("components"), Key(block)]`, preserving the resolver's traversal
   order — table map iteration, then array index order). `ProvenanceSet`
   becomes a public opaque type (private inner vec; `ConfigSeg` stays
   `pub(crate)` — callers never touch segments). Public cross-crate accessor
   `provenance_for(key) -> &ProvenanceSet` returns an empty set when absent.
   The two load entry points (`load_from_file_inner`,
   `from_toml_value_with_env`) attach the split after
   `deserialize_with_probe`. `raw` keeps its type; `raw` values stay resolved
   strings — coercion never rewrites them.
2. **Generic helper.** `deserialize_block_with_probe<T, E>(block, provenance,
   parse)` in `env_int_probe.rs`, re-exported at the crate root: first pass is
   `parse(block.clone())` (the strict oracle — for the root arm, the unchanged
   `try_into::<CamelConfig>` with its exact `Failed to deserialize merged
   config: {e}` wrapping; for the bundle seam, `B::from_toml`); on error,
   candidate leaves (provenance paths whose leaf is a `String` accepted by
   `clean_i64`/`clean_bool`) are coerced in cloned trees, index subsets tried
   ascending size lexicographic, cap 8, first success wins, otherwise the
   first-pass error is returned verbatim. The parse closure owns
   deserialization, so `T` needs no `DeserializeOwned` bound — that is the
   strictly more general shape of "generic over the target type": the
   `try_into` oracle is one closure, `from_toml` is another. `deserialize_with_probe`
   becomes a one-line wrapper; the root-field TOML arm's semantics and cap are
   untouched. Worst-case bound: at most `2^8 − 1 = 255` subset retries plus
   the first pass (256 deserialize attempts). The bundle seam's per-block cap
   applies after a successful root load; the root-field probe keeps its
   existing whole-tree candidate count and cap (pinned by the
   `root_probe_still_counts_component_candidates` scenario).
3. **Bundle seam.** `bundle_from_config` (`crates/camel-bundles/src/lib.rs:215`)
   becomes one call:
   `deserialize_block_with_probe(&raw, provenance_for(config_key), B::from_toml)`.
   First-pass failure inside the helper is today's behavior; coercion is
   scoped to the block being deserialized because the provenance passed in is
   the per-block split; no subset parsing returns the first-pass `CamelError`
   verbatim.

**Why this is safe:** coercion runs only after a real deserialization failure,
so a string field whose placeholder resolves to `"1234"` deserializes on the
first pass and keeps its string; a literal quoted numeric with no token is
never in the provenance set, so it is never a candidate; the target type is
enforced by the bundle's real `try_into` inside `from_toml`, never by a
heuristic. The probe cap keeps the worst case at 255 subset retries plus the
first pass.

**Stale-provenance boundary:** the guarantee covers unmodified loader-produced
blocks. `ComponentsConfig.raw` is a public field, so a caller that edits it
after load invalidates the recorded provenance; such callers MUST rebuild
(default-construct `ComponentsConfig`, assign the edited map) so no stale path
coerces a token-free value. The accessor's doc comment carries this rule.
Automatic invalidation of arbitrary public mutations would be a separate
design and is out of scope.

**Source-compat note:** the new serde-skipped field makes external
`ComponentsConfig { raw }` struct literals a missing-field error (pre-1.0,
ADR-0090). Exactly one in-repo construction site exists
(`crates/camel-cli/src/commands/job/job_effective_config_tests.rs:51`) and is
updated to default-construct `ComponentsConfig` and assign `raw` (cross-crate
struct literals cannot name the private field, so literal-plus-update syntax
is not an option). `ComponentsConfig`'s `PartialEq` is
implemented manually over `raw` only — provenance is metadata about how values
arrived, not a value, and derived equality would break existing
default-vs-loaded comparisons.

**Quota probe:** `@experts/e_gpt` probe dispatch before the first bless
returned AVAILABLE (2026-11-21); blessings use `@experts/e_gpt` as ordered.

## Affected crates

- `camel-config`: `ComponentsConfig` provenance store + accessor; public
  `ProvenanceSet` + `deserialize_block_with_probe`; `deserialize_with_probe`
  wrapper; both load entry points attach the split. Unit tests next to the
  existing probe tests; e2e in `tests/` (load → block provenance).
- `camel-bundles`: `bundle_from_config` calls the helper. E2E tests for redis,
  http, ws, string-mirror, unrelated-error.
- `camel-cli`: one test construction site
  (`crates/camel-cli/src/commands/job/job_effective_config_tests.rs:51`) is
  updated to default-construct `ComponentsConfig` and assign `raw`.
- `docs`: `docs/src/configuration/env-interpolation.md` — "Component
  configuration" section (typed-leaf rule; `raw` keeps strings).
- `AGENTS.md`: `camel-config | local | cargo test -p camel-config` lane row
  (camel-config has a `tests/` surface and no row).

Not touched: `camel-component-api`'s `ComponentBundle` trait, the 48
`from_toml` impls, `camel-dsl`, any other camel-config load seam.

## Architecture boundaries

Config (camel-config) stays pure — no component-crate dependency; the helper
is consumed by camel-bundles which already depends on camel-config. The DSL is
untouched (rc-3c67o.2 owns route-document seams). Components are untouched:
the coercion happens before their `from_toml` runs and reuses its existing
strictness. Single source of truth for the clean gates remains
`camel-config/src/env_int_probe.rs` (the camel-dsl and camel-lint mirrors are
unchanged by this change).

## Alternatives considered

- Probe consumes the whole-tree provenance plus the `components.<key>` prefix
  at the bundle seam — rejected: it would hand camel-bundles paths outside its
  block and make cross-block bleed a caller bug; the per-block split keeps
  scoping structural.
- Per-type schemas for component blocks / typed `ComponentsConfig` fields —
  rejected: 48 bundles, no single schema source; the order forbids it.
- Re-running the resolver with typed substitution inside each bundle —
  rejected: double resolution, provenance loss, and per-bundle changes.
- Storing provenance on `CamelConfig` (whole-tree) — rejected: manual
  `PartialEq` on the large config struct and a wider public surface for the
  same information.
