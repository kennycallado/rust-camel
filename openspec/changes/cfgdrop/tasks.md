# Tasks: cfgdrop

## Task 1 — Reject-with-error guard in the camel-config filesystem loader

- [x] 1.1

**Files**:

- `crates/camel-config/src/config.rs` (guard in
  `build_from_toml_value_inner`)
- `crates/camel-config/src/config_tests/profile_loading_tests.rs`
  (unit tests)

**Steps**:

1. In `build_from_toml_value_inner`, inside the `if has_profile_structure`
   branch and immediately before `apply_profile(&mut config_value, profile)?`,
   add the guard (design "The guard"):

   ```rust
   // gh#52 / rc-zbyyv: the strict selection below keeps ONLY the walked
   // sections — a root-level CamelConfig key would be silently discarded.
   // Fail loud, naming the key(s) and the accepted shapes.
   if let toml::Value::Table(ref table) = config_value {
       let discarded: Vec<&str> = table
           .keys()
           .map(String::as_str)
           .filter(|k| KNOWN_TOP_LEVEL_KEYS.contains(k))
           .collect();
       if !discarded.is_empty() {
           return Err(ConfigError::Message(format!(
               "top-level key(s) {} would be silently discarded by profile \
                selection: when a [default] or selected profile section is \
                present, config keys must live inside [default] (overlaid by \
                the selected profile section) — move the key(s) there, or \
                remove the profile sections to use a flat document",
               discarded.join(", ")
           )));
       }
   }
   ```

   (Word the message to match house error tone; keep the three
   assertions the tests need: the key names, `[default]`, and
   `flat document`.)

2. Leave the rc-cflo warning block, the lenient path, includes, env
   overrides, and the canonical helpers untouched.

**Tests** (in `profile_loading_tests.rs`, following the file's existing
style; use `write_temp_config` + `CamelConfig::from_file_with_profile`,
and hold `env_lock()` for the `CAMEL_PROFILE` test):

1. `root_known_table_beside_default_is_rejected`
   - **Arrange**: temp config: `[default]\nlog_level = "info"\n` plus a
     root-level `[runtime_journal]` table with `path = "j.db"`.
   - **Act**: `from_file_with_profile(path, None)`.
   - **Assert**: `Err` whose message contains `runtime_journal`,
     `[default]`, and `flat document`.
2. `root_known_scalar_beside_default_is_rejected`
   - **Arrange**: temp config: `[default]\nwatch = true\n` plus root
     `log_level = "DEBUG"`.
   - **Act**: same call.
   - **Assert**: `Err` naming `log_level`.
3. `root_known_table_with_selected_profile_and_no_default_is_rejected`
   - **Arrange**: temp config: `[prod]\nlog_level = "warn"\n` plus a
     root-level `[runtime_journal]` table. Hold `env_lock()`, set
     `CAMEL_PROFILE=prod`, unset in a drop guard.
   - **Act**: `from_file_with_profile(path, None)`.
   - **Assert**: `Err` naming `runtime_journal`.
4. `nested_journal_under_default_still_loads`
   - **Arrange**: temp config with `[default.runtime_journal]` (
     `path = "j.db"`, `durability = "eventual"`).
   - **Act**: same call with `None`.
   - **Assert**: `Ok`, `config.runtime_journal` is `Some` with
     `path == "j.db"` and the configured durability.
5. `flat_document_root_journal_still_loads`
   - **Arrange**: temp flat config (no `[default]`) with a root
     `[runtime_journal]` table (`path = "j.db"`).
   - **Act**: `from_file_with_profile(path, None)`.
   - **Assert**: `Ok`, `config.runtime_journal` is `Some` with
     `path == "j.db"` — flat behavior unchanged.
6. Confirm the existing rc-cflo ergonomics tests
     (`unset_camel_profile_with_profile_like_sections_warns_once`,
     `no_default_section_means_no_profile_structure_no_warn`,
     `active_camel_profile_suppresses_section_warn`) still pass
     unmodified — they lock the warn class this change must not touch.

**Acceptance**:

- `cargo test -p camel-config --lib` green with the five new tests.
- The guard fires only on the strict path (flat + include tests prove
  the negative).

## Task 1b — Parity battery case h: silent-discard lock becomes an error lock

- [x] 1b.1

**Files**:

- `crates/camel-config/src/config_tests/parity_golden_tests.rs`
  (convert case h)
- `crates/camel-config/src/config_tests/parity_goldens/case_08.json` (delete)
- `crates/camel-config/src/config_tests/parity_goldens/case_08_error.txt` (new,
  generated via `UPDATE_GOLDENS=1`)

**Steps**:

1. Rename `parity_section_routes_replace_toplevel` to
   `parity_root_keys_beside_default_rejected_error_locked`. Keep the
   same fixture document (root `routes`/`timeout_ms`/`watch` +
   `[default]`), switch the body from `from_file(...).expect(...)` to
   an error assertion + `lock_text_golden("case_08_error.txt",
   &display)` following the case (c) pattern
   (`parity_unknown_profile_error_string_locked`) — capture
   `ConfigError`'s full `Display`.
2. Delete the stale `case_08.json`; generate
   `case_08_error.txt` with `UPDATE_GOLDENS=1 cargo test -p
   camel-config --lib parity_root_keys`, then re-run the battery
   WITHOUT the flag to prove the lock holds. (Locate the golden dir
   from `golden_path` in the battery — the path above may differ.)
3. Update the case (h) doc comment to state the conversion (was:
   section routes replace top-level; live replacement semantic is
   locked by the profile deep-merge case; this case now locks the
   cfgdrop rejection string).

**Tests**:

1. `parity_root_keys_beside_default_rejected_error_locked`
   - **Arrange**: temp config: root `routes = ["routes/toplevel.yaml"]`,
     `timeout_ms = 5000`, `watch = true` + `[default]` with
     `routes`/`timeout_ms`.
   - **Act**: `CamelConfig::from_file(&config_path(...))`.
   - **Assert**: `Err`, and the full `Display` string matches
     `case_08_error.txt` byte-for-byte (the message names
     `routes`/`timeout_ms`/`watch` or the offending subset, `[default]`,
     and the flat-document alternative).

**Acceptance**:

- `cargo test -p camel-config --lib parity` green; no stale
  `case_08.json` remains; golden regenerated once and locked.

## Task 2 — Docs: the two document shapes, stated

- [x] 2.1

**Files**:

- `docs/src/configuration/schema.md` (one clarifying sentence in
  "Top-level fields")
- `crates/camel-config/README.md` (one clarifying line in the
  profile-structure section, near the `[default]` bullet)

**Steps**:

1. In `schema.md` "Top-level fields" intro (currently "The fields
   below live directly under `[default]`"), add: when a document uses
   profile sections, every config key lives inside `[default]` or a
   `[<profile>]` section; root-level config keys are valid only in
   flat (profile-less) documents, and mixing the two is rejected at
   load time.
2. In `crates/camel-config/README.md`, next to the `[default]` /
   `[<profile>]` bullets, state the same rule in one line.

**Tests**: none (prose; no code paths).

**Acceptance**:

- Both docs name the rejection; no other doc claims root keys merge
  into `[default]` (grep `schema.md` + README for contradictions).

## Task 3 — Gate sweep

- [x] 3.1

**Files**: none (verification only).

**Steps**:

1. From the worktree root, run and record:
   - `cargo fmt --check --all`
   - `cargo clippy --workspace --all-features --exclude camel-cli --exclude camel-component-kafka --exclude security-keycloak --exclude security-wasm-policy -- -D warnings`
   - `cargo clippy -p camel-component-kafka --all-targets -- -D warnings`
   - `cargo clippy -p camel-cli -- -D warnings`
   - `cargo clippy -p camel-cli --no-default-features --features flavor-regular,exec --all-targets -- -D warnings`
   - `cargo test -p camel-config --lib`
   - `cargo test -p camel-cli --lib`
   - `cargo test -p camel-cli --test config_compile_parity` (parity
     goldens — must stay byte-identical)
   - `cargo build --workspace`
   - `cargo test --workspace --lib`
   - `RUSTDOCFLAGS="-D warnings" cargo doc -p camel-api -p camel-core -p camel-builder -p camel-dsl -p camel-endpoint -p camel-config --no-deps`
   - `cargo xtask lint-unwrap && cargo xtask lint-log-levels && cargo xtask lint-log-redaction && cargo xtask lint-test-sleep && cargo xtask lint-unbounded-wait`
2. `openspec validate cfgdrop --type change --json` — delta structure
   clean.

**Acceptance**:

- Every command exit 0 (or a pre-existing failure verified on main and
  recorded); parity golden suite green without regeneration.
