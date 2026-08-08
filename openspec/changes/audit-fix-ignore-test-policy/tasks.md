# Tasks: audit-fix-ignore-test-policy

## scripts/xtask

### Task 1: Implement `lint-ignore` xtask command

**Files:**
- `scripts/xtask/src/main.rs` (modified)

**Steps:**
1. Add `LintIgnore` variant to the `Commands` enum with doc comment:
   `/// Enforce ADR-0054: every #[ignore] must carry a reason string from a closed vocabulary. Exits non-zero on violations.`
2. Add a match arm in `main()` for `Commands::LintIgnore` following the exact pattern of `Commands::LintNonExhaustive` (call `lint_ignore(&workspace_root)`, print violations or OK, exit non-zero on failure).
3. Implement `fn lint_ignore(workspace_root: &Path) -> Result<Vec<Violation>, String>`:
   - Walk all `.rs` files using `WalkDir::new(workspace_root)`. For each file, SKIP if any path component is `target`, `.worktrees`, `scripts`, or `bridges`. Also skip if the file is NOT under a `crates/` or `examples/` subdirectory. IMPORTANT: do NOT call `is_test_file()` — test files under `tests/` MUST be scanned (unlike `lint_log_levels` which skips them).
   - For each file, scan line by line. FIRST: skip the line if its trimmed form starts with `//` or `//!` (comment lines — prevents false positives on lines like `// REDIS-009: ... (#[ignore] by default)` or `//! All tests ... #[ignore] ...`). THEN: use regex `r"#\[ignore\s*(?:=\s*\"([^\"]*)\")?\]"` to find `#[ignore]` attributes and capture the reason string if present.
   - If the regex matched but no reason group was captured (bare `#[ignore]`): emit a Violation whose `snippet` field starts with `ignore:missing-reason:` followed by a human-readable message. Example snippet: `"ignore:missing-reason: bare #[ignore] — add a reason string from the closed vocabulary (see ADR-0054)"`.
   - If a reason was captured, validate it against the closed vocabulary:
     - `requires live ` (prefix + literal space, then non-empty detail)
     - `requires pre-built ` (prefix + literal space, then non-empty detail)
     - `slow test: ` (prefix + literal colon + literal space, then non-empty detail)
   - If the reason does not start with any valid prefix+delimiter: emit Violation with snippet starting `ignore:invalid-prefix:`.
   - If the reason starts with a valid prefix+delimiter but the detail after the delimiter is empty/whitespace-only: emit Violation with snippet starting `ignore:empty-detail:`.
   - For violations that have no source line (allowlist reverse-check violations): use `line: 0` as sentinel.
4. Call `load_ignore_allowlist(workspace_root)` at the top of `lint_ignore` and use the returned set for the forward, reverse, and mixed-reason checks below.
5. Implement `fn load_ignore_allowlist(workspace_root: &Path) -> std::collections::HashSet<String>`:
   - Read `scripts/xtask/allowlist-ignore.txt`. Same format as `allowlist-log-levels.txt` — one relative path per line, `#` for comments, blank lines ignored.
   - If the file does not exist, return an empty set.
6. Implement allowlist coupling checks within `lint_ignore` (after the per-file scan):
   - **Forward check:** for each file containing a `requires pre-built` test, verify the file's relative path is in the allowlist. If not: Violation with snippet starting `ignore:pre-built-not-in-allowlist:`.
   - **Reverse check:** for each allowlist entry: (a) verify the path is a direct-child `.rs` file under `crates/components/camel-component-wasm/tests/` (not nested deeper). If not: Violation `ignore:allowlist-out-of-scope:`. (b) Verify the file exists on disk. If not: Violation `ignore:allowlist-stale:`. (c) Verify the file contains at least one `requires pre-built` test. If not: Violation `ignore:allowlist-no-pre-built-test:`.
   - **Mixed-reason check:** for each allowlisted file, verify ALL `#[ignore]` reasons in that file start with `requires pre-built`. If any uses a different prefix: Violation `ignore:allowlist-mixed-reasons:`.
7. Add unit tests in the `#[cfg(test)]` module. Use the same `tmp_workspace` helper pattern as existing lint tests (see `lint_non_exhaustive` tests for the pattern: create temp dir, write source files, call the lint function, assert on violations, clean up). IMPORTANT: since the Violation struct has no `rule` field, tests must assert on `violation.snippet.contains("ignore:<code>")` rather than a separate rule field.

**Tests:** (executable spec — name, setup, action, assert)
- `bare_ignore_is_violation`: temp workspace with `#[ignore]\n#[test] fn foo() {}` in `crates/foo/src/lib.rs` → `lint_ignore(&ws)` returns 1 violation whose `.snippet.contains("ignore:missing-reason")`
- `valid_requires_live_passes`: `#[ignore = "requires live Kafka at localhost:9092"]` → 0 violations
- `valid_requires_pre_built_passes`: `#[ignore = "requires pre-built guest wasm"]` in `crates/components/camel-component-wasm/tests/foo.rs` listed in allowlist → 0 violations
- `valid_slow_test_passes`: `#[ignore = "slow test: file polling"]` → 0 violations
- `invalid_prefix_is_violation`: `#[ignore = "because reasons"]` → 1 violation `.snippet.contains("ignore:invalid-prefix")`
- `near_prefix_typo_rejected`: `#[ignore = "requires livewire foo"]` → 1 violation `.snippet.contains("ignore:invalid-prefix")`
- `case_sensitive_prefix`: `#[ignore = "Requires live Redis"]` → 1 violation `.snippet.contains("ignore:invalid-prefix")`
- `empty_detail_rejected`: `#[ignore = "requires live "]` → 1 violation `.snippet.contains("ignore:empty-detail")`
- `prefix_without_detail_no_space`: `#[ignore = "requires live"]` → 1 violation (assert the emitted rule — either `invalid-prefix` since no delimiter follows, or `empty-detail`)
- `slow_test_wrong_delimiter_rejected`: `#[ignore = "slow test file polling"]` (missing colon) → 1 violation `.snippet.contains("ignore:invalid-prefix")`
- `requires_live_wrong_delimiter_rejected`: `#[ignore = "requires live: Kafka"]` (colon instead of space) → 1 violation `.snippet.contains("ignore:invalid-prefix")`
- `requires_pre_built_wrong_delimiter_rejected`: `#[ignore = "requires pre-built: wasm"]` → 1 violation `.snippet.contains("ignore:invalid-prefix")`
- `ignore_in_comment_not_violation`: file with `// REDIS-009: ... (#[ignore] by default)` on a comment line → 0 violations from that line; file with `//! All tests ... #[ignore] ...` → 0 violations from that line
- `scripts_dir_excluded`: `#[ignore]` in `scripts/foo.rs` → 0 violations (not scanned)
- `tests_dir_scanned`: `#[ignore = "requires live Foo"]` in `crates/foo/tests/bar.rs` → 0 violations (test files ARE scanned, unlike lint_log_levels)
- `pre_built_not_in_allowlist`: `#[ignore = "requires pre-built wasm"]` in `crates/foo/tests/bar.rs`, file not in allowlist → 1 violation `.snippet.contains("ignore:pre-built-not-in-allowlist")`
- `allowlist_out_of_scope`: allowlist contains `crates/other/tests/foo.rs` → 1 violation `.snippet.contains("ignore:allowlist-out-of-scope")`
- `allowlist_stale`: allowlist contains `crates/components/camel-component-wasm/tests/nonexistent.rs` → 1 violation `.snippet.contains("ignore:allowlist-stale")` and `.line == 0`
- `allowlist_no_pre_built_test`: allowlist contains a path to a real WASM test file that has only `requires live` tests (no `requires pre-built`) → 1 violation `.snippet.contains("ignore:allowlist-no-pre-built-test")`
- `allowlist_mixed_reasons`: allowlisted file contains both `requires pre-built` and `requires live` → 1 violation `.snippet.contains("ignore:allowlist-mixed-reasons")`

**Acceptance:**
- `cargo test -p xtask` passes (all new unit tests green)
- `cargo clippy -p xtask -- -D warnings` passes
- `cargo fmt --check` passes on modified `main.rs`

- [x] 1

## docs/adr

### Task 2: Write ADR-0054 — `#[ignore]` test classification and enforcement policy

**Files:**
- `docs/adr/0054-ignore-test-classification-policy.md` (new)

**Steps:**
1. Read ADR-0049 (`docs/adr/0049-*.md`) and ADR-0012 (`docs/adr/0012-*.md`) for the established ADR format in this repo. Follow the same section structure: Context (Problem, sub-sections), Decision (Scope, Rule, Exceptions), Considered options, Consequences (with Self-grill record).
2. Write the ADR with these sections:
   - **Status:** Proposed
   - **Related:** ADR-0012 (lint+policy pairing precedent), ADR-0049 (lint+policy precedent), ADR-0053 (the WIT break that exposed the gap)
   - **Context > Problem:** 29 `#[ignore]` tests, zero enforcement. ADR-0053's `camel:plugin@1.0.0` broke 13 buildable WASM tests that the merge gate silently skipped. No lint, no ADR, no CI coverage existed.
   - **Context > Why a workspace ADR:** the question is workspace-wide — `#[ignore]` semantics affect every crate, not just WASM.
   - **Decision > Closed vocabulary:** the three prefixes with their exact grammar (from design.md). Cite the grammar table.
   - **Decision > Rule:** a test may only be `#[ignore]` for a prerequisite CI cannot cheaply satisfy. Buildable artifacts are cheaply satisfiable; such tests MUST run in a dedicated CI job.
   - **Decision > Allowlist coupling:** bidirectional contract. Forward + reverse + mixed-reason checks. The allowlist is consumed by both lint and CI.
   - **Decision > No escape hatch:** rationale for strict vocabulary.
   - **Considered options:** do-nothing, docker-compose Category 1, delete `#[ignore]` from Category 2, catch-all reason, escape hatch — each with rejection rationale.
   - **Consequences:** the class of ABI/contract break is closed. Contributors get one vocabulary. Marginal CI cost. Live-service tests remain dev-run.
   - **Consequences > Self-grill record:** 3-5 self-grill questions with answers (follow ADR-0049's pattern).

**Tests:**
- N/A (document). Verify by reading the ADR and checking format compliance.

**Acceptance:**
- ADR exists at `docs/adr/0054-ignore-test-classification-policy.md`
- Contains all required sections (Status, Related, Context, Decision, Considered options, Consequences, Self-grill)
- References ADR-0012, ADR-0049, ADR-0053 by number
- Grammar table matches the spec exactly (3 prefixes, their delimiters, their meanings)

- [x] 2

## Multiple crates

### Task 3: Normalize all existing `#[ignore]` reasons + create allowlist

**Files:**
- `scripts/xtask/allowlist-ignore.txt` (new)
- `crates/platforms/camel-platform-kubernetes/src/readiness_gate.rs` (modified)
- `crates/components/camel-component-wasm/tests/integration.rs` (modified)
- `crates/components/camel-component-wasm/tests/source_integration.rs` (modified)
- `crates/components/camel-component-wasm/tests/source_stream_integration.rs` (modified)
- `crates/components/camel-component-llm/tests/ollama_live.rs` (modified)
- `crates/components/camel-redis/src/lib.rs` (modified)
- `crates/components/camel-file/src/lib.rs` (modified)

**Steps:**
1. Create `scripts/xtask/allowlist-ignore.txt` with header comment and 3 entries (one per line, relative paths):
   ```
   # allowlist for ignore-test lint (see ADR-0054)
   # format: <relative path to test file>
   # Files listed here MUST contain only requires pre-built tests.
   crates/components/camel-component-wasm/tests/source_integration.rs
   crates/components/camel-component-wasm/tests/source_stream_integration.rs
   crates/components/camel-component-wasm/tests/integration.rs
   ```
2. Fix bare `#[ignore]` at `readiness_gate.rs:130`:
   Change `#[ignore]` to `#[ignore = "requires live Kubernetes cluster"]`
   (The test calls `kube::Client::try_default()` which requires a live cluster. Category: external service.)
3. Fix bare `#[ignore]` at `integration.rs:277`:
   Change `#[ignore]` to `#[ignore = "requires pre-built guest wasm (see fixtures README)"]`
   (The test is a stub that documents the need for a compiled .wasm fixture. Category: buildable artifact.)
4. Normalize `ollama_live.rs` — change all 8 occurrences of `"requires local Ollama with ..."` to `"requires live Ollama: ..."`:
   - Line 59: `"requires local Ollama with qwen3.5:4b"` → `"requires live Ollama: qwen3.5:4b"`
   - Line 124: same pattern
   - Line 172: `"requires local Ollama with embeddinggemma"` → `"requires live Ollama: embeddinggemma"`
   - Line 225: same as 59
   - Line 269: `"requires local Ollama with qwen3.5:4b (tool-supporting model)"` → `"requires live Ollama: qwen3.5:4b (tool-supporting model)"`
   - Line 347: same as 269
   - Line 437: `"requires local Ollama with qwen3.5:4b and pricing configured"` → `"requires live Ollama: qwen3.5:4b with pricing configured"`
   - Line 510: `"requires local Ollama with qwen3.5:4b and cache configured"` → `"requires live Ollama: qwen3.5:4b with cache configured"`
5. Normalize `camel-redis/src/lib.rs` — change 3 occurrences of `"Requires live Redis"` to lowercase `"requires live Redis"`:
   - Lines 227, 244, 267: capitalize first letter → lowercase
6. Normalize `camel-file/src/lib.rs:2140`:
   Change `#[ignore] // Slow test - run with --ignored flag` to `#[ignore = "slow test: file polling (run with --ignored)"]`
7. Verify `camel-kafka/src/lib.rs` (lines 349, 373) and `camel-opensearch/tests/live_opensearch.rs` (line 21) — these already use `requires live ` prefix and need no change.
8. Verify `source_integration.rs` and `source_stream_integration.rs` — these already use `requires pre-built guest wasm (see module docs)` and need no change. Confirm the prefix matches `requires pre-built ` exactly.
9. Run `cargo xtask lint-ignore` to verify zero violations.

**Tests:**
- `lint_ignore_passes_after_normalization`: run `cargo xtask lint-ignore` in the worktree → exits 0 with 0 violations

**Acceptance:**
- `cargo xtask lint-ignore` exits 0
- No test bodies modified — only `#[ignore]` reason strings changed
- `cargo test -p camel-component-wasm --lib` still compiles (no syntax errors from reason string changes)
- `cargo test -p camel-component-llm --lib` still compiles
- `cargo test -p camel-file --lib` still compiles

- [x] 3

## Config

### Task 4: Register `lint-ignore` in quality gates

**Files:**
- `AGENTS.md` (modified)
- `.github/workflows/ci.yml` (modified)

**Steps:**
1. In `AGENTS.md`, add a new entry to the QUALITY GATES block after `lint-log-levels`:
   ```yaml
   - name: lint-ignore
     run: cargo xtask lint-ignore
   ```
2. In `.github/workflows/ci.yml`, add a new step to the `quality` job after the `lint-non-exhaustive` step:
   ```yaml
        - name: lint-ignore
          run: cargo xtask lint-ignore
   ```

**Tests:**
- N/A (config change). Verify by reading both files and confirming the new entries are present.

**Acceptance:**
- `AGENTS.md` QUALITY GATES block contains `lint-ignore` entry
- `ci.yml` quality job contains `lint-ignore` step
- YAML is syntactically valid

- [x] 4

## CI

### Task 5: Add `wasm-integration` CI job

**Files:**
- `.github/workflows/ci.yml` (modified)

**Steps:**
1. Read the existing CI config to understand the toolchain+cache+build pattern. The relevant block is the `unit-tests` job (checkout → Maximize disk space → rust-toolchain → rust-cache → Install libclang → Build → Test). The `full-tests-linux` job (lines 46-57 for setup, then bridge build+test steps) shows the bridge-build pattern.
2. Read `crates/components/camel-component-wasm/tests/source_integration.rs` lines 60-65 and `source_stream_integration.rs` lines 37-42 to understand how the tests resolve the guest wasm path: they use a constant `GUEST_WASM_FILE: &str = "wasm32-wasip2/debug/wasm_source_webhook_guest.wasm"` resolved relative to the Cargo target directory. So building the guest crate places the .wasm where the test expects it — NO manual copy needed for these two test files.
3. Read `crates/components/camel-component-wasm/tests/integration.rs` line 277 to confirm the third allowlisted test is a placeholder stub (test body is `let _ = "placeholder..."`).
4. Add a new job `wasm-integration` to `.github/workflows/ci.yml` (after the `quality` job). Structure:
   ```yaml
     wasm-integration:
       name: WASM Integration (ubuntu-latest)
       runs-on: ubuntu-latest
       steps:
         - uses: actions/checkout@<same-pin-as-other-jobs>
         - name: Maximize disk space
           run: |
             sudo rm -rf /usr/share/dotnet /usr/local/lib/android /opt/ghc /opt/hostedtoolcache/CodeQL
             sudo apt-get clean
             df -h /
         - uses: dtolnay/rust-toolchain@<same-pin-as-other-jobs>
           with:
             toolchain: stable
             targets: wasm32-wasip2
         - uses: Swatinem/rust-cache@<same-pin-as-other-jobs>
         - name: Install libclang for bindgen
           run: sudo apt-get install -y libclang-dev
         - name: Build WASM guest crates
           run: |
             # Build all guest crates that produce .wasm components.
             # A guest compile failure MUST fail the job — this is the
             # primary oracle for detecting WIT version breakage (ADR-0054).
             # source_integration and source_stream_integration tests resolve
             # the guest wasm from the Cargo target dir — no copy needed.
             for guest in examples/wasm-source-webhook/guest \
                          examples/wasm-bean-example/guest \
                          examples/wasm-streaming-plugin/guest \
                          examples/security-wasm-policy/guest \
                          examples/security-wasm-policy/guest-init-check \
                          examples/wasm-example/guest; do
               echo "::group::Building $guest"
               (cd "$guest" && cargo build --target wasm32-wasip2 2>&1)
               echo "::endgroup::"
             done
         - name: WASM integration tests (per-target, --ignored)
           run: |
             # Run buildable #[ignore] tests per cargo test target.
             # source_integration and source_stream_integration resolve the
             # guest wasm from target/wasm32-wasip2/debug/ — these provide
             # the real WIT-breakage oracle.
             # integration target's test is a placeholder stub (WASM-010);
             # it passes trivially and provides no oracle signal, but stays
             # in the loop because it is in the allowlist.
             for target in source_integration source_stream_integration integration; do
               echo "Running --ignored tests for target: $target"
               cargo test -p camel-component-wasm --test "$target" -- --ignored
             done
   ```
   Use the same action pin hashes as the other jobs in the file (copy from existing jobs). Do NOT invent new pins.

**Tests:**
- N/A (CI config). Verify by reading the YAML and confirming the job is syntactically valid.

**Acceptance:**
- `.github/workflows/ci.yml` contains a `wasm-integration` job
- Job installs `wasm32-wasip2` target via `targets:` key
- Job builds WASM guest crates
- Job runs `cargo test -p camel-component-wasm --test <target> -- --ignored` per target for the 3 allowlisted targets
- YAML is syntactically valid (verify with `python3 -c "import yaml; yaml.safe_load(open('.github/workflows/ci.yml'))"`)
- Local dry-run: run `cargo build --target wasm32-wasip2` in `examples/wasm-source-webhook/guest/` (requires wasm target installed), then run `cargo test -p camel-component-wasm --test source_integration -- --ignored` and confirm the buildable tests execute (not skipped). Report the result.

- [x] 5
