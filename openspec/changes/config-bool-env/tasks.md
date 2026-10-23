# Tasks: config-bool-env

## camel-config (TOML arm)

### Task 1.1: Extend the TOML typed probe to booleans

**Files:**
- `crates/camel-config/src/env_int_probe.rs` (modified)
- `crates/camel-config/tests/placeholder_e2e.rs` (modified)

**Steps:**
1. In `crates/camel-config/src/env_int_probe.rs`, add `fn clean_bool(s: &str) -> Option<bool>` directly below `fn clean_i64`: return `Some(true)` for exactly `"true"`, `Some(false)` for exactly `"false"`, `None` otherwise (byte-exact match; no trim, no case folding, no aliases). Doc comment states the strict lexical gate and carries a `// SYNC:` note mirroring camel-dsl's `clean_boolean` (same pattern as the existing `clean_i64`/`clean_integer` SYNC note).
2. In `fn deserialize_with_probe`, change the candidate vector from `Vec<(Vec<ConfigSeg>, i64)>` to `Vec<(Vec<ConfigSeg>, toml::Value)>`; the candidate scan becomes: `Some(toml::Value::String(s))` → `clean_i64(s).map(toml::Value::Integer).or_else(|| clean_bool(s).map(toml::Value::Boolean))`.
3. In the subset loop, replace the `*leaf = toml::Value::Integer(*number)` write with `*leaf = value.clone()` (the stored `toml::Value`). Do not touch `MAX_PROBE_CANDIDATES`, `next_combination`, the first-pass oracle, or the final `Err(first_err)`.
4. In `crates/camel-config/tests/placeholder_e2e.rs`, add the tests listed below, mirroring the existing harness (`common::env_lock()`, `ensure_unset`, `EnvCleanup`, `write_main`, `load`).

**Tests:** (executable spec — name, arrange, act, assert)
- `placeholder_bool_field_default_coerces` (FAILS on main): `ensure_unset("CFG_WATCH")` → write `watch = "${env:CFG_WATCH:-true}"\n` → `load(&dir)` must be `Ok` and `cfg.watch == true`. Command: `cargo test -p camel-config --test placeholder_e2e placeholder_bool_field_default_coerces`.
- `placeholder_bool_field_resolved_env_coerces` (FAILS on main): `EnvCleanup::set("CFG_WATCH", "false")` → write `watch = "${env:CFG_WATCH:-true}"\n` → `load` must be `Ok` and `cfg.watch == false`. Command: `cargo test -p camel-config --test placeholder_e2e placeholder_bool_field_resolved_env_coerces`.
- `literal_quoted_boolean_still_rejected_alongside_probe` (passes on main, regression pin): write `log_level = "${env:CFG_LL_OK:-info}"\nwatch = "true"\n` (no token on the bool leaf) → `load` must be `Err` and the message must contain `watch`. Command: `cargo test -p camel-config --test placeholder_e2e literal_quoted_boolean_still_rejected_alongside_probe`.
- `string_field_boolean_default_stays_string` (passes on main, mirror pin): `ensure_unset("CFG_LL")` → write `log_level = "${env:CFG_LL:-true}"\n` → `load` must be `Ok` and `cfg.log_level == "true"`. Command: `cargo test -p camel-config --test placeholder_e2e string_field_boolean_default_stays_string`.
- `placeholder_bool_field_out_of_set_keeps_first_pass_error` (passes on main, must stay): `ensure_unset("CFG_WATCH")` → loop over defaults `yes`, `TRUE`, `1`, `true ` (trailing space) writing `watch = "${env:CFG_WATCH:-<default>}"` → each `load` must be `Err` with a message containing `watch`. Command: `cargo test -p camel-config --test placeholder_e2e placeholder_bool_field_out_of_set_keeps_first_pass_error`.
- `probe_cap_eight_bools_one_int_nine_candidates_fails` (passes on main, must stay): `ensure_unset` for `CFG_CAPB1`..`CFG_CAPB8` and `CFG_CAPB_INT` → write a document with EIGHT boolean candidates (`watch = "${env:CFG_CAPB1:-true}"`; `[observability.prometheus] enabled = "${env:CFG_CAPB2:-true}"`; `[observability.health] enabled = "${env:CFG_CAPB3:-true}"`; `[observability.otel] enabled = "${env:CFG_CAPB4:-true}"` and `logs_enabled = "${env:CFG_CAPB5:-true}"`; `[binds."127.0.0.1:8081"]` / `[binds."127.0.0.1:8082"]` / `[binds."127.0.0.1:8083"]` each `allow_public_exposure = "${env:CFG_CAPB6:-false}"` / `"${env:CFG_CAPB7:-false}"` / `"${env:CFG_CAPB8:-false}"`) PLUS one integer candidate (`timeout_ms = "${env:CFG_CAPB_INT:-5000}"`) — nine combined, over the cap → `load` must be `Err` containing `Failed to deserialize merged config`. Companion (within the cap, mixed kinds): the same document WITHOUT the third binds entry (`[binds."127.0.0.1:8083"]`) — seven booleans + one integer, eight combined — must `load` `Ok` with `cfg.watch == true` and `cfg.timeout_ms == 5000`. If a section rejects a bare `enabled`/`allow_public_exposure` for a missing required sibling field, fill that sibling with a literal (non-placeholder) value; never add placeholder candidates. Command: `cargo test -p camel-config --test placeholder_e2e probe_cap_eight_bools_one_int_nine_candidates_fails`.

**Acceptance:**
- `cargo test -p camel-config --test placeholder_e2e` passes (all new and existing tests).
- `cargo test -p camel-config` passes.
- The two FAILS-on-main tests fail on the pre-task tree when run before implementation (worker verifies first, then implements).
- `cargo fmt --check` and `cargo clippy -p camel-config -- -D warnings` exit 0.

- [x] 1.1

## camel-dsl (YAML arm)

### Task 2.1: Extend the YAML typed probe to booleans

**Files:**
- `crates/camel-dsl/src/env_int_probe.rs` (modified)

**Steps:**
1. Add `fn clean_boolean(s: &str) -> Option<serde_yml::Value>` directly below `fn clean_integer`: `Some(serde_yml::Value::Bool(true))` for exactly `"true"`, `Some(serde_yml::Value::Bool(false))` for exactly `"false"`, `None` otherwise. Doc comment states the strict lexical gate; extend the existing `// SYNC:` note on `clean_integer` to mention the boolean mirrors (`clean_bool` in camel-config).
2. In `fn candidates`, the second filter becomes: resolved `Value::String(s)` passes `clean_integer(s).is_some() || clean_boolean(s).is_some()` (the `routes:`-only scope filter is unchanged).
3. In `fn parse_with_probe`, the candidate-collection loop pairs each accepted path with its coerced value: try `clean_integer(s)` then `clean_boolean(s)` on the resolved string; push the resulting `serde_yml::Value` into `coerced_values`. `try_coerced_subset` needs no change (it already writes arbitrary `serde_yml::Value` leaves). Cap, smallest-first enumeration, QUIET oracle, and final `Err(first_error)` are untouched.
4. In the `#[cfg(test)] mod tests` of the same file, add the tests below using the existing `probe_defaults` harness and `interpolate_yaml_source_with_provenance`; for the explicit-lookup test, interpolate with a lookup closure returning `Some("true".to_string())` for `RC_AUTO` before calling `parse_with_probe`.

**Tests:** (executable spec — name, arrange, act, assert)
- `probe_fixes_bool_route_field_with_default` (FAILS on main): raw doc = `"routes:\n  - id: bool-one\n    from: \"direct:start\"\n    auto_startup: ${env:RC_AUTO:-false}\n    steps:\n      - throttle:\n          max_requests: 1\n          period_secs: 1\n"` → `probe_defaults(raw)` must be `Ok` and `routes[0].auto_startup() == false`. Command: `cargo test -p camel-dsl --lib probe_fixes_bool_route_field_with_default`.
- `probe_bool_route_field_honors_explicit_lookup` (FAILS on main): same doc with `auto_startup: ${env:RC_AUTO:-false}` → interpolate with lookup `|name| (name == "RC_AUTO").then(|| "true".to_string())` → `parse_with_probe` must be `Ok` and `routes[0].auto_startup() == true`. Command: `cargo test -p camel-dsl --lib probe_bool_route_field_honors_explicit_lookup`.
- `literal_quoted_bool_at_bool_position_stays_rejected` (passes on main, regression pin): raw doc with `auto_startup: "true"` (quoted, no token) → `probe_defaults` must be `Err` containing `type mismatch: expected bool, found string` (the direct-bool-field pass-1 error; it names the field). Command: `cargo test -p camel-dsl --lib literal_quoted_bool_at_bool_position_stays_rejected`.
- `probe_string_position_bool_default_stays_string` (passes on main, mirror pin): raw doc with `- set_header:\n          key: k\n          value: ${env:RC_STR:-true}` and no bool-typed placeholder → `probe_defaults` must be `Ok` and the header value equals `ValueSourceDef::Literal(serde_json::Value::String("true".into()))` (reuse `assert_set_header_string`). Command: `cargo test -p camel-dsl --lib probe_string_position_bool_default_stays_string`.
- `probe_bool_out_of_set_keeps_pass1_error` (passes on main, must stay): for each default in `yes`, `TRUE`, `1`, `true ` (trailing space) build the `auto_startup: ${env:RC_B:-<default>}` doc → `probe_defaults` must be `Err` containing `type mismatch: expected bool, found string`. Command: `cargo test -p camel-dsl --lib probe_bool_out_of_set_keeps_pass1_error`.
- `probe_cap_eight_bools_one_int_nine_candidates_fails` (passes on main, must stay): build a nine-route document — routes 1-8 each `auto_startup: ${env:RC_CAPB_<i>:-false}` (EIGHT boolean candidates, `ensure_unset` each `RC_CAPB_1`..`RC_CAPB_8`), route 9 with `throttle: {max_requests: ${env:RC_CAPB_INT:-2}, period_secs: 1}` (one integer candidate) — nine combined, over the cap → `probe_defaults` must be `Err` containing `type mismatch: expected bool, found string` (the first-pass direct-bool error). Companion (within the cap, mixed kinds): the same document WITHOUT route 8's bool candidate — seven booleans + one integer, eight combined — must be `Ok` with `auto_startup() == false` for the seven placeholder-bearing routes (1-7), and `auto_startup() == true` for routes 8-9 (no `auto_startup` key → the default), plus the throttle route's `max_requests == 2`. Command: `cargo test -p camel-dsl --lib probe_cap_eight_bools_one_int_nine_candidates_fails`.

**Acceptance:**
- `cargo test -p camel-dsl --lib` passes (all new and existing tests, including the existing `probe_attempts_log_nothing` / `probe_final_failure_logs_exactly_one_error` logging contract).
- `cargo test -p camel-dsl` passes.
- The two FAILS-on-main tests fail on the pre-task tree when run before implementation (worker verifies first, then implements).
- `cargo fmt --check` and `cargo clippy -p camel-dsl -- -D warnings` exit 0.

- [x] 2.1

## docs

### Task 3.1: Document the boolean rule in env-interpolation.md

**Files:**
- `docs/src/configuration/env-interpolation.md` (modified)

**Steps:**
1. Load the `ste-writing` skill rules before editing (docs/AGENTS.md mandates ASD-STE100 for this directory); keep sentences short, imperative, and factual.
2. Add a `## Typed values` section directly after `## Syntax`: strings pass through everywhere. Integer-typed fields coerce through the typed probe after a failed strict parse (already documented behavior — reference it in one sentence). Boolean-typed fields coerce only the exact lowercase literals: a placeholder whose resolved value is `true` or `false` becomes a real boolean. Give a one-line `Camel.toml` example: `watch = "${env:WATCH:-true}"` and a one-line YAML route example: `auto_startup: ${env:AUTO_START:-false}`. Tag any new fenced block with a language (`toml` / `yaml`) so `mdbook test` does not compile it as Rust.
3. In the same section, state the rejection rule with the same honesty: any other spelling fails typed deserialization — on `Camel.toml` the error names the field, in route files the load fails with the original parse error. No `TRUE`, no `yes`/`no`, no `1`/`0`, no surrounding whitespace, and a literal quoted `"true"` (no placeholder) never coerces.
4. Verify with the mdbook gates (below); do not touch SUMMARY.md (section sits on an existing page).

**Tests:** (executable spec — name, arrange, act, assert)
- mdbook build: `nix shell nixpkgs#mdbook -c mdbook build docs` → exit 0, every include/link resolves.
- mdbook test: `nix shell nixpkgs#mdbook -c mdbook test docs` → exit 0; the new fenced blocks are language-tagged and skipped as Rust.
- Content assertion: the section names the four rejected classes (`TRUE`, `yes`/`no`, `1`/`0`, whitespace) and both accepted literals exactly.

**Acceptance:**
- Both mdbook gates exit 0.
- No new untagged code fences (grep the diff for ``` lines lacking a language tag).
- Prose follows ASD-STE100 per the `ste-writing` skill.

- [x] 3.1
