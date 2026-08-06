# Tasks: non-exhaustive-contract-enforcement

<!--
  Multi-phase change. Phase 1 applies ADR-0049 (rc-3pw3); Phase 2 adds the
  enforcement lint (rc-ierl). The whole plan is blessed ONCE; PHASE 3
  implements phase-group by phase-group with an inter-phase review between them.
-->

## Phase 1: Apply ADR-0049 contract-enum attributes

### Task 1.1: Add exhaustive-by-contract exception notes to the two closed-set enums

**Files:**
- `crates/camel-api/src/pipeline_outcome.rs` (modified)
- `crates/camel-api/src/exchange.rs` (modified)

**Steps:**
1. In `crates/camel-api/src/pipeline_outcome.rs`, directly above `pub enum PipelineOutcome {` (line 17), add a rustdoc note:
   `/// exhaustive-by-contract: the Completed|Stopped|Failed set is the deliberate outcome algebra of ADR-0024; adding a variant is a reviewed breaking change, not additive growth.`
2. Confirm `PipelineOutcome` is NOT marked `#[non_exhaustive]` (it must stay exhaustive so the in-crate `into_tower_result()` exhaustive match keeps its correctness guarantee per ADR-0024).
3. In `crates/camel-api/src/exchange.rs`, directly above `pub enum ExchangePattern {` (line 46), add a rustdoc note:
   `/// exhaustive-by-contract: the InOnly|InOut MEP dichotomy is a fixed, spec-level closed set.`
4. Confirm `ExchangePattern` is NOT marked `#[non_exhaustive]`.
5. Run `cargo build -p camel-api` to confirm no breakage (exceptions do not gain the attribute, so no match loses exhaustiveness).

**Tests:** (verification commands)
- `rg -n -B1 'pub enum PipelineOutcome' crates/camel-api/src/pipeline_outcome.rs` → the preceding line matches `/// exhaustive-by-contract:` with non-empty rationale.
- `rg -n -B1 'pub enum ExchangePattern' crates/camel-api/src/exchange.rs` → same.
- `rg -c '#\[non_exhaustive\]' crates/camel-api/src/pipeline_outcome.rs crates/camel-api/src/exchange.rs` → both report `0` (the exceptions stay exhaustive).

**Acceptance:**
- Both exception rustdoc notes present with non-empty rationale.
- Neither exception enum carries `#[non_exhaustive]`.
- `cargo build -p camel-api` exits 0.

- [x] 1.1

### Task 1.2: Apply #[non_exhaustive] to camel-component-api and camel-language-api contract enums

**Files:**
- `crates/components/camel-component-api/src/consumer.rs` (modified)
- `crates/languages/camel-language-api/src/error.rs` (modified)
- Predicted blast zone (out-of-crate `match` sites): ~134 workspace files reference contract-enum variants; the compiler identifies the subset containing non-wildcard `match` arms on `ConsumerStartupMode`/`ConcurrencyModel`/`LanguageError`. Likely affected crates: `camel-core`, `camel-component-*` (consumer/health impls). Each modified `_ =>` arm is recorded in the task commit message (exact files are compiler-confirmed, not pre-specifiable — a `match` loses exhaustiveness only if it has no existing wildcard arm).

**Steps:**
1. In `crates/components/camel-component-api/src/consumer.rs`, add `#[non_exhaustive]` directly above `pub enum ConsumerStartupMode {` (line 38).
2. In the same file, add `#[non_exhaustive]` directly above `pub enum ConcurrencyModel {` (line 374).
3. In `crates/languages/camel-language-api/src/error.rs`, add `#[non_exhaustive]` directly above `pub enum LanguageError {` (line 4).
4. Run `cargo build --workspace`. The compiler reports every out-of-crate `match` on `ConsumerStartupMode` / `ConcurrencyModel` / `LanguageError` that lost exhaustiveness.
5. For each broken match, add a forward-safe `_ =>` arm:
   - Permitted: `unreachable!()` ONLY when an invariant independent of the current variant set guarantees the wildcard is unreachable (e.g. the value was just constructed from a known variant) — add a `// INVARIANT:` comment stating it.
   - Otherwise: an explicit branch returning an error or a documented default, with existing behavioural test coverage.
   - Forbidden: a bare `unreachable!()` that assumes "no other variant exists" (that is the silent-mishandle hole ADR-0049 exists to prevent).
6. Record a per-arm inventory in the task commit body so the Phase-1→Phase-2 reviewer can audit each wildcard without re-deriving from source. One line per new `_ =>` arm:
   `<file>:<line> | <EnumName> | <invariant-justified: quote the invariant> | OR | <explicit-default: → <test_fn_name covering that branch>>`
   (An `unreachable!()` arm has no behavioural coverage BY DESIGN, so `cargo test` cannot police it — the commit-body inventory is the audit artifact.)
7. Run `cargo test --workspace --lib` to confirm existing behavioural coverage still passes.
8. Run `cargo fmt --check --all` and `cargo clippy -p camel-component-api -p camel-language-api -- -D warnings`.

**Tests:** (verification)
- `rg -n -B1 'pub enum (ConsumerStartupMode|ConcurrencyModel)' crates/components/camel-component-api/src/consumer.rs` → each preceding line is `#[non_exhaustive]`.
- `rg -n -B1 'pub enum LanguageError' crates/languages/camel-language-api/src/error.rs` → preceding line is `#[non_exhaustive]`.
- `cargo build --workspace` exits 0.
- `cargo test --workspace --lib` exits 0 (no behavioural regression from new wildcard arms).

**Acceptance:**
- All 3 enums carry `#[non_exhaustive]`.
- `cargo build --workspace` exits 0; every new `_ =>` arm is either invariant-justified (with `// INVARIANT:` comment) or an explicit non-panic branch.
- The task commit body contains the per-arm inventory (one line per new `_ =>` arm, per step 6).
- `cargo clippy -p camel-component-api -p camel-language-api -- -D warnings` exits 0.
- `cargo fmt --check --all` exits 0.

- [x] 1.2

### Task 1.3: Apply #[non_exhaustive] to all camel-api contract enums and fix the workspace match cascade

**Files (attribute application — modified):**
- `crates/camel-api/src/error_handler.rs` (modified)
- `crates/camel-api/src/aggregator.rs` (modified)
- `crates/camel-api/src/resequencer.rs` (modified)
- `crates/camel-api/src/body.rs` (modified)
- `crates/camel-api/src/platform.rs` (modified)
- `crates/camel-api/src/route_controller.rs` (modified)
- `crates/camel-api/src/declarative.rs` (modified)
- `crates/camel-api/src/component_metadata.rs` (modified)
- `crates/camel-api/src/function.rs` (modified)
- `crates/camel-api/src/body_converter.rs` (modified)
- `crates/camel-api/src/step_lifecycle.rs` (modified)
- `crates/camel-api/src/multicast.rs` (modified)
- `crates/camel-api/src/loop_eip.rs` (modified)
- `crates/camel-api/src/runtime.rs` (modified)
- `crates/camel-api/src/load_balancer.rs` (modified)
- `crates/camel-api/src/ssrf.rs` (modified)
- `crates/camel-api/src/splitter.rs` (modified)
- `crates/camel-api/src/exchange_lookup.rs` (modified)
- `crates/camel-api/src/throttler.rs` (modified)
- `crates/camel-api/src/lifecycle.rs` (modified)
- `crates/camel-api/src/security_policy.rs` (modified)
- Predicted blast zone (out-of-crate `match` sites): ~134 workspace files reference contract-enum variants across `camel-core` (lifecycle/CQRS/context/health/hot-reload), `camel-dsl` (compile/yaml), `camel-processor` (load_balancer/multicast/throttler/streaming_splitter/security_policy_layer), `camel-builder`, `camel-cli`, `camel-config`, `camel-test`, `camel-health`, `services/*`, `components/*`, and `examples/`. The compiler identifies the subset containing non-wildcard `match` arms on the 48 attributed enums; each modified `_ =>` arm is recorded in the task commit message (exact files are compiler-confirmed, not pre-specifiable — only `match` expressions without an existing wildcard arm break).

**Steps:**
1. Add `#[non_exhaustive]` directly above each of these 48 `pub enum` declarations in `crates/camel-api/src/` (do NOT touch the 3 already-compliant enums `CamelError`/`ConfigValidationError`/`TemplateError`, nor the 2 exceptions handled in Task 1.1):
   - `error_handler.rs`: `ExceptionDisposition` (93), `RetryOutcome` (112), `StepDisposition` (163), `BoundaryKind` (171)
   - `aggregator.rs`: `CorrelationStrategy` (22), `AggregationStrategy` (61), `CompletionCondition` (79), `CompletionMode` (113), `CompletionReason` (128)
   - `resequencer.rs`: `BatchCompletion` (12), `GapPolicy` (24), `CapacityPolicy` (34), `ResequenceMode` (44)
   - `body.rs`: `Body` (162)
   - `platform.rs`: `LeadershipEvent` (34), `PlatformError` (41)
   - `route_controller.rs`: `RouteStatus` (12), `RouteAction` (29)
   - `declarative.rs`: `ValueSourceDef` (23)
   - `component_metadata.rs`: `OptionKind` (17)
   - `function.rs`: `PatchBody` (48), `FunctionInvocationError` (55)
   - `body_converter.rs`: `BodyType` (9), `BodyConverterError` (18)
   - `step_lifecycle.rs`: `StepShutdownReason` (6)
   - `multicast.rs`: `MulticastStrategy` (9)
   - `loop_eip.rs`: `LoopMode` (5)
   - `runtime.rs`: `CanonicalStepSpec` (113), `CanonicalSplitExpressionSpec` (180), `CanonicalSplitAggregationSpec` (199), `CanonicalAggregateStrategySpec` (217), `CanonicalConcurrencySpec` (281), `RuntimeCommand` (444), `RuntimeCommandResult` (542), `RuntimeQuery` (565), `RuntimeQueryResult` (579), `RuntimeEvent` (587)
   - `load_balancer.rs`: `LoadBalanceStrategy` (2)
   - `ssrf.rs`: `SsrfPolicy` (26)
   - `splitter.rs`: `AggregationStrategy` (28), `StreamSplitFormat` (65)
   - `exchange_lookup.rs`: `PathSegment` (20), `ExchangeLookupPath` (30), `LookupPathError` (46)
   - `throttler.rs`: `ThrottleStrategy` (4)
   - `lifecycle.rs`: `ServiceStatus` (8), `HealthStatus` (16)
   - `security_policy.rs`: `AuthorizationDecision` (37)
2. Run `cargo build --workspace`. The compiler reports every out-of-crate `match` that lost exhaustiveness (expected blast radius: camel-core lifecycle/CQRS, camel-dsl compiler, camel-processor, components).
3. For each broken match, add a forward-safe `_ =>` arm with the same rule as Task 1.2 step 5: `unreachable!()` only with an independent-invariant `// INVARIANT:` comment; otherwise an explicit error/default branch with behavioural coverage; never a bare "no other variant" `unreachable!()`.
4. Record a per-arm inventory in the task commit body, same format as Task 1.2 step 6 (one line per new `_ =>` arm: `file:line | Enum | invariant-justified: <quote> | explicit-default: → <test_fn>`). This is the audit artifact for the Phase-1→Phase-2 reviewer since `unreachable!()` arms have no behavioural coverage by design.
5. Confirm the `variant_name()` exhaustive-guard tests for the newly-attributed enums live IN-CRATE (per ADR-0049 §Exceptions third bullet): `rg -n 'fn variant_name' crates/camel-api/src/` and verify each guard's match is in the defining crate (where `#[non_exhaustive]` does not force a wildcard). If any guard is out-of-crate, it now needs a `_ =>` arm — report it as a discovered finding rather than silently adding a wildcard.
6. Run `cargo test --workspace --lib` to confirm existing behavioural coverage still passes (this is the regression net for the new wildcard arms).
7. Run `cargo fmt --check --all`.
8. Run `cargo clippy --workspace --all-features --exclude camel-cli --exclude camel-component-kafka -- -D warnings`, then `cargo clippy -p camel-cli -- -D warnings`, then `cargo clippy -p camel-component-kafka --all-targets -- -D warnings`.

**Tests:** (verification)
- `rg -B1 'pub enum ' crates/camel-api/src/ -r '$0'` followed by a check that every `pub enum` (except `PipelineOutcome`, `ExchangePattern`, `CamelError`, `ConfigValidationError`, `TemplateError`) is preceded by `#[non_exhaustive]`. Concretely: `rg -n 'pub enum ' crates/camel-api/src/` lists 53 sites; subtracting the 5 excluded, the remaining 48 each have `#[non_exhaustive]` on the immediately preceding non-blank line.
- `cargo build --workspace` exits 0.
- `cargo test --workspace --lib` exits 0.

**Acceptance:**
- All 48 targeted camel-api enums carry `#[non_exhaustive]`; the 5 excluded enums are unchanged (3 already-compliant, 2 exceptions).
- `cargo build --workspace` exits 0.
- Every new out-of-crate `_ =>` arm is either invariant-justified (with `// INVARIANT:` comment) or an explicit non-panic branch; no bare "no other variant" `unreachable!()`.
- The task commit body contains the per-arm inventory (one line per new `_ =>` arm, per step 4).
- `variant_name()` guard tests for the attributed enums are confirmed in-crate (step 5); any out-of-crate guard reported as a discovered finding.
- `cargo test --workspace --lib` exits 0.
- `cargo clippy --workspace --all-features --exclude camel-cli --exclude camel-component-kafka -- -D warnings` exits 0; `cargo clippy -p camel-cli -- -D warnings` exits 0; `cargo clippy -p camel-component-kafka --all-targets -- -D warnings` exits 0.
- `cargo fmt --check --all` exits 0.

- [x] 1.3

## Phase 2: Enforcement lint

### Task 2.1: Implement cargo xtask lint-non-exhaustive with unit tests

**Files:**
- `scripts/xtask/src/main.rs` (modified)

**Steps:**
1. In the `Commands` enum (near the existing `LintUnwrap`/`LintSecrets`/`LintLogLevels` variants around line 71-83), add a new variant:
   ```rust
   /// Enforce ADR-0049: pub enums in the contract crates must be
   /// #[non_exhaustive] or carry a `/// exhaustive-by-contract: <rationale>`
   /// rustdoc note. Exits non-zero on violations.
   LintNonExhaustive,
   ```
   Add a matching doc comment above the existing `LintUnwrap` doc pattern.
2. Add a dispatch arm in the `match self.command` block (near line 204, after `Commands::LintLogLevels =>`), mirroring the `lint-unwrap` dispatch shape:
   ```rust
   Commands::LintNonExhaustive => {
       let workspace_root = workspace_root_or_exit();
       match lint_non_exhaustive(&workspace_root) {
           Ok(violations) if violations.is_empty() => {
               println!("lint-non-exhaustive: OK (no violations)");
           }
           Ok(violations) => {
               println!("NON-EXHAUSTIVE VIOLATIONS ({} found):", violations.len());
               for v in &violations {
                   println!("  {}:{}  {}", v.file, v.line, v.snippet.trim());
               }
               eprintln!("\nlint-non-exhaustive: FAILED");
               std::process::exit(1);
           }
           Err(e) => {
               eprintln!("lint-non-exhaustive error: {e}");
               std::process::exit(1);
           }
       }
   }
   ```
3. Add the unit-testable core function (reuse the existing `Violation` struct at line 1235; do NOT introduce a new violation type):
   ```rust
   /// Scan source `src` for `pub enum` declarations that violate ADR-0049:
   /// a contract-crate pub enum must carry `#[non_exhaustive]` OR a directly
   /// attached `/// exhaustive-by-contract: <non-empty rationale>` rustdoc note.
   /// A plain `//` comment or an empty rationale does NOT satisfy the rule.
   pub fn lint_non_exhaustive_src(src: &str, file_path: &str) -> Vec<Violation> { /* body per algorithm below */ }
   ```
   Algorithm: iterate lines; on a line matching `^\s*pub\s+enum\s+(\w+)`, scan backwards over the contiguous attached region — a blank line is the region TERMINATOR (Rust doc-attachment requires the note directly attached, with no blank line between note and item; a detached note has no effect and must not count as compliant). Within the region include lines that are `#[...]` attributes, `///` rustdoc, or `//` comments, stopping at the first blank line or at a line that is none of these. The enum is compliant iff the region contains `#[non_exhaustive]` OR contains a line matching `^\s*///\s*exhaustive-by-contract:\s*\S` (rustdoc with non-empty rationale after the colon). Otherwise push a `Violation { file, line, snippet }`.
4. Add the walker function, restricted to the three contract crates (do NOT scan the whole workspace — ADR-0049 §Scope is deliberately narrow):
   ```rust
   pub fn lint_non_exhaustive(workspace_root: &Path) -> Result<Vec<Violation>, String> {
       // walk ONLY these three roots, reusing the WalkDir + is_test_file filter
       // from lint_unwrap but scoped:
       //   crates/camel-api/src
       //   crates/components/camel-component-api/src
       //   crates/languages/camel-language-api/src
       // REUSE is_test_file to EXCLUDE test files: a pub enum inside a
       // #[cfg(test)] module is NOT released contract surface and would
       // false-positive. Skip target/ and .worktrees/ components.
   }
   ```
5. Add unit tests in a `#[cfg(test)] mod tests` block (or extend the existing test module) for `lint_non_exhaustive_src` covering: pass-with-attribute, fail-without-anything, pass-with-valid-rustdoc-note, reject-plain-comment-marker, reject-empty-rationale, ignore-non-pub-enum.
6. Run `cargo test -p xtask --lib` to confirm the unit tests pass.
7. Run `cargo run -p xtask -- lint-non-exhaustive` and confirm it prints `lint-non-exhaustive: OK (no violations)` (the crates are compliant after Phase 1) — if Phase 1 is already merged into this worktree, this is the green baseline; if run before Phase 1 it reports violations (expected).
8. Command-level negative coverage (spec scenario "Lint fails on a non-compliant enum" at the binary level, complementing the unit tests): temporarily comment out one `#[non_exhaustive]` in `crates/camel-api/src/lifecycle.rs` (`ServiceStatus`), run `cargo run -p xtask -- lint-non-exhaustive`, and confirm it exits non-zero AND prints the offending `crates/camel-api/src/lifecycle.rs:<line>`. Then revert the edit. Record the observed line in the task commit body.

**Tests:** (executable unit tests on `lint_non_exhaustive_src`)
- `lint_passes_enum_with_non_exhaustive`: src `#[non_exhaustive]\npub enum E { A }` → returns `[]`.
- `lint_fails_enum_without_attribute_or_note`: src `pub enum E { A }` → returns 1 violation with line = enum line.
- `lint_passes_enum_with_valid_exception_note`: src `/// exhaustive-by-contract: closed set is the contract\npub enum E { A }` → returns `[]`.
- `lint_rejects_plain_comment_marker`: src `// exhaustive-by-contract: foo\npub enum E { A }` (non-rustdoc `//`) → returns 1 violation (the marker is invalid).
- `lint_rejects_empty_rationale`: src `/// exhaustive-by-contract:\npub enum E { A }` (empty after colon) → returns 1 violation.
- `lint_ignores_non_pub_enum`: src `enum Internal { A }` → returns `[]`.
- `lint_rejects_detached_marker`: src `/// exhaustive-by-contract: closed set\n\npub enum E { A }` (blank line between note and enum detaches the rustdoc) → returns 1 violation (the note is NOT directly attached, so it does not satisfy ADR-0049 §Rule 3).
- command: `cargo test -p xtask --lib lint_non_exhaustive` → all pass.

**Acceptance:**
- `cargo run -p xtask -- lint-non-exhaustive` runs without error.
- On the Phase-1-compliant crates it exits 0 and prints `lint-non-exhaustive: OK (no violations)`.
- All 7 unit tests pass via `cargo test -p xtask --lib`.
- Command-level negative test (step 8) confirmed: removing one `#[non_exhaustive]` makes the binary exit non-zero and print the offending `file:line`; the edit is reverted.
- `cargo clippy -p xtask -- -D warnings` exits 0; `cargo fmt --check --all` exits 0.

- [x] 2.1

### Task 2.2: Register lint-non-exhaustive in AGENTS.md and CI workflow

**Files:**
- `AGENTS.md` (modified)
- `.github/workflows/ci.yml` (modified)

**Steps:**
1. In `AGENTS.md`, in the `## QUALITY GATES` block (line 16), add a new entry directly after the `lint-secrets` entry (around line 33-34), matching the existing indentation/format — anchoring at `lint-secrets` to match the ci.yml insertion point (avoids needless divergence):
   ```
   - name: lint-non-exhaustive
     run: cargo xtask lint-non-exhaustive
   ```
2. In `.github/workflows/ci.yml`, in the quality-gates job, add a new step directly after the `lint-secrets` step (line 189-190), matching the existing format:
   ```yaml
       - name: lint-non-exhaustive
         run: cargo xtask lint-non-exhaustive
   ```
3. Run `cargo run -p xtask -- lint-non-exhaustive` and confirm exit 0 on the compliant crates (the gate must be green where it is enforced).
4. Confirm the gate is listed: `rg -n 'lint-non-exhaustive' AGENTS.md .github/workflows/ci.yml` returns both files.

NOTE: `conductor-light.md` also carries a duplicated, hard-coded gate list (with a "12 gates" count) for the conductor agent's local PHASE 4 execution. This task does NOT modify it — that duplication is an anti-pattern tracked separately (the conductor gate list should derive from AGENTS.md, not be hand-synced). See bd follow-up rc-fuxr.

**Tests:** (verification)
- `rg -n 'lint-non-exhaustive' AGENTS.md` → at least one hit in the QUALITY GATES block.
- `rg -n 'lint-non-exhaustive' .github/workflows/ci.yml` → at least one hit in the quality-gates job.
- `cargo run -p xtask -- lint-non-exhaustive` → exits 0, prints `lint-non-exhaustive: OK (no violations)`.

**Acceptance:**
- `lint-non-exhaustive` step present in `AGENTS.md` QUALITY GATES and `.github/workflows/ci.yml`.
- `cargo run -p xtask -- lint-non-exhaustive` exits 0 on the now-compliant crates (Phase 1 must be complete for this to pass — it is a dependency of this task).

- [x] 2.2
