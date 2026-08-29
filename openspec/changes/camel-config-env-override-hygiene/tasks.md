# Tasks: camel-config-env-override-hygiene

## Phase 1: typed contract + poison safety + docs

### camel-config

#### Task 1.1: Poison-safe test-only `env_lock()` helper replacing all lock acquisitions (bd rc-fd4f)

**Files:**
- `crates/camel-config/src/config.rs` (modified)

**Steps:**
1. Locate `ENV_OVERRIDE_LOCK` (declaration + setters are `#[cfg(test)]`-only, `config.rs:2980-3002`) and every direct acquisition site (`rg -n "ENV_OVERRIDE_LOCK" crates/camel-config/src/config.rs` — expect ~60 mentions, ~48 `.lock().unwrap()` acquisitions).
2. Add next to the lock declaration, inside the `#[cfg(test)]` region:
   `fn env_lock() -> std::sync::MutexGuard<'static, ()>` (match the lock's actual inner type at implementation time) whose body is `ENV_OVERRIDE_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner)`, with a doc comment stating: test-only coordination mutex; recovery from poison is safe because every env test restores vars before assertions.
3. Replace every `ENV_OVERRIDE_LOCK.lock().unwrap()` acquisition with `env_lock()`. Do not touch the declaration, setters, or doc comments that merely mention the lock.
4. Run the full `--lib` suite to confirm zero behavior change.

**Tests:** (executable spec)
- `env_lock_recovers_after_poison`: setup — new `#[test]` in the env-override test module; act — `let _ = std::panic::catch_unwind(|| { let _g = env_lock(); panic!("poison source"); });` then call `env_lock()` again; assert — the second call returns a usable guard without panicking (e.g. bind it and assert the guard is held). Command: `cargo test -p camel-config --lib env_lock_recovers_after_poison`. Expected: fails before implementation (helper does not exist), passes after.
- Whole existing suite as regression: `cargo test -p camel-config --lib` — expected: green before and after (refactor only).

**Acceptance:**
- `rg -n "ENV_OVERRIDE_LOCK\s*\.lock\(\)\.unwrap\(\)" crates/camel-config/src/config.rs` returns 0 matches.
- Exactly one definition of `env_lock` in the crate.
- `cargo check -p camel-config`, `cargo fmt --check`, `cargo clippy -p camel-config -- -D warnings`, `cargo xtask lint-unwrap` all exit 0.
- `cargo test -p camel-config --lib` passes.

- [x] 1.1

#### Task 1.2: `LEGACY_STRING_ENV_OVERRIDES` verbatim passthrough + unit-bearing duration errors (bd rc-xq3t)

Depends on: Task 1.1 (`env_lock()` is used by the new tests).

**Files:**
- `crates/camel-config/src/config.rs` (modified)

**Steps:**
1. Add below `STRING_ENV_OVERRIDES` (`config.rs:2519`):
   `const LEGACY_STRING_ENV_OVERRIDES: &[&str] = &["CAMEL_CACHE_REPO_BACKEND", "CAMEL_CACHE_REPO_PATH", "CAMEL_CACHE_REPO_STALE_RETENTION"];`
   with a doc comment: verbatim passthrough for pre-existing String-typed cache_repo vars; deliberately NOT in `STRING_ENV_OVERRIDES` (keeps the `STRING ⊆ EMPTY_SCALAR` subset assertion) and NOT in `EMPTY_SCALAR_ENV_OVERRIDES` (legacy vars keep their established empty behavior).
2. In the merge-loop dispatch where `STRING_ENV_OVERRIDES`/`CSV_ENV_OVERRIDES`/`EMPTY_SCALAR_ENV_OVERRIDES` are consulted, route legacy vars through the same verbatim-string arm as `STRING_ENV_OVERRIDES` (condition of the form `STRING_ENV_OVERRIDES.contains(v) || LEGACY_STRING_ENV_OVERRIDES.contains(v)`). Empty-skip and CSV dispatch stay untouched and ordered first as today.
3. Upgrade both cache_repo duration validation errors to the unit-bearing format:
   - `config.rs:1987` (stale_retention, non-memory backends): `cache_repo.stale_retention: invalid duration '{stale}' — use a unit-bearing form such as '7d' or '24h'`
   - `config.rs:2131` (sweep_interval): same pattern with `cache_repo.sweep_interval`.
4. Extend the consts classification test module: `LEGACY_STRING_ENV_OVERRIDES` is disjoint from `STRING_ENV_OVERRIDES`, `CSV_ENV_OVERRIDES`, and `EMPTY_SCALAR_ENV_OVERRIDES`; every legacy var is in `ALLOWED_ENV_OVERRIDES`. Existing assertions (STRING ⊆ EMPTY_SCALAR; STRING/CSV disjoint; `empty_preexisting_typed_override_still_fails`) must remain present and green.
5. Write the new tests below (TDD: write them first, watch each fix-required one fail, then implement steps 1-3).

**Tests:** (executable spec — all under `ENV_OVERRIDE_LOCK` via `env_lock()`, each unsets vars before asserting)
- `unitless_numeric_stale_retention_fails_validation_with_unit_error`: setup — file config with a valid standalone `url` topology (redis backend), env `CAMEL_CACHE_REPO_STALE_RETENTION=604800`; act — load + merge overrides + validate; assert — `Err` whose message contains `cache_repo.stale_retention: invalid duration '604800'` and `unit-bearing`, and does NOT contain `invalid type: integer`. Command: `cargo test -p camel-config --lib unitless_numeric_stale_retention`. Expected: fails before implementation (today's error is the deserialization `invalid type: integer`).
- `human_readable_duration_override_applies`: setup — same topology, `stale_retention` absent in file, env `CAMEL_CACHE_REPO_STALE_RETENTION=7d`; act — load + merge; assert — effective `stale_retention == Some("7d")` and validation succeeds. Expected: passes before and after (regression pin).
- `legacy_path_override_passes_through_verbatim`: setup — any valid config, env `CAMEL_CACHE_REPO_PATH=604800`; act — load + merge + deserialize; assert — effective `path == Some("604800")`. Same sub-case for `CAMEL_CACHE_REPO_PATH=007` → `Some("007")`. Expected: BOTH sub-cases fail before implementation — `parse_env_value` tries `val.parse::<i64>()` first and Rust accepts leading zeros, so `007` also becomes `Integer(7)` today, failing `Option<String>` deserialization with `invalid type: integer` (the same hazard `STRING_ENV_OVERRIDES`'s doc comment documents for `KEY_PREFIX=007`) — and both pass after.
- `numeric_backend_override_reaches_backend_validation`: setup — valid memory-backend config, env `CAMEL_CACHE_REPO_BACKEND=123`; act — load + merge + validate; assert — `Err` naming the unknown/invalid backend (existing backend validation error text containing `123`), NOT `invalid type: integer`. Expected: fails before, passes after.
- `numeric_typed_override_fields_stay_strict`: setup — valid config, env `CAMEL_CACHE_REPO_MAX_ENTRIES=notanumber`; act — load + merge + deserialize; assert — `Err` from typed deserialization on `Option<usize>`. Expected: passes before and after (pin).
- `legacy_empty_scalar_overrides_file_value_and_fails_validation`: setup — standalone `url` topology with file `stale_retention = "7d"`, env `CAMEL_CACHE_REPO_STALE_RETENTION=` (empty); act — load + merge + deserialize + validate; assert — no empty-skip applied: `Err` containing `cache_repo.stale_retention: invalid duration ''`. Expected: fails with the analogous pre-change message before (`invalid duration ''`), fails with the unit-bearing message after (behavior pin, wording upgraded).
- `credential_override_denial_tripwires_unchanged`: re-run the existing credential-denial tests (`CAMEL_CACHE_REPO_URL`/`_USERNAME`/`_PASSWORD`/`_SENTINEL_USERNAME`/`_SENTINEL_PASSWORD` ignored with the exact warning fragment `env var not in config override allowlist; ignored`). Expected: green before and after.
- `legacy_override_lists_are_disjoint_and_allowlisted`: setup — the consts classification test module; act — set-compare the four lists; assert — `LEGACY_STRING_ENV_OVERRIDES` disjoint from `STRING_ENV_OVERRIDES`, `CSV_ENV_OVERRIDES`, `EMPTY_SCALAR_ENV_OVERRIDES`, and fully contained in `ALLOWED_ENV_OVERRIDES`. Expected: fails before (const absent), passes after.
- Command (all): `cargo test -p camel-config --lib`.

**Acceptance:**
- All named tests green; whole `cargo test -p camel-config --lib` suite green. The 6 pre-existing scenarios stay covered by these existing tests in `cache_repo_env_override_tests` (verified present ~config.rs:6117-6352): `scalar_override_db_applied`, `empty_scalar_override_preserves_file_value`, `csv_override_builds_trimmed_node_list`, `csv_override_plus_master_name_validates_sentinel`, `empty_csv_override_clears_populated_file_value`, `credential_vars_stay_denied`.
- The `STRING ⊆ EMPTY_SCALAR` subset assertion and `empty_preexisting_typed_override_still_fails` remain present and green.
- `cargo check -p camel-config`, `cargo fmt --check`, `cargo clippy -p camel-config -- -D warnings`, `cargo xtask lint-unwrap` exit 0.

- [x] 1.2

#### Task 1.3: Docs sync — schema table, CONTEXT.md typed contract

**Files:**
- `docs/src/configuration/schema.md` (modified)
- `crates/camel-config/CONTEXT.md` (modified)

**Steps:**
1. `schema.md` env-override table (`docs/src/configuration/schema.md:542-560`, currently 17 rows: 12 non-cache vars + 5 legacy cache_repo vars, missing the 8 newer cache_repo vars): KEEP the 12 existing non-cache rows (`CAMEL_TIMEOUT_MS`, `CAMEL_DRAIN_TIMEOUT_MS`, `CAMEL_WATCH`, `CAMEL_WATCH_DEBOUNCE_MS`, `CAMEL_LOG_LEVEL`, `CAMEL_RUNTIME_JOURNAL_*` (3), `CAMEL_IDEMPOTENT_REPO_*` (2), `CAMEL_SUPERVISION_*` (2)) untouched; ADD the 8 missing cache_repo rows and extend the 5 legacy cache_repo rows with the new columns — variable, field, value class (numeric-typed / string-verbatim / CSV list), empty-value semantics (skip for the 7 newer scalars; legacy vars receive the raw value), and the duration rule for `_STALE_RETENTION`/`_SWEEP_INTERVAL` (humantime units required; unitless numerics rejected with the unit-bearing error; example message `cache_repo.stale_retention: invalid duration '604800' — use a unit-bearing form such as '7d' or '24h'`). Credential vars remain excluded (L-C2).
2. `crates/camel-config/CONTEXT.md`: update the env-override allowlist section — document `LEGACY_STRING_ENV_OVERRIDES` (three vars, verbatim passthrough, not empty-skippable), the duration error-message contract, and that `STRING_ENV_OVERRIDES`/`EMPTY_SCALAR_ENV_OVERRIDES` scopes are unchanged. Cite the canonical capability spec `openspec/specs/cache-repo-configuration/spec.md`.
3. Verify `docs/src/configuration/index.md` (already complete) needs no change; fix only if a link to the schema table anchors broke.

**Tests:**
- Docs consistency check (executable, run from the worktree root):
  `diff <(rg -o 'CAMEL_CACHE_REPO_[A-Z_]+' docs/src/configuration/schema.md | grep -vE '_(URL|USERNAME|PASSWORD)$' | sort -u) <(printf '%s\n' CAMEL_CACHE_REPO_BACKEND CAMEL_CACHE_REPO_PATH CAMEL_CACHE_REPO_STALE_RETENTION CAMEL_CACHE_REPO_MAX_CAPACITY CAMEL_CACHE_REPO_MAX_ENTRIES CAMEL_CACHE_REPO_PAYLOAD CAMEL_CACHE_REPO_PAYLOAD_DIR CAMEL_CACHE_REPO_CACHE_SIZE CAMEL_CACHE_REPO_SWEEP_INTERVAL CAMEL_CACHE_REPO_MASTER_NAME CAMEL_CACHE_REPO_KEY_PREFIX CAMEL_CACHE_REPO_DB CAMEL_CACHE_REPO_SENTINEL_NODES | sort)`
  (credential vars are filtered because the exclusion note legitimately names them as denied.) Expected: non-empty diff BEFORE the step (8 vars missing), empty diff (exit 0) AFTER.

**Acceptance:**
- `cargo xtask lint-context-citations` exits 0.
- Schema table retains all 12 pre-existing non-cache rows AND lists all 13 non-credential cache_repo override vars; no credential var appears as overridable.
- CONTEXT.md section states the typed contract consistently with the delta spec.

- [x] 1.3

## Phase 2: mechanical test extraction

### camel-config

#### Task 2.1: Extract inline `config.rs` test modules to `src/config_tests/` siblings (bd rc-nucw)

**Files:**
- `crates/camel-config/src/config.rs` (modified)
- `crates/camel-config/src/config_tests/byte_size_tests.rs` (new)
- `crates/camel-config/src/config_tests/camel_config_defaults_tests.rs` (new)
- `crates/camel-config/src/config_tests/components_config_tests.rs` (new)
- `crates/camel-config/src/config_tests/prometheus_config_tests.rs` (new)
- `crates/camel-config/src/config_tests/platform_config_tests.rs` (new)
- `crates/camel-config/src/config_tests/profile_loading_tests.rs` (new)
- `crates/camel-config/src/config_tests/additional_config_tests.rs` (new)
- `crates/camel-config/src/config_tests/beans_config_tests.rs` (new)
- `crates/camel-config/src/config_tests/config_validation_tests.rs` (new)
- `crates/camel-config/src/config_tests/security_config_tests.rs` (new)
- `crates/camel-config/src/config_tests/placeholder_tests.rs` (new — mod renamed from `placeholder`)
- `crates/camel-config/src/config_tests/native_credentials_tests.rs` (new — mod renamed from `native_credentials`)
- `crates/camel-config/src/config_tests/stale_native_tests.rs` (new — mod renamed from `stale_native`)
- `crates/camel-config/src/config_tests/config_builder_tests.rs` (new)
- `crates/camel-config/src/config_tests/async_io_tests.rs` (new)
- `crates/camel-config/src/config_tests/permission_provider_config_tests.rs` (new)
- `crates/camel-config/src/config_tests/languages_config_integration_tests.rs` (new)
- `crates/camel-config/src/config_tests/oversized_file_tests.rs` (new)
- `crates/camel-config/src/config_tests/config_ergonomics_tests.rs` (new)
- `crates/camel-config/src/config_tests/empty_topology_normalization_tests.rs` (new)
- `crates/camel-config/src/config_tests/cache_repo_env_override_tests.rs` (new)

**Steps:**
1. Record the baseline: `cargo test -p camel-config --lib -- --list 2>/dev/null | grep -c ": test"` → note the number in the task result.
2. The 21 inline `#[cfg(test)] mod` blocks in `config.rs` are (in file order): `byte_size_tests` (L1103), `camel_config_defaults_tests` (L3108), `components_config_tests`, `prometheus_config_tests`, `platform_config_tests`, `profile_loading_tests`, `additional_config_tests`, `beans_config_tests`, `config_validation_tests`, `security_config_tests`, `placeholder`, `native_credentials`, `stale_native`, `config_builder_tests`, `async_io_tests`, `permission_provider_config_tests`, `languages_config_integration_tests`, `oversized_file_tests`, `config_ergonomics_tests`, `empty_topology_normalization_tests`, `cache_repo_env_override_tests`. Rename the three bare-named mods during extraction — `placeholder` → `placeholder_tests`, `native_credentials` → `native_credentials_tests`, `stale_native` → `stale_native_tests` — because `is_test_file` in `scripts/xtask/src/main.rs:1461-1471` classifies src files by `test_*`/`*_test.rs`/`*_tests.rs` naming; un-renamed files would be scanned as production source by `lint-unwrap` and 4 other xtask gates. The bare `#[cfg(test)]` helper/const region (L2987-3031: `ENV_OVERRIDE_LOCK`, its setters, `env_lock()`) stays in `config.rs` — do not extract it. Renaming changes the tests' qualified names (`placeholder::x` → `placeholder_tests::x`) — acceptable; test COUNT parity is unaffected.
3. Create `crates/camel-config/src/config_tests/` and move each inline test module body to `src/config_tests/<module>.rs` verbatim (keep `use super::*` and all imports — the files remain nested modules of the config module, so private items stay reachable; NO visibility widening, no `pub`, no `pub(crate)`, no `#[doc(hidden)]`). No `mod.rs` is created.
4. In `config.rs`'s `#[cfg(test)]` region (where the first test module used to sit), reference each moved module as:
   `#[path = "config_tests/<module>.rs"] mod <module>;`
   (precedent: `crates/camel-core/src/cache/disk_offload.rs:673-675`, `crates/camel-processor/src/multicast_segment.rs:346-348`).
5. Re-run the count command; compare with the baseline.

**Tests:**
- Test-count parity (procedure, recorded in the task result): `cargo test -p camel-config --lib -- --list 2>/dev/null | grep -c ": test"` identical before and after extraction.
- Full suite: `cargo test -p camel-config --lib` — all green after extraction (this transitively re-exercises every Phase 1 test in its new location).

**Acceptance:**
- Test count identical pre/post (both numbers recorded in the result).
- Diff adds no `pub`/`pub(crate)`/`#[doc(hidden)]` to previously private items (`git diff` shows visibility keywords only in pre-existing lines).
- `config.rs` line count reduced (before/after numbers recorded).
- `cargo check -p camel-config`, `cargo fmt --check`, `cargo clippy -p camel-config -- -D warnings`, `cargo xtask lint-unwrap` exit 0.

- [x] 2.1
