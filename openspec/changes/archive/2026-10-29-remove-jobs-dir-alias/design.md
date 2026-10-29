# Design: remove-jobs-dir-alias

## Approach

**Model (camel-config).** Delete `dir: Option<String>` from `JobsCamelConfig`. `resolved_dirs()` collapses to two branches: `dirs.clone()` when `dirs` is `Some` (an explicit `[]` means "no roots"), else `vec![default_jobs_dir()]` — no alias branch remains. Replace the derived `Deserialize` + `#[serde(deny_unknown_fields)]` with a hand-written impl (visit_map): key `dirs` deserializes `Option<Vec<String>>` (absent key → `None`, replacing the removed `#[serde(default)]` helper); key `dir` returns a custom error; any other key returns `Error::unknown_field(other, &["dirs"])` so the existing unknown-key contract (`jobs_table_unknown_key_rejected`) keeps its `unknown field` message. Keep `Debug, Clone, Default, PartialEq` derives and the struct literal surface minus `dir`.

**Tombstone.** Message, modeled on the landed ws tombstone (`camel-ws/src/config.rs` `reconnect_tombstone`: "`{param}` URI parameter was removed; set `{toml_field}` ..."):

`dir` was removed from the `[jobs]` table; use `dirs = ["<path>"]`

It names the removed key (`dir`) and the replacement (`dirs`) with the migration spelling, satisfying ADR-0090's "removed keys fail at load with a tombstone error naming the replacement". `deny_unknown_fields` alone is not enough: serde's generic message carries no migration text. A field-level `deserialize_with` tombstone was rejected because the mission (and ADR-0090's "delete, not hide") requires the field itself gone — the hand-written impl is the only shape that deletes the field and still produces a tailored message. The tombstone fires identically for direct `toml::from_str::<CamelConfig>` loads and for the profile-merge path (`build_from_toml_value_inner` merges the TOML value key-by-key, then deserializes), so a `[development.jobs] dir = ...` profile also fails closed. No `#[deprecated]` anywhere (lint-no-deprecated).

**CLI (camel-cli).** The job command warns on stderr when a merged profile carries an ignored `dir` next to `dirs` (`commands/job/mod.rs`, `jobs_roots` diagnostic surface). With the field gone this code cannot compile — delete the warning path, update the two surviving `JobsCamelConfig` struct literals (`commands/job/tests.rs` ≈L1797 `dir: None` dropped in the kept duplicate-root test; `commands/job/job_effective_config_tests.rs` ≈L111-112 `dir:` line deleted and the existing `dirs: None` changed to `dirs: Some(vec!["job-roots"])`), and drop the alias-only CLI tests.

**Tests.** camel-config (`config_tests/jobs_config_tests.rs`): remove `legacy_jobs_dir_alias_is_supported`, `explicit_job_dirs_override_legacy_dir`, `jobs_table_dir_round_trip`; rework `empty_dirs_override_legacy_dir` into a `dirs = []`-alone test, fix the `jobs_overlay_overrides_default` struct literal, replace `merged_profile_keeps_both_keys_dirs_wins` with a merged-profile tombstone test (build must fail naming `dir`/`dirs`), fix the `jobs_table_no_unselected_profile_warning` fixture to `dirs`. Add `jobs_dir_alias_is_a_tombstone`: `toml::from_str::<CamelConfig>` of `[jobs]\ndir = "legacy"` errors; the message contains `dir`, `dirs`, and the `dirs = [` migration spelling. camel-cli (`tests/job_one_shot_test.rs`): remove `explicit_job_dirs_override_legacy_dir` and `listing_warns_on_profile_shadowed_legacy_dir`; add a tombstone load-failure test (stderr names `dir` and `dirs`). First step everywhere: workspace grep for `jobs.dir` / `dir =` fixtures so no site is missed.

**Spec delta.** The alias is not a standalone requirement in `openspec/specs/cli-jobs/spec.md` — its normative text ("Legacy `dir` SHALL fold into one `dirs` entry." and the `dir`/`dirs` precedence sentence) and its two scenarios ("Legacy directory alias", "Explicit dirs take precedence") live inside "job discovery and no-argument listing". OpenSpec deltas operate at requirement granularity, so the mission's `## REMOVED Requirements` for the alias is expressed as a `## MODIFIED Requirements` entry that reproduces the requirement without the alias text/scenarios and adds the tombstone SHALL + scenario. This deviation is recorded here and in the park report; `openspec validate --all --strict` gates it.

**Docs.** CONTEXT-MAP.md L106 (ADR-0062 line) and L206 (`[jobs].dirs` bullet) drop the "legacy `[jobs].dir`" phrasing and note the tombstone; `crates/camel-cli/CONTEXT.md` (≈L107) swaps its alias-compatibility promise for the tombstone wording; ADR-0062 gets a short amendment (L131, L167 describe the alias as current behavior); `docs/src/configuration/schema.md` drops the `jobs.dir` row. Migration text everywhere: `dirs = ["<path>"]`.

## Affected crates

- `camel-config`: struct + `Deserialize` tombstone + `resolved_dirs()` + config tests.
- `camel-cli`: job command warning removal, job command unit tests (including `job_effective_config_tests.rs` struct-literal fix), `job_one_shot_test.rs` alias test removal + tombstone test, `camel-cli/CONTEXT.md` doc update.
- No API/DSL/component/service changes; no `Cargo.toml` dependency changes.

## Architecture boundaries

Config parsing stays in `camel-config` (the tombstone is a deserialization concern, not a CLI concern); the CLI keeps only its stderr diagnostic surface, which shrinks by one warning. Job discovery semantics (`named job resolution`, ordering, bare-name rules) are untouched — the mission's non-goals hold.

## Alternatives considered

- Keep `dir` behind a `deserialize_with` tombstone: rejected — leaves the alias in the model, against Required 1 and ADR-0090's delete-not-hide.
- Rely on `deny_unknown_fields` after field deletion: rejected — generic `unknown field` message lacks the replacement spelling the policy demands; the spec scenario asserts the migration text.
- Rename-only deprecation warning for a release: rejected — ADR-0090 forbids `#[deprecated]` and schedules surface removal pre-1.0.
