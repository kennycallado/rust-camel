# Tasks: gate-forwarding-assert

## xtask lint

### Task 1.1: pure rule functions and unit tests

**Files:**
- `scripts/xtask/src/lint_gate_forwarding.rs` (new)

**Steps:**
1. Create module `lint_gate_forwarding.rs`. Copy the module header
   pattern from `scripts/xtask/src/lint_component_deps.rs` (doc
   comment, imports).
2. Add pure function
   `fn gates_of(features: &toml::map::Table) -> BTreeSet<String>`
   that returns every `[features]` key except the literal key
   `default`.
3. Add pure function
   `fn closure_of(feature: &str, features: &BTreeMap<String, Vec<String>>) -> BTreeSet<String>`
   that resolves the transitive closure of a feature through the
   consumer's own feature table. String entries activate sibling
   features. Entries starting with `dep:` are skipped. Unknown sibling
   names are ignored (Cargo would error on them at build time; the
   lint does not duplicate Cargo's own validation).
4. Add pure function
   `fn is_consumer(manifest: &toml::Table) -> bool`
   that returns true when the manifest's `[dependencies]` table or its
   `[dev-dependencies]` table contains a `camel-bundles` key. Absent
   tables count as empty.
5. Add pure function
   `fn check_consumer(crate_path: &str, gates: &BTreeSet<String>, features: &BTreeMap<String, Vec<String>>, boot_consumer: bool) -> Vec<String>`
   implementing both rules. Rule 1: for each key of `features` equal
   to a gate name, if `closure_of(key, features)` contains no entry
   equal to `camel-bundles/<gate>`, push the violation line
   `<crate_path>: feature '<name>' shadows bundles gate '<name>' but does not forward camel-bundles/<name>`.
   Rule 2: when `boot_consumer` is true, for each gate with no feature
   whose closure contains `camel-bundles/<gate>`, push
   `<crate_path>: boot consumer does not forward gate '<gate>'`.
6. Write unit tests in the same file under `#[cfg(test)]` using
   synthetic in-memory tables only (no tempdirs, no real manifests).

**Tests:** (executable spec)
- `gates_default_excluded`: setup: table with keys `default`, `kafka`, `wasm` → action: `gates_of` → assert: returns `{"kafka", "wasm"}`. Command: `cargo test -p xtask lint_gate_forwarding`. Expected: pass after implementation.
- `shadow_feature_without_forwarding`: setup: gates `{"kafka"}`, features `{"kafka": ["dep:camel-component-kafka"]}` → action: `check_consumer("crates/x", gates, features, false)` → assert: returns exactly one line containing `feature 'kafka' shadows bundles gate 'kafka'`. Command: `cargo test -p xtask lint_gate_forwarding`. Expected: pass after implementation.
- `transitive_forwarding_counts`: setup: gates `{"kafka"}`, features `{"kafka": ["bundle-kafka"], "bundle-kafka": ["camel-bundles/kafka"]}` → action: `check_consumer` → assert: empty. Command: `cargo test -p xtask lint_gate_forwarding`. Expected: pass after implementation.
- `boot_consumer_missing_gate`: setup: gates `{"kafka", "mqtt"}`, features `{"kafka": ["camel-bundles/kafka"]}`, boot_consumer true → action: `check_consumer` → assert: exactly one line containing `boot consumer does not forward gate 'mqtt'`. Command: `cargo test -p xtask lint_gate_forwarding`. Expected: pass after implementation.
- `unmarked_consumer_exempt`: setup: gates `{"kafka"}`, features `{"kafka": ["camel-bundles/kafka"]}`, boot_consumer false → action: `check_consumer` → assert: empty (no completeness rule applied). Command: `cargo test -p xtask lint_gate_forwarding`. Expected: pass after implementation.
- `non_gate_feature_ignored`: setup: gates `{"kafka"}`, features `{"otel": ["dep:tracing"]}` → action: `check_consumer` → assert: empty. Command: `cargo test -p xtask lint_gate_forwarding`. Expected: pass after implementation.
- `dep_entries_skipped_in_closure`: setup: gates `{"kafka"}`, features `{"kafka": ["dep:camel-component-kafka", "kafka-extra"], "kafka-extra": ["camel-bundles/kafka"]}` → action: `check_consumer` → assert: empty (closure crosses `kafka-extra`). Command: `cargo test -p xtask lint_gate_forwarding`. Expected: pass after implementation.
- `non_consumer_ignored`: setup: manifest table with `[dependencies]` containing `serde` only → action: `is_consumer` → assert: false. Command: `cargo test -p xtask lint_gate_forwarding`. Expected: pass after implementation.
- `dev_dependency_counts_as_consumer`: setup: manifest table with empty `[dependencies]` and `[dev-dependencies]` containing `camel-bundles` → action: `is_consumer` → assert: true. Command: `cargo test -p xtask lint_gate_forwarding`. Expected: pass after implementation.

**Acceptance:**
- `cargo test -p xtask lint_gate_forwarding` passes with the nine
  tests above.
- `cargo clippy -p xtask -- -D warnings` exits 0.
- `cargo fmt --check` passes for the new file.

- [x] 1.1

### Task 1.2: workspace discovery and public wrapper

**Files:**
- `scripts/xtask/src/lint_gate_forwarding.rs` (modified)

**Steps:**
1. Add wrapper
   `pub fn lint_gate_forwarding(workspace_root: &std::path::Path) -> Result<Vec<String>, String>`
   that:
   a. Parses `crates/camel-bundles/Cargo.toml` and calls `gates_of` on
      its `[features]` table. A missing file or missing `[features]`
      table returns `Err` with the path in the message.
   b. Parses the root `Cargo.toml` `[workspace] members` list and
      expands each glob entry with `walkdir` (already an xtask
      dependency) into concrete manifest paths under `crates/` and
      `scripts/`. Members listed as explicit paths keep their path.
   c. For each member manifest for which `is_consumer` returns true, reads its
      `[features]` table (empty if absent) and its
      `[package.metadata.camel-bundles]` table (`boot-consumer = true`
      flag, false when absent), then calls `check_consumer` with the
      manifest's directory path relative to the workspace root as
      `crate_path`.
   d. Returns all violation lines concatenated.
2. Confirm the module compiles standalone before dispatch wiring;
   the wrapper is not yet called from `main.rs`.
3. Delete the module-level
   `#![cfg_attr(not(test), allow(dead_code))]` attribute added in
   task 1.1. The wrapper makes the rule functions reachable, so the
   allow is no longer needed.

**Tests:** (executable spec)
- This task ships no new unit test. The wrapper is manifest-reading
  glue over the task 1.1 pure functions, which carry the behavioral
  coverage. Its deferred behavioral proof is task 1.3
  `clean_tree_exits_zero`. Regression check: `cargo test -p xtask
  lint_gate_forwarding` → the nine task 1.1 tests still pass.

**Acceptance:**
- `cargo build -p xtask` exits 0.
- `cargo clippy -p xtask -- -D warnings` exits 0.
- `cargo test -p xtask lint_gate_forwarding` still passes (nine
  tests).

- [x] 1.2

### Task 1.3: dispatch wiring, camel-cli marker, CI, gate registry

**Files:**
- `scripts/xtask/src/main.rs` (modified)
- `crates/camel-cli/Cargo.toml` (modified)
- `.github/workflows/ci.yml` (modified)
- `AGENTS.md` (modified)

**Steps:**
1. Register the module where sibling lint modules are declared (same
   declaration style as `lint_component_deps`).
2. In `main.rs`, add enum variant `LintGateForwarding` to the command
   enum and a match arm calling `lint_gate_forwarding(&workspace_root)`
   with these exact outputs: zero violations prints
   `lint-gate-forwarding: OK (0 violations)` to stdout and exits 0.
   One or more violations prints every violation line with `println!`
   to stdout, then `lint-gate-forwarding: FAILED` to stderr, and exits
   1. An `Err` return prints `lint-gate-forwarding error: {e}` to
   stderr and exits 2.
3. In `crates/camel-cli/Cargo.toml`, add after the `[package]`
   section:
   `[package.metadata.camel-bundles]`
   `boot-consumer = true`
4. In `.github/workflows/ci.yml`, add a step
   `- name: lint-gate-forwarding`
   `run: cargo xtask lint-gate-forwarding`
   placed immediately after the `lint-metric-labels` step in the same
   job (the lint block around line 289; `lint-component-deps` has no
   CI step today).
5. In `AGENTS.md` `## QUALITY GATES`, add the entry
   `- name: lint-gate-forwarding`
   `run: cargo xtask lint-gate-forwarding`
   immediately after the `lint-component-deps` entry.
6. Run `cargo xtask lint-gate-forwarding` in the worktree and confirm
   it prints OK with 0 violations on the current tree (camel-cli
   forwards all 8 gates, camel-integration-test is unmarked with no
   gate-named features).

**Tests:** (executable spec)
- `clean_tree_exits_zero`: setup: worktree with blessed tree state and the camel-cli marker applied → action: `cargo xtask lint-gate-forwarding` → assert: exit 0, stdout contains `lint-gate-forwarding: OK (0 violations)`. Command: `cargo xtask lint-gate-forwarding`. Expected: pass after implementation.
- `marker_inert_to_cargo`: setup: camel-cli Cargo.toml with the metadata block → action: `cargo metadata --no-deps` in the worktree → assert: exit 0 (cargo accepts the manifest). Command: `cargo metadata --no-deps`. Expected: pass.
- `ci_yaml_valid`: setup: edited ci.yml → action: `actionlint .github/workflows/ci.yml` if available, else `python3 -c "import yaml;yaml.safe_load(open('.github/workflows/ci.yml'))"` → assert: exit 0. Command: as listed. Expected: pass.

**Acceptance:**
- `cargo xtask lint-gate-forwarding` exits 0 with OK output on this
  worktree.
- `cargo clippy -p xtask -- -D warnings` exits 0.
- ci.yml parses (actionlint or yaml parse exits 0).
- AGENTS.md lists the gate after `lint-component-deps`.

- [x] 1.3
