# Tasks: syn3

## Baselines (captured on syn 2, BEFORE the pin flip)

### Task 1.1: bean-macros expansion baselines

**Files:**
- `crates/camel-bean-macros/src/expansion_baselines.rs` (new)
- `crates/camel-bean-macros/src/lib.rs` (modified — add `#[cfg(test)] mod expansion_baselines;`)
- `crates/camel-bean-macros/src/baselines/bean_impl_ok.txt` (new, generated)
- `crates/camel-bean-macros/src/baselines/bean_impl_generic_reject.txt` (new, generated)
- `crates/camel-bean-macros/src/baselines/bean_impl_no_handler_reject.txt` (new, generated)
- `crates/camel-bean-macros/src/baselines/handler_self_reject.txt` (new, generated)
- `crates/camel-bean-macros/src/baselines/handler_nonasync_reject.txt` (new, generated)
- `crates/camel-bean-macros/src/baselines/handler_dup_param_reject.txt` (new, generated)

**Steps:**
1. Create `expansion_baselines.rs` with a helper `fn check(name: &str, produced: String)` that resolves `env!("CARGO_MANIFEST_DIR")/src/baselines/{name}` and either writes the file (when `std::env::var("UPDATE_GOLDENS").is_ok_and(|v| v == "1")`, matching the repo's canonical regeneration switch) or `assert_eq!`s byte-for-byte against the file's contents (read with `std::fs::read_to_string` so regeneration writes land on disk).
2. Each baseline test parses its input via `syn::parse_quote!` (mirror the ItemImpl/ImplItemFn literals already used in the `mod tests` at `crates/camel-bean-macros/src/lib.rs:250+` and `crates/camel-bean-macros/src/handler.rs:157+`), calls the crate-internal generator (`bean_impl_gen` for impl-level cases — private-in-root is visible to the child test module, no visibility change needed; `parse_handler_method` at `crates/camel-bean-macros/src/handler.rs:71` for handler-level rejects — make it `pub(crate)` if the sibling-module boundary requires, a visibility-only change), and passes `Ok(ts) => check(name, ts.to_string())` or `Err(e) => check(name, e.to_string())`.
3. Run `UPDATE_GOLDENS=1 cargo test -p camel-bean-macros --lib` to generate the six baseline files on the CURRENT syn 2 pin; commit them.
4. Run `cargo test -p camel-bean-macros --lib` without the env var — all tests green (byte-stable on re-run).
5. Do NOT touch `Cargo.toml` or the workspace pin in this task.

**Tests:** (all on syn 2, pre-flip)
- `bean_impl_expansion_baseline_ok`: ItemImpl with two `#[handler]` methods (one `async fn` `&self` with a `String` param and a `Result<String, String>` return, one unit-return `&self`) fed to `bean_impl_gen` → Ok → `baselines/bean_impl_ok.txt` byte-equal on re-run. Command: `cargo test -p camel-bean-macros --lib bean_impl_expansion_baseline_ok`. Expected: pass after generation.
- `bean_impl_baseline_generic_reject`: ItemImpl with generics (`impl<T> Foo<T>`) → Err → `bean_impl_generic_reject.txt` (message must contain "bean_impl does not support generic types").
- `bean_impl_baseline_no_handler_reject`: ItemImpl with zero `#[handler]` methods → Err → `bean_impl_no_handler_reject.txt` (message "No #[handler] methods found in impl block").
- `handler_baseline_self_reject`: `#[handler] fn handle(self)` (by-value receiver) → Err → `handler_self_reject.txt` (message "Handler methods must use &self, not self").
- `handler_baseline_nonasync_reject`: `#[handler] fn handle(&self)` (sync) → Err → `handler_nonasync_reject.txt`.
- `handler_baseline_dup_param_reject`: `#[handler] async fn h(&self, a: String, a: String)`-shaped input → Err → `handler_dup_param_reject.txt` (message contains "Duplicate parameter name").

**Acceptance:**
- `cargo test -p camel-bean-macros --lib` exits 0 on syn 2 with baselines in place.
- Six baseline files committed; `git diff` clean after step 4 re-run.
- Workspace `Cargo.toml` still `syn = "2.0"` in this task's commit.
- `cargo clippy -p camel-bean-macros --all-targets -- -D warnings` exits 0.

- [x] 1.1

### Task 1.2: endpoint-macros expansion baselines

**Files:**
- `crates/camel-endpoint-macros/src/expansion_baselines.rs` (new)
- `crates/camel-endpoint-macros/src/lib.rs` (modified — add `#[cfg(test)] mod expansion_baselines;`)
- `crates/camel-endpoint-macros/src/baselines/uri_config_ok.txt` (new, generated)
- `crates/camel-endpoint-macros/src/baselines/uri_config_missing_scheme_reject.txt` (new, generated)
- `crates/camel-endpoint-macros/src/baselines/uri_config_bad_param_reject.txt` (new, generated)

**Steps:**
1. Same `check(name, produced)` helper pattern as Task 1.1 (CARGO_MANIFEST_DIR + `UPDATE_GOLDENS=1` regeneration switch + byte `assert_eq!`).
2. Build `DeriveInput`s via `syn::parse_quote!` mirroring the attribute shapes exercised by the 20 existing tests in `crates/camel-endpoint-macros/src/uri_config.rs` (uri_scheme / uri_param / uri_config attributes as parsed by `extract_scheme`, `parse_uri_param_attr`, `parse_uri_config_attr`).
3. Call `crate::uri_config::impl_uri_config(&input)`; baseline `Ok(ts).to_string()` or `Err(e).to_string()`.
4. `UPDATE_GOLDENS=1 cargo test -p camel-endpoint-macros --lib` generates on syn 2; commit; plain re-run green.

**Tests:** (all on syn 2, pre-flip)
- `uri_config_expansion_baseline_ok`: struct with a uri_scheme attribute plus fields `name: String`, `optional: Option<String>`, `multi: Vec<String>`, `flag: bool` (one field carrying a `#[uri_param]` attribute per existing test shapes) → Ok → `baselines/uri_config_ok.txt` byte-equal on re-run. Command: `cargo test -p camel-endpoint-macros --lib uri_config_expansion_baseline_ok`.
- `uri_config_baseline_missing_scheme_reject`: same struct without any uri_scheme attribute → Err → `uri_config_missing_scheme_reject.txt`.
- `uri_config_baseline_bad_param_reject`: struct with a `#[uri_param(unknown = "x")]` payload (a key `parse_uri_param_attr` does not recognize) → Err → `uri_config_bad_param_reject.txt`.

**Acceptance:**
- `cargo test -p camel-endpoint-macros --lib` exits 0 on syn 2 (20 existing + 3 new tests).
- Three baseline files committed.
- Workspace `Cargo.toml` still `syn = "2.0"` in this task's commit.
- `cargo clippy -p camel-endpoint-macros --all-targets -- -D warnings` exits 0.

- [x] 1.2

### Task 1.3: xtask lint verdict captures + lock pre-copy

**Files:**
- `openspec/changes/syn3/baselines/Cargo.lock.pre` (new, copy of current lock)
- `openspec/changes/syn3/baselines/xtask/lint-test-sleep.pre.txt` (new, captured)
- `openspec/changes/syn3/baselines/xtask/lint-metric-labels.pre.txt` (new, captured)
- `openspec/changes/syn3/baselines/xtask/lint-unbounded-wait.pre.txt` (new, captured)
- `openspec/changes/syn3/baselines/xtask/lint-context-citations.pre.txt` (new, captured)
- `openspec/changes/syn3/baselines/xtask/manifest.txt` (new — sha256 + line count per capture + exit codes)

**Steps:**
1. Copy `Cargo.lock` to `openspec/changes/syn3/baselines/Cargo.lock.pre`.
2. Find the exact CLI subcommand names by reading the match arms in `scripts/xtask/src/main.rs` for the four lints (verified arms: `lint-test-sleep`, `lint-metric-labels`, `lint-unbounded-wait`, `lint-context-citations`).
3. Build first, then capture from the built binary directly so no cargo compile/`Finished … in N.NNs` noise enters the capture: `cargo build -p xtask` then for each lint `./target/debug/xtask <lint> > openspec/changes/syn3/baselines/xtask/<lint>.pre.txt 2>&1; echo "EXIT=$?" >> openspec/changes/syn3/baselines/xtask/<lint>.pre.txt`.
4. Write `manifest.txt`: for each capture — sha256, line count, EXIT code.
5. Pre-flip test-count baseline, recorded as two labeled normalized lines (per-target runs avoid cargo's indented `Running` lines whose binary hashes change across the pin flip):
   - `LIB: ` + `cargo test -p xtask --lib 2>&1 | grep 'test result' | sed -E 's/;? *finished in [0-9.]+s//'`
   - `E2E_COMPILE: ` + `cargo test -p xtask --test archive_e2e 2>&1 | grep 'test result' | sed -E 's/;? *finished in [0-9.]+s//'` (its tests are `#[ignore]`d so this run compiles the harness and reports 0 run — the value is the deterministic result line; the real execution happens in Task 2.4 via `-- --ignored`)
   - Plus a full `cargo test -p xtask` exit-code record (raw output kept out of the contract).
   All three lines go into `manifest.txt`; they are the no-tests-lost contract for Task 2.4.
6. Commit. No source changes in this task; the pin must still be `syn = "2.0"`.

**Tests:**
- Capture per lint: `./target/debug/xtask lint-test-sleep > ….pre.txt 2>&1; echo "EXIT=$?" >> ….pre.txt` (repeat for the other three) → assert: each file non-empty, EXIT code recorded; runs against the unchanged syn-2 tree.
- Byte-stability self-check: run `lint-test-sleep` twice back-to-back from the built binary; the two captures must be byte-identical (proves the capture method itself is deterministic before it becomes the comparison contract).
- Expected: exit codes are whatever main currently yields (record, do not judge); they become the comparison contract for Task 2.4.

**Acceptance:**
- Four `.pre.txt` captures + manifest + `Cargo.lock.pre` committed.
- `git status` clean; no source files modified.

- [x] 1.3

## Migration

### Task 2.1: pin flip + compile-error inventory

**Files:**
- `Cargo.toml` (modified — workspace dep `syn = "2.0"` -> `syn = "3"`)
- `openspec/changes/syn3/baselines/compile-inventory.txt` (new)

**Steps:**
1. Change ONLY the workspace `syn` entry in root `Cargo.toml` (currently line 265).
2. Run `cargo check -p camel-bean-macros -p camel-endpoint-macros -p xtask 2>&1 | tee openspec/changes/syn3/baselines/compile-inventory.txt`; exit code nonzero is EXPECTED — do not fix any code in this task.
3. Run `git diff Cargo.lock` and verify every `+`/`-` hunk is the syn edge move (our three crates rewiring onto syn 3.0.4 which is already in the lock); record the hunk list at the top of `compile-inventory.txt`.
4. Commit pin + lock + inventory together.

**Tests:**
- `grep -n 'syn = ' Cargo.toml` → exactly one workspace entry, `syn = "3"`.
- `cargo check -p camel-bean-macros -p camel-endpoint-macros -p xtask` → nonzero exit with errors enumerated in the inventory (this IS the expected pre-migration state).
- `git diff Cargo.lock` between this task's commit and its parent contains no package changes beyond syn-edge rewiring (transitive syn 2.0.119 entries may remain — they belong to third-party proc-macros).

**Acceptance:**
- Inventory committed, lists every compile error of all three crates on syn 3.
- Lock diff hunks all justified in the inventory header.

- [x] 2.1

### Task 2.2: camel-bean-macros on syn 3

**Files:**
- `crates/camel-bean-macros/src/handler.rs` (modified — expected: `FnArg::Receiver` arm per syn 3 Receiver shape)
- `crates/camel-bean-macros/src/lib.rs` (modified only if compile inventory shows errors there)

**Steps:**
1. Read `openspec/changes/syn3/baselines/compile-inventory.txt`; fix every bean-macros error it lists (compile again to catch residuals). Prime suspect: `handler.rs` `receiver.reference.is_some()` — consult docs.rs syn 3 `Receiver`/`ReceiverKind` and adapt so the `&self`-only policy and the exact error string "Handler methods must use &self, not self" are unchanged.
2. Run the full lib battery INCLUDING the Task 1.1 baselines — they must pass byte-identical WITHOUT UPDATE_GOLDENS (no baseline edits allowed in this task; a baseline diff is a parity failure, stop and report).
3. `cargo clippy -p camel-bean-macros --all-targets -- -D warnings` and `cargo fmt -p camel-bean-macros`.

**Tests:**
- `cargo test -p camel-bean-macros --lib` → exit 0, all 24 tests (18 pre-existing + 6 baseline) pass, baselines byte-identical (no UPDATE_GOLDENS run).
- `handler_baseline_self_reject` and `handler_baseline_nonasync_reject` specifically green — they pin the Receiver-adapted diagnostics.

**Acceptance:**
- crate compiles + battery green + clippy/fmt clean on syn 3.
- Zero changes under `crates/camel-bean-macros/src/baselines/`.
- git diff of this task touches only bean-macros source files.

- [x] 2.2

### Task 2.3: camel-endpoint-macros on syn 3

**Files:**
- `crates/camel-endpoint-macros/src/lib.rs` (modified only if inventory lists errors)
- `crates/camel-endpoint-macros/src/uri_config.rs` (modified only if inventory lists errors)

**Steps:**
1. Fix every endpoint-macros error from the inventory (expected: none to few — surface is stable API; do not pre-emptively rewrite).
2. Run the full lib battery including Task 1.2 baselines — byte-identical, no UPDATE_GOLDENS.
3. Check for a trybuild suite: `ls crates/camel-endpoint-macros/tests/` — if trybuild cases exist, run them; if the dev-dependency is declared but no suite directory exists, record "trybuild declared, no suite present" in the task report and skip.
4. `cargo clippy -p camel-endpoint-macros --all-targets -- -D warnings` and `cargo fmt -p camel-endpoint-macros`.

**Tests:**
- `cargo test -p camel-endpoint-macros --lib` → exit 0, all 23 tests (20 pre-existing + 3 baseline) pass byte-identical.

**Acceptance:**
- Battery green + clippy/fmt clean on syn 3.
- Zero changes under `crates/camel-endpoint-macros/src/baselines/`.

- [x] 2.3

### Task 2.4: scripts/xtask on syn 3 + verdict re-capture proof

**Files:**
- `scripts/xtask/src/lint_unbounded_wait.rs` (modified only if inventory lists errors)
- `scripts/xtask/src/lint_test_sleep.rs` (modified only if inventory lists errors)
- `scripts/xtask/src/lint_metric_labels.rs` (modified only if inventory lists errors)
- `scripts/xtask/src/main.rs` (modified only if inventory lists errors)
- `openspec/changes/syn3/baselines/xtask/post-vs-pre.md` (new — comparison evidence)

**Steps:**
1. Fix every xtask error from the inventory — the task-2.1 inventory could not reach xtask (starved behind the bean-macros failure via xtask→camel-dsl→camel-core→camel-bean→camel-bean-macros), so run `cargo check -p xtask` yourself after task 2.2 has unlocked the chain and treat its full error output as the inventory supplement (record it as a header addendum in `post-vs-pre.md`).
2. `cargo test -p xtask` — exit 0; plus re-run the two labeled per-target commands exactly as recorded in Task 1.3 (`--lib` and `--test archive_e2e`, same grep + same sed normalization) — both labeled lines byte-identical to the `manifest.txt` record (no test binary lost, no count changed; timing and binary hashes excluded from the contract by construction).
3. `command -v openspec` must be non-empty (the e2e harness early-returns silently when the binary is absent — `archive_e2e.rs:103-117`); record `openspec --version` in `post-vs-pre.md`, then `cargo test -p xtask --test archive_e2e -- --ignored` — the 4 e2e tests are `#[ignore]`d; run them explicitly (per the invocation documented at `scripts/xtask/tests/archive_e2e.rs:8`) — all 4 green.
4. Rebuild and re-run the four captured lints from the built binary exactly as in Task 1.3 (`cargo build -p xtask` then `./target/debug/xtask <lint>` into `.post.txt` siblings; append `EXIT=$?` identically).
5. Byte-diff each `.post.txt` against its `.pre.txt`. The diff MUST be empty — any delta is a parity FAILURE: stop, report `parity-failure: <lint> <delta>`, and do not proceed (the conductor decides next steps; workers never classify lint-output deltas as benign). Write the verdict table (lint, pre-sha256, post-sha256, delta bytes, verdict) to `post-vs-pre.md` with all four verdicts EMPTY-DIFF.
6. Ratchet stability: `git diff --exit-code $(git log --format=%H -1 -- openspec/changes/syn3/baselines/xtask/manifest.txt) -- scripts/xtask/ratchet-test-sleep.max scripts/xtask/ratchet-unbounded-wait.max scripts/xtask/ratchet-cancel-tokens.max` — exit 0 required (this resolves the Task 1.3 baseline commit and diffs all three ratchet files against it).
7. `cargo clippy -p xtask --all-targets -- -D warnings` and `cargo fmt -p xtask`.

**Tests:**
- `cargo test -p xtask` → exit 0, summary count equals the Task 1.3 recorded count.
- `cargo test -p xtask --test archive_e2e -- --ignored` → exit 0, 4 pass.
- For each of the four lints: `diff openspec/changes/syn3/baselines/xtask/<lint>.pre.txt openspec/changes/syn3/baselines/xtask/<lint>.post.txt` → empty; any non-empty diff is a parity failure (stop and report).

**Acceptance:**
- Battery green (per-target counts byte-identical to pre-flip); e2e 4/4 with `openspec` on PATH; four lint verdict diffs EMPTY; ratchets untouched; clippy/fmt clean.

- [x] 2.4

## Verification

### Task 3.1: downstream battery + lock audit

**Files:**
- `openspec/changes/syn3/baselines/lock-audit.md` (new)

**Steps:**
1. `cargo test -p camel-dsl --lib` (854 tests) — green; confirm `git status` shows zero modifications under `crates/camel-dsl/tests/goldens/` (corpus byte-stable).
2. `cargo test --workspace --lib` — whole-workspace lib battery green (covers camel-bean / camel-endpoint re-export compile surface and all macro consumers).
3. CI-equivalent clippy legs (the exact gates the Dependabot PR failed), all four must exit 0:
   - `cargo clippy --workspace --all-features --exclude camel-cli --exclude camel-component-kafka --exclude security-keycloak --exclude security-wasm-policy -- -D warnings`
   - `cargo clippy -p camel-component-kafka --all-targets -- -D warnings`
   - `cargo clippy -p camel-cli -- -D warnings`
   - `cargo clippy -p camel-cli --no-default-features --features flavor-regular,exec --all-targets -- -D warnings`
4. Lock audit: `git diff --no-index openspec/changes/syn3/baselines/Cargo.lock.pre Cargo.lock` — walk EVERY hunk; write `lock-audit.md` listing each changed package/edge with its justification (expected: only syn-edge rewiring for camel-bean-macros, camel-endpoint-macros, xtask). Any unrelated hunk = failure — report, do not force.
5. `cargo audit` if the advisory DB resolves offline; otherwise record "advisory scan deferred to CI" in lock-audit.md.
6. `cargo fmt --all -- --check`.

**Tests:**
- `cargo test -p camel-dsl --lib` → exit 0, 854 pass.
- `cargo test --workspace --lib` → exit 0.
- All four clippy legs → exit 0.
- `git status --porcelain crates/camel-dsl/tests/goldens` → empty output.

**Acceptance:**
- DSL battery + workspace lib battery + four clippy legs green; goldens untouched; lock-audit.md justifies every lock hunk; fmt clean.

- [x] 3.1
