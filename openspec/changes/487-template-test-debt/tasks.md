# Tasks: 487-template-test-debt

## camel-template

### Task 1.1: Split `template_set.rs` tests into a sibling file

**Files:**
- `crates/components/camel-template/src/template_set.rs` (modified)
- `crates/components/camel-template/src/template_set/tests.rs` (new)

**Steps:**
1. Cut the entire body of the `#[cfg(test)] mod tests { ... }` block
   (currently lines 314-1147) out of `template_set.rs`, dedent it one
   level, and save it verbatim as `src/template_set/tests.rs`. The first
   item in the new file stays `use super::*;`, and the test-only imports
   (`crate::config::AutoescapeMode`, `camel_language_api::MinijinjaLimitsConfig`,
   `std::collections::BTreeMap`) move with it. No test body, name, or
   assertion changes.
2. Replace the in-file block in `template_set.rs` with exactly:
   `#[cfg(test)]` on one line followed by `mod tests;` on the next. Do not
   add `#[cfg(feature = "default")]`: the parent
   `pub(crate) mod template_set;` is already `#[cfg(feature = "default")]`
   in `lib.rs`, so a child compiles only when the parent does.
3. Keep the production file ending at the last production item; no blank
   per-test helpers remain in `template_set.rs`.
4. Confirm `src/template_set/tests.rs` calls private parent items
   (`ClosureSnapshot`, `TemplateSet`, `ResolvedLimits`) through `super::*`
   without widening any visibility.

**Tests:**
- name: `template_set` module suite
- arrange: worktree at `/home/shared/rust-camel-worktrees/487-template-test-debt`
- action: `cargo test -p camel-template --lib`
- assert: `126 passed; 0 failed` (baseline measured before the edit), and
  `cargo test -p camel-template --lib -- --list 2>/dev/null | grep -c "^template_set::tests::"` prints `28`
- command: `cargo test -p camel-template --lib`
- expected: pass after the step; the same suite fails to compile if the
  submodule path or `use super::*;` is wrong

**Acceptance:**
- `rg -n "^mod tests;" crates/components/camel-template/src/template_set.rs` finds exactly one hit.
- `rg -c '#\[(tokio::)?test' crates/components/camel-template/src/template_set.rs` prints no count (exit 1, no attribute left behind).
- `rg -n 'fn (snapshot|ctx_name|ctx_empty)\(' crates/components/camel-template/src/template_set.rs` exits 1 (no hits).
- `wc -l < crates/components/camel-template/src/template_set.rs` is at most 350.
- `cargo test -p camel-template --lib -- --list` lists 28 `template_set::tests::*` names, byte-identical to the baseline list.
- `cargo fmt --check` exits 0.

- [x] 1.1

### Task 1.2: Pin sorted-key `dump` output with non-alphabetical fixture keys

**Files:**
- `crates/components/camel-template/src/template_set/tests.rs` (modified)

**Steps:**
1. In `dump_serializes_object_array_scalar_deterministically`, change the
   object fixture insert to
   `m.insert("obj", serde_json::json!({"b": 1, "a": 2}));`.
2. Change the first expected string from
   `{"name":"Ada","year":1843}|[1,2,3]|"Ada"` to
   `{"a":2,"b":1}|[1,2,3]|"Ada"`. Keep the pass-through of `items` and
   `title`, both renders, and both assertions.
3. Extend the test's `///` doc comment with one evidence sentence: the
   fixture keys are deliberately non-alphabetical, so the test pins the
   sorted-output contract for unsorted input; it does not prove that
   `dump_filter` resorts, because minijinja stores object keys sorted
   (`preserve_order` is off). Do not overclaim.

**Tests:**
- name: `dump_serializes_object_array_scalar_deterministically`
- arrange: template `{% autoescape "none" %}{{ obj | dump }}|{{ items | dump }}|{{ title | dump }}{% endautoescape %}` with context `{"b":1,"a":2}`, `[1,2,3]`, `"Ada"`
- action: render the entry twice with `ResolvedLimits::default()`
- assert: first render equals `{"a":2,"b":1}|[1,2,3]|"Ada"`; second render byte-equal to the first
- command: `cargo test -p camel-template --lib dump_serializes_object_array_scalar_deterministically`
- expected: pass; would fail if the fixture/expectation pair were left in insertion order

**Acceptance:**
- `rg -n '"b": 1, "a": 2' crates/components/camel-template/src/template_set/tests.rs` finds one hit.
- `rg -n '\{"a":2,"b":1\}\|\[1,2,3\]\|"Ada"' crates/components/camel-template/src/template_set/tests.rs` finds one hit.
- `cargo test -p camel-template --lib` still reports `126 passed; 0 failed`.

- [x] 1.2

### Task 1.3: Extract tracing-capture test infra to crate-level `test_support`

**Files:**
- `crates/components/camel-template/src/test_support.rs` (new)
- `crates/components/camel-template/src/lib.rs` (modified)
- `crates/components/camel-template/src/producer.rs` (modified)

**Steps:**
1. Create `src/test_support.rs` containing the four helpers moved verbatim
   from `producer::tests`: `ensure_global_tracing_default` (private),
   `struct FieldCollector` plus its `push` and `tracing::field::Visit`
   impl (private), `struct CaptureLayer` plus its
   `tracing_subscriber::Layer` impl (private), and `capture_warns`
   (`pub(crate)`). Add `use std::sync::Arc;` at the top: the moved bodies
   use bare `Arc`, which `producer::tests` supplied today. Keep their `///`
   doc comments, including the plain-path reference to
   `docs/testing/tracing-capture-guards.md`.
2. In `lib.rs`, declare immediately after the
   `#[cfg(feature = "default")] pub(crate) mod template_set;` line:
   `#[cfg(all(test, feature = "default"))]` then
   `pub(crate) mod test_support;`. The feature gate is required so a
   `--no-default-features` test build has no uncalled `pub(crate)` item
   (`dead_code` with `-D warnings`).
3. In `producer.rs`, delete the four helper definitions and add
   `use crate::test_support::capture_warns;` to `mod tests`. The
   `SubscriberExt`/`SubscriberInitExt` imports are function-local
   `use ... as _;` statements inside `capture_warns` and move with it;
   `std::sync::Mutex` is fully qualified at its call sites, so no
   module-level import cleanup is expected — run clippy and remove only
   what it actually flags. Keep all 17 test bodies, names, and helper
   functions (`make_templates`, `make_exchange_with_body`, `body_string`).
4. Do not touch `producer.rs` production code. The only deletions there
   are inside `#[cfg(test)] mod tests`.

**Tests:**
- name: `producer` module suite
- arrange: `test_support` compiled only under `cfg(all(test, feature = "default"))`
- action: `cargo test -p camel-template --lib`
- assert: `126 passed; 0 failed`; `cargo test -p camel-template --lib -- --list 2>/dev/null | grep -c "^producer::tests::"` prints `17`
- command: `cargo test -p camel-template --lib`
- expected: pass; the extraction fails to compile if the module declaration or feature gate is wrong
- name: no-default-features compile
- arrange: engine feature off
- action: `cargo test -p camel-template --lib --no-default-features`
- assert: exits 0 with no warnings; `cargo test -p camel-template --lib --no-default-features -- --list 2>/dev/null | grep -cE '^(template_set|producer|test_support)::'` prints `0`; total pass count is `23` (baseline measured before any edit)
- command: `cargo test -p camel-template --lib --no-default-features`
- expected: pass only with the `feature = "default"` gate on `test_support`; the 23 non-engine tests (`config`, `path_util`, `uri`, `error`) do not depend on the moved module

**Acceptance:**
- `rg -n "ensure_global_tracing_default|struct FieldCollector|struct CaptureLayer|fn capture_warns" crates/components/camel-template/src/producer.rs` returns no hits.
- `rg -nF -e 'pub(crate) fn capture_warns' -e 'fn ensure_global_tracing_default' -e 'struct FieldCollector' -e 'struct CaptureLayer' crates/components/camel-template/src/test_support.rs` finds all four.
- `rg -nF '#[cfg(all(test, feature = "default"))]' crates/components/camel-template/src/lib.rs` finds one hit.
- `cargo clippy -p camel-template --all-targets -- -D warnings` exits 0 in both feature modes.
- `wc -l < crates/components/camel-template/src/producer.rs` is at most 1000.

- [x] 1.3

## docs

### Task 1.4: Refresh the tracing-capture guard inventory

**Files:**
- `docs/testing/tracing-capture-guards.md` (modified)

**Steps:**
1. Replace the dated 22-site snapshot with an inventory of 30 guard bodies
   in 28 files, verified with
   `grep -rn "set_global_default(tracing_subscriber::registry())" --include=*.rs crates/ | wc -l`
   (30 bodies) and `... | sed 's/:[0-9]*:.*//' | sort -u | wc -l` (28
   files), dated 2026-11-01.
2. Natural homes table becomes 7 rows: keep the five current rows and add
   `camel-template/src/test_support.rs` (serves `producer.rs` tests; added
   by this change) and `camel-processor/src/test_log_capture.rs` (shared
   `ensure_global_registry`, serving `error_handler.rs` and
   `do_try_segment.rs`; replaced the removed `error_handler.rs` copy).
3. Per-file copies, no natural home in the binary (14 bodies / 14 files):
   keep the ten surviving entries, delete the stale
   `camel-processor/src/error_handler.rs` entry, add
   `camel-core/src/lifecycle/adapters/route_compiler_span_tests.rs` and
   `camel-core/src/lifecycle/application/context_lifecycle.rs` to the
   camel-core group (7 bodies there total), and add a camel-jms group with
   `src/component.rs` and `src/consumer.rs` (2 bodies).
4. Sole copy in their binary (9 bodies / 7 files): add
   `camel-cxf/tests/consumer_profile_test.rs`; record
   `camel-ws/src/lib.rs` as **3 bodies** in one file; move
   `camel-otel/src/service.rs` to its current path
   `services/camel-otel/src/service_tests.rs` (under `crates/`).
5. Add `camel-rabbitmq/tests/common/mod.rs` to the "Not this pattern"
   paragraph (recording subscriber installed with `expect`, like
   `camel-component-mcp/tests/common/mod.rs`), and exclude it from the 30.
6. Replace the old "Count note" arithmetic with the grouping sums
   **7 natural homes + 14 per-file + 9 sole = 30 bodies**, and
   **7 + 14 + 7 = 28 files** (`camel-ws/src/lib.rs` contributes 3 bodies in
   one file), plus one sentence that the snapshot is grep-verified as of
   2026-11-01.

**Tests:**
- name: inventory arithmetic check
- arrange: the rewritten table and lists
- action: extract every backticked `*.rs` path from the inventory sections
  and compare against the grep output (`30` bodies, `28` files)
- assert: the union of listed paths equals the grep path set; total body
  count stated in the file equals 30
- command: `grep -rn "set_global_default(tracing_subscriber::registry())" --include=*.rs crates/ | sed 's/:[0-9]*:.*//' | sort | uniq -c`
- expected: pass after the edit; fails before it (22 stated vs 30 actual)
- name: mdbook guide test
- arrange: `docs/` book with the modified page
- action: `cd docs && mdbook test`
- assert: exits 0
- command: `cd docs && mdbook test`
- expected: pass

**Acceptance:**
- `rg -n "camel-template/src/test_support.rs" docs/testing/tracing-capture-guards.md` finds the new natural-home row.
- `rg -n "error_handler.rs" docs/testing/tracing-capture-guards.md` finds no guard-inventory entry.
- `rg -n "camel-ws/src/lib.rs.*3 bodies|3 bodies.*camel-ws" docs/testing/tracing-capture-guards.md` finds the annotation.
- `rg -n "camel-rabbitmq/tests/common/mod.rs" docs/testing/tracing-capture-guards.md` finds the exclusion mention.
- The file states 30 bodies / 28 files and the grep command used to verify it.

- [x] 1.4

## Close-out

### Task 1.5: Verify the whole change

**Files:**
- No source files; produces measurement evidence only.

**Steps:**
1. Run the full acceptance set from the worktree with
   `CARGO_TARGET_DIR=<worktree>/target TMPDIR=/home/shared/tmp
   RUSTC_WRAPPER=` and capture exit codes.
2. Record before/after line counts for `template_set.rs`, the new
   `template_set/tests.rs`, `producer.rs`, `test_support.rs`, and the docs
   file.
3. Record before/after test-name lists (`cargo test -p camel-template
   --lib -- --list`) and confirm a zero-diff set.
4. Record the order typo: `cargo test -p camel-component-template` does not
   resolve because the package name is `camel-template`; all commands used
   the real name.

**Tests:**
- name: full gate set
- arrange: all four edits applied
- action: run fmt, clippy `-p camel-template --all-targets` (default features), the 24 locally-runnable `cargo xtask lint-*` gates (25 minus `lint-commits`, which fetches `origin/main`) plus `schema --check`, rustdoc, mdbook test, and `openspec validate --all --strict`
- assert: every command exits 0; the only expected exception is `cargo clippy -p camel-template --all-targets --no-default-features`, whose 4 `dead_code` errors in `uri.rs`/`config.rs` are pre-existing on the base commit (verified by stashing the change and re-running on HEAD; those files are untouched by this change) and are recorded in the park report instead of fixed
- command: the per-gate commands from the mission order
- expected: pass

**Acceptance:**
- `git diff --stat` touches only `crates/components/camel-template/src/`
  and `docs/testing/tracing-capture-guards.md`.
- `cargo test -p camel-template --lib -- --list` equals the baseline list.
- `openspec validate 487-template-test-debt --type change --json` is valid
  with `skip_specs: true`.
- [x] 1.5
