# Tasks: lspfix

bd rc-6g6g4 · gh #55 · Mission 312. Single phase, three ordered tasks.

**TEST MANDATE (applies to EVERY `cargo test` in this change):**
```
systemd-run --user --scope --collect --unit=fleet-lspfix \
  -p MemoryMax=12G -p MemorySwapMax=2G -p TasksMax=1024 -p OOMPolicy=kill -- \
  env CARGO_BUILD_JOBS=6 RUSTC_WRAPPER=sccache cargo test -j 4 <ARGS>
```
After each invocation: `systemctl --user stop fleet-lspfix.scope 2>/dev/null || true`.
Before and after each task: `pgrep -c compiled_artifa` — must print `0`.
Build/clippy invocations set `CARGO_BUILD_JOBS=6 RUSTC_WRAPPER=sccache`. All
work happens in the feature worktree — never in the main checkout.

## camel-api / camel-dsl

### Task 1.1: Move reserved-suffix predicates to camel-api, re-export from camel-dsl

**Files:**
- `crates/camel-api/src/reserved_suffix.rs` (new)
- `crates/camel-api/src/lib.rs` (modified)
- `crates/camel-dsl/src/discovery.rs` (modified)

**Steps:**
1. Create `crates/camel-api/src/reserved_suffix.rs` with the three predicates moved VERBATIM (bodies + doc comments) from `crates/camel-dsl/src/discovery.rs` (`is_test_document`, `is_job_document`, `is_reserved_document`, each `fn(&Path) -> bool`, std-only). Keep the suffix sets exactly: test = `.test.yaml`/`.test.yml`; job = `.job.yaml`/`.job.yml`; reserved = test ∪ job.
2. Add `pub mod reserved_suffix;` to `crates/camel-api/src/lib.rs` (alphabetical position in the module list).
3. In `crates/camel-dsl/src/discovery.rs`, delete the three function definitions and add `pub use camel_api::reserved_suffix::{is_job_document, is_reserved_document, is_test_document};` so every existing `camel_dsl::discovery::*` import path keeps resolving to the same functions. Internal discovery call sites and the existing `#[cfg(test)]` predicate tests in discovery.rs stay untouched (they now exercise the re-export).
4. Add a `#[cfg(test)] mod tests` in `reserved_suffix.rs` with the tests below.

**Tests:**
- `test_suffixes_match`: `Path::new` for `a.test.yaml`, `a.test.yml` → `is_test_document` true; `a.job.yaml`, `a.job.yml` → `is_job_document` true; all four → `is_reserved_document` true; command: mandate wrapper with `-p camel-api --lib reserved_suffix`; expected: fails before step 1, passes after.
- `non_reserved_names_rejected`: `atest.yaml`, `ajob.yaml`, `a.yaml`, `x.test.json`, `x.job.json` → all three predicates false; same command.
- `reexport_predicate_still_true` (in `crates/camel-dsl/src/discovery.rs` tests): assert `super::is_reserved_document(Path::new("a.test.yaml"))` is true — the in-crate unit test exercises the re-exported symbol via `super::`; the external `camel_dsl::discovery::*` path is exercised by the camel-cli tests in this task's acceptance; command: mandate wrapper with `-p camel-dsl --lib discovery::tests::reexport_predicate_still_true`; expected: passes (behavioral no-op move).

**Acceptance:**
- Mandate wrapper `cargo test -j 4 -p camel-api --lib` exits 0.
- Mandate wrapper `cargo test -j 4 -p camel-dsl --lib discovery` exits 0 (all existing reserved-suffix discovery tests green through the re-export).
- Mandate wrapper `cargo test -j 4 -p camel-cli --test lint_test_doc_skip` exits 0 (CLI skip behavior unchanged).
- `cargo clippy -p camel-api -p camel-dsl -- -D warnings` (with `CARGO_BUILD_JOBS=6 RUSTC_WRAPPER=sccache`) exits 0.
- `cargo fmt --check --all` exits 0.

- [x] 1.1

## camel-lint

### Task 1.2: Add `LintEngine::lint_with_path` with the R-RESERVED reserved-suffix skip

**Files:**
- `crates/camel-lint/src/diagnostic.rs` (modified)
- `crates/camel-lint/src/engine.rs` (modified)

**Steps:**
1. In `crates/camel-lint/src/diagnostic.rs`, add variant `RReserved` to `DiagnosticCode` and extend the `Display` impl with `DiagnosticCode::RReserved => f.write_str("R-RESERVED")` (additive to the stable string contract). Also add `PartialEq, Eq` to the derive lists of both `Diagnostic` (line ~103) and `Fix` (line ~117) so tests can compare `Vec<Diagnostic>` with `assert_eq!` (all fields are already `Eq`-able: `Span`, `Severity`, `DiagnosticCode`, `String`, `Option<Fix>`).
2. In `crates/camel-lint/src/engine.rs`, add `use std::path::Path;` and `use camel_api::reserved_suffix::is_reserved_document;` (camel-lint already depends on camel-api — no Cargo.toml change).
3. Add `pub fn lint_with_path(&self, source: &str, path: Option<&Path>) -> Vec<Diagnostic>`: when `path` is `Some(p)` and `is_reserved_document(p)`, return a single `Diagnostic` with code `DiagnosticCode::RReserved`, `Severity::Info`, `Span::new(0, 0)`, `fix: None`, and message `format!("skipped: {} is a reserved document (camel test or camel job)", p.display())` — no rules run. Otherwise parse and run the rule loop exactly as `lint` does today.
4. Rewrite `pub fn lint(&self, source: &str) -> Vec<Diagnostic>` as `self.lint_with_path(source, None)` (doc comment updated to say it delegates without file context).
5. Add engine unit tests (in the existing `#[cfg(test)]` module of engine.rs, using the existing `StubCatalog` test support and `LintEngine::with_default_rules`).

**Tests:**
- `lint_with_path_reserved_test_suffix_skips_all_rules`: engine with default rules; source = a test document body `routeFiles: [hello.yaml]\ninputs: {}\nexpects: []\n`; act = `lint_with_path(source, Some(Path::new("routes/hello.test.yaml")))`; assert = result.len() == 1, code == `DiagnosticCode::RReserved`, severity == `Severity::Info`, span == `Span::new(0, 0)`, message contains `reserved document`; command: mandate wrapper `-p camel-lint --lib engine`; expected: fails before step 3 (R-SCHEMA errors for `routeFiles`), passes after.
- `lint_with_path_every_reserved_suffix_variant_skips`: same source; paths `x.test.yaml`, `x.test.yml`, `x.job.yaml`, `x.job.yml` each → exactly one Info `Reserved` diagnostic; same command.
- `lint_with_path_ordinary_path_lints_as_before`: source = `from: timer:tick\n` with `StubCatalog` empty (no `timer` metadata) or a schema-violating source; path `routes/hello.yaml`; assert = `lint_with_path(source, Some(path))` == `lint(source)` (identical Vec). Same command.
- `lint_without_path_is_unchanged`: same test-document source, `lint(source)` and `lint_with_path(source, None)` both produce the non-empty rule diagnostics (identical to each other) — no skip without file context. Same command.
- `lint_with_path_non_reserved_lookalike_not_skipped`: path `routes/atest.yaml` and `routes/x.test.json` → diagnostics equal `lint(source)`. Same command.

**Acceptance:**
- Mandate wrapper `cargo test -j 4 -p camel-lint --lib` exits 0.
- `grep -n 'camel-dsl\|camel-core' crates/camel-lint/Cargo.toml` finds nothing (hex boundary intact).
- Mandate wrapper `cargo test -j 4 -p camel-core --test hexagonal_architecture_boundaries_test camel_lint` exits 0.
- `cargo clippy -p camel-lint -- -D warnings` (CARGO_BUILD_JOBS=6, RUSTC_WRAPPER=sccache) exits 0; `cargo fmt --check --all` exits 0.

- [x] 1.2

## camel-lsp

### Task 1.3: Thread the document URI path through all three LSP lint sites

**Files:**
- `crates/camel-lsp/src/lib.rs` (modified)
- `crates/camel-lsp/src/debounce.rs` (modified)
- `crates/camel-lsp/tests/lsp_session.rs` (modified)

**Steps:**
1. In `did_open` (lib.rs), compute `let path = uri.to_file_path().ok();` before linting and call `self.engine.lint_with_path(&doc.raw, path.as_deref())`. Same in `did_save` for the cloned `raw` (the `uri` variable is already in scope; convert before publishing).
2. In `debounce.rs` `schedule`'s spawned task, after cloning `raw`, compute `let path = task_uri.to_file_path().ok();` and call `engine.lint_with_path(&raw, path.as_deref())` in place of `engine.lint(&raw)`. The version-check/publish logic is untouched.
3. camel-lsp gains NO new dependency (camel-api comes transitively via camel-lint; the change uses only `Url::to_file_path` from tower-lsp's lsp_types, already imported via `Url`).
4. Add the session tests below to `crates/camel-lsp/tests/lsp_session.rs`, following the existing spawn-server-over-stdio harness and the `ERROR`/`INFO` severity-constant pattern used by `placeholder_string_field_publishes_info_not_error`.

**Tests:**
- `reserved_test_yaml_did_open_publishes_single_info_no_errors`: setup = running server; act = `didOpen` with URI `file:///tmp/lspfix%20dir/routes/hello.test.yaml` (percent-encoded directory — verifies `to_file_path` decoding) and text `routeFiles: [hello.yaml]\ninputs: {}\nexpects: []\n`; assert = published diagnostics have length 1, severity Info (not Error), source `camel-lint`, code `R-RESERVED`, and zero entries with severity Error; command: mandate wrapper `-p camel-lsp --test lsp_session reserved_test_yaml_did_open`; expected: fails before step 1 (three Error diagnostics: missing id/from, unexpected routeFiles/inputs/expects), passes after.
- `reserved_job_yaml_did_change_debounce_stays_error_free`: setup = didOpen of `file:///tmp/routes/nightly.job.yaml` with placeholder job body; act = `didChange` full replacement (version 2) with `schedule: "0 2 * * *"\nsteps: []\n` then drain notifications with the inline `tokio::time::timeout` loop pattern used by `placeholder_string_field_publishes_info_not_error` until the publish tagged with version 2 arrives (identify the final publish by its document version, not merely by absence of errors); assert = that version-2 publish contains zero Error-severity diagnostics; command: mandate wrapper `-p camel-lsp --test lsp_session reserved_job_yaml_did_change`.
- `reserved_test_yaml_did_save_stays_error_free`: setup = didOpen as in the first test; act = `didSave`; assert = republished diagnostics contain zero Error entries; command: mandate wrapper `-p camel-lsp --test lsp_session reserved_test_yaml_did_save`.
- `malformed_reserved_yaml_skips_rsyn`: act = `didOpen` with URI `file:///tmp/routes/broken.test.yaml` and text `not: [a, route`; assert = exactly one Info diagnostic (R-RESERVED mapping), no Error diagnostics, server still responds to a subsequent request (no panic/hang); command: mandate wrapper `-p camel-lsp --test lsp_session malformed_reserved_yaml`.
- `untitled_uri_keeps_full_lint`: act = `didOpen` with URI `untitled:Untitled-1` and a schema-violating route (e.g. `foo: bar\n`); assert = at least one Error-severity diagnostic is published (full lint, no path context); command: mandate wrapper `-p camel-lsp --test lsp_session untitled_uri_keeps_full_lint`.

**Acceptance:**
- Mandate wrapper `cargo test -j 4 -p camel-lsp --test lsp_session` exits 0 (all new + existing session tests).
- Mandate wrapper `cargo test -j 4 -p camel-lsp --lib` exits 0.
- `grep -n 'camel-dsl' crates/camel-lsp/Cargo.toml` finds nothing (route-lsp dependency contract intact).
- Mandate wrapper `cargo test -j 4 -p camel-core --test hexagonal_architecture_boundaries_test` exits 0.
- `cargo clippy -p camel-lsp --all-targets -- -D warnings` (CARGO_BUILD_JOBS=6, RUSTC_WRAPPER=sccache) exits 0; `cargo fmt --check --all` exits 0.

- [x] 1.3
