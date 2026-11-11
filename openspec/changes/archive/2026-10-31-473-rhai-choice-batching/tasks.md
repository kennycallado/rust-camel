# Tasks: 473-rhai-choice-batching

<!-- Single-phase change. Bench-first baseline (choice-pre) already recorded in
     benchmarks/records/20261031T191644Z/ on commit 1bb202e161b45fe2b2a17c54ab66f86acb14791. -->

## Task 473-t1: language-api group hook (PredicateGroup + IndexedPredicateError)

Files:
- `crates/languages/camel-language-api/src/lib.rs` (modified)

Steps:
1. Add `pub struct IndexedPredicateError { pub index: usize, pub error: LanguageError }` with a `Debug` derive and a `Display` impl that names the clause index and renders ONLY the inner error's class and `redacted_detail()` (never the inner error's raw `Display` — `LanguageError::EvalError(String)` can carry arbitrary data).
2. Add `#[async_trait] pub trait PredicateGroup: Send + Sync { async fn first_match(&self, exchange: &Exchange) -> Result<Option<usize>, IndexedPredicateError>; }` — doc comment: evaluates predicates in declaration order, `Ok(Some(i))` = first matching clause index, `Ok(None)` = no match, `Err(IndexedPredicateError)` = clause `i` evaluation failed; skipped clauses are never evaluated.
3. Add defaulted method to `trait Language`: `fn create_predicate_group(&self, scripts: &[&str]) -> Result<Option<Box<dyn PredicateGroup>>, LanguageError> { let _ = scripts; Ok(None) }` with a doc comment stating the default means "group evaluation not supported; callers fall back to per-clause sequential evaluation". Do NOT change any existing trait method signature.
4. Extend the crate doc-comment header list of main traits with `PredicateGroup`.

Tests:
- `default_create_predicate_group_returns_none` — setup: existing `MockLanguage` in `lib.rs` tests; action: call `create_predicate_group(&["a", "b"])`; assert: `Ok(None)`. command: `cargo test -p camel-language-api --lib`; expected: fails before step 3 (method missing), passes after.
- `language_trait_group_hook_is_object_safe` — setup: `&dyn Language` binding of `MockLanguage`; action: call `create_predicate_group` through the dyn binding and hold the result as `Option<Box<dyn PredicateGroup>>`; assert: compiles and returns `Ok(None)`. Same command; expected: passes after step 3.
- `indexed_predicate_error_display_names_index` — setup: `IndexedPredicateError { index: 2, error: LanguageError::EvalError("SECRET".into()) }`; action: format with `{}`; assert: output contains "2" and does NOT contain "SECRET" (class/redacted-detail rendering only). Same command.

Acceptance:
- `cargo test -p camel-language-api --lib` passes.
- `cargo clippy -p camel-language-api --all-targets -- -D warnings` exits 0.
- `cargo build -p camel-language-rhai -p camel-language-simple` still passes (no trait impl breakage from the new defaulted method).

- [x] 473-t1

## Task 473-t2a: rhai group implementation (RhaiPredicateGroup)

Files:
- `crates/languages/camel-language-rhai/src/lib.rs` (modified)

Steps:
1. Read the existing `create_predicate` impl (`lib.rs` around line 1117) and the private helpers it uses (`compile_read_only`, `body_to_dynamic`, `make_scope`, `create_eval_engine`, `eval_sync`, `resolve_rhai_limits`). The group reuses exactly these; private associated fns of the crate-root `impl RhaiLanguage` are callable from a sibling module in the same crate.
2. Add `pub struct RhaiPredicateGroup { asts: Vec<Arc<AST>>, limits: RhaiLimitsConfig }` plus `#[cfg(test)]` per-instance instrumentation (NEVER a global/shared static — tests run in parallel): `snapshot_conversions: AtomicUsize` (Relaxed ops) and `eval_log: Arc<Mutex<Vec<usize>>>` recording each clause index at evaluation. Both live in the struct (test-only build); `RhaiPredicateGroup` must satisfy `Send + Sync` (`AtomicUsize`/`Arc<Mutex<_>>` are Sync; a `Cell` would break the trait bound with E0277). Expose the struct only via the trait; counters are read by tests through `#[cfg(test)]` accessors.
3. Override `create_predicate_group` in the `impl Language for RhaiLanguage` block: compile each script through the same read-only path `create_predicate` uses; any compile error returns `Err` (the message already names the offending source text, matching today's per-clause compile errors). Empty slice returns `Ok(None)` so zero-clause callers fall back.
4. Implement `PredicateGroup for RhaiPredicateGroup::first_match`: derive the exchange snapshot with EXACTLY ONE body conversion per call — first verify the split of `make_scope` (`lib.rs:552-570`) and `body_to_dynamic` (note: the eval path around `lib.rs:621` converts the body inside the scope helper too; audit so the group does NOT double-convert); if the existing helpers force a double conversion, extract a small shared snapshot helper (single `body_to_dynamic` + header/property map build) that both the group and (optionally) the existing path can call; increment the `snapshot_conversions` counter at the single conversion site; stream-marker bodies take the per-clause branch (exempt from the once-only rule). Build ONE eval engine ONCE per call via `create_eval_engine(limits, headers, properties)`. PRE-LOOP failures (snapshot conversion errors: unrepresentable header/property/body) return `Err(IndexedPredicateError { index: 0, error })` — clause 0 attribution. Then for each clause index i in order: fresh `Scope` seeded with the shared snapshot maps (same shape `eval_sync` uses), wrapped in a `tokio::time::timeout(execution_timeout_ms, tokio::task::spawn_blocking(move || eval))` future whose exact shape mirrors `eval_async` at `lib.rs:954-986` (the engine is shared — rhai `Engine` evals take `&self`, clone or `Arc` it into the blocking task). FAILURE-CONTRACT PARITY with `eval_sync` (`lib.rs:742-768`): each clause evaluation preserves the post-evaluation stream-refusal check and the `dynamic_to_value` conversion so clause errors carry the same classes/positions as today. Strict-bool check identical to `RhaiPredicate::matches` at `lib.rs:997-1005`. `Ok(true)` → return `Ok(Some(i))` immediately; `Ok(false)` → next clause; `Err(e)` → return `Err(IndexedPredicateError { index: i, error: e })`; after the last clause return `Ok(None)`.
5. Keep engine/snapshot count structural: the snapshot conversion and engine construction happen before the clause loop, and the loop body may not call them (enforce by code shape; the t2b counter test pins it). The clause loop records each evaluated index into `eval_log` exactly once, before evaluating that clause.

## Task 473-t2b: rhai group — semantics + isolation tests (depends on 473-t2a)

Files:
- `crates/languages/camel-language-rhai/src/lib.rs` (modified, tests only)

Evaluated-index instrumentation (defined in t2a, asserted in t2b): the per-instance `eval_log` records exactly which clause indices the group's loop evaluated, in order. Assertions assert EXACT index sequences (order + multiplicity), which is stronger than throw-probes (an error index proves only that the thrower ran, not the trace). Throw-probe scripts are retained as complementary semantic checks.

Tests (all in `camel-language-rhai`; command `cargo test -p camel-language-rhai --lib`; no `std::thread::sleep`/tokio sleeps — busy rhai loops are the only delay mechanism, `execution_timeout_ms` from a small `RhaiLimitsConfig`):
- `group_first_match_returns_first_matching_index` — clauses `header("k") == "1"`, `header("k") == "2"`, `header("k") == "3"`; exchange header k=2; expect `Ok(Some(1))` AND `eval_log == [0, 1]` (clause 2 never evaluated).
- `group_no_match_evaluates_every_clause` — 3 false clauses + final `throw "probe-last"` clause; expect `Err(IndexedPredicateError { index: 3, .. })` AND `eval_log == [0, 1, 2, 3]`; a separate 3-all-false group (no thrower) asserts `Ok(None)` with `eval_log == [0, 1, 2]`.
- `group_skipped_clauses_never_evaluate` — clause 0 matches, clause 1 is `throw "probe-1";`, clause 2 harmless; expect `Ok(Some(0))` with no error AND `eval_log == [0]` (single element — proves neither clause 1 nor 2 evaluated, even if their errors would have been discarded).
- `group_budget_workloads_are_validated_standalone` — validation harness for the accumulation test below: with operation limit L, each busy-loop clause script run STANDALONE must succeed under L and fail (operation-Limit error) under L/2; the test fails with the measured outcome if either bound does not hold (kills vacuous passes where the workload is outside the intended (L/2, L) band).
- `group_budgets_do_not_accumulate_across_clauses` — using the validated workloads from the test above (L limit, two terminating loops each in (L/2, L), both returning false); expect `Ok(None)` AND `eval_log == [0, 1]` (a cumulative counter would fail clause 1).
- `group_budget_breach_is_indexed` — clause 0 false, clause 1 unbounded `while true {}` under a tight operation limit, clause 2 `throw "probe-2";`; expect `Err(IndexedPredicateError { index: 1, .. })` with an operation-Limit class AND `eval_log == [0, 1]`.
- `group_clause_timeout_is_indexed_and_stops` — small `execution_timeout_ms` with the operation limit set HIGH AND FINITE (e.g. 1_000_000_000) so the timeout fires before any operation Limit; clause 0 false; clause 1 is a FINITE calibrated busy-loop sized > T of work (NOT an infinite loop — spawned blocking work cannot be aborted, and an infinite loop would outlive the awaited timeout and stall runtime teardown; sized work completes naturally within a short documented cleanup bound after the timeout fires); clause 2 harmless; expect `Err` with `index: 1` and `LanguageError` of `Timeout` class (`EvalFailure` with `ExpressionErrorClass::Timeout`), `eval_log == [0, 1]`, and clause 1's blocking task observed completing within the cleanup bound.
- `group_timeouts_are_independent_per_clause` — small test-only `execution_timeout_ms` T (e.g. 200ms) with operation limits set HIGH (non-binding, so the timeout is the only limiter); calibrate sleep-free busy-loop scripts at test start: time a small rhai counting loop via `Instant`, derive iterations-per-ms, target each clause at ~0.6T of rhai work. VALIDATE the final calibrated workloads through the same evaluation path before the assertion: run each clause script standalone and require measured runtime < 0.8T for each AND measured sum > 1.1T (aggregate must exceed one timeout with margin); on violation re-calibrate once, then fail with a diagnostic naming the measured values rather than asserting (prevents the test passing vacuously when calibration undersizes both loops). With validation green: run the 2-clause group (both loops returning false) and assert `Ok(None)` with `eval_log == [0, 1]` — a single timeout wrapped around the whole clause loop would expire during clause 1 and yield `Err(Timeout)`, so `Ok(None)` proves per-clause timeout wraps.
- `group_conversion_failure_is_indexed_to_clause_zero` — exchange carrying an unrepresentable header/property value (mirror the existing unrepresentable-value tests' construction); expect `Err(IndexedPredicateError { index: 0, .. })` with a Conversion-error class (pre-loop failure: `eval_log` empty).
- `group_stream_marker_body_falls_back_to_per_clause_conversion` — build an exchange whose body is a stream marker (mirror how existing stream-refusal tests construct one); group with a false clause and a matching clause; expect `Ok(Some(1))` and no stream-refusal error; additionally assert stream-read parity with the sequential path: the stream stays unconsumed until a clause actually reads it (mirror the assertions of the existing stream-body tests).
- `group_snapshot_and_engine_are_per_call_not_per_clause` — per-instance counter assertion: build a 4-clause no-match group, run `first_match`, read `snapshot_conversions`; assert exactly 1 (one snapshot conversion per call); engine count is structural (single pre-built engine referenced by the clause loop — t2a step 5) and behaviorally pinned by the budget/timeout tests above.
- `group_rejects_non_boolean_predicate_like_create_predicate` — clause script returning a non-bool; expect the same strict-bool error class as `create_predicate` today, as `Err(IndexedPredicateError { index: 0, .. })`.
- `group_is_send_and_sync` — compile-time assertion `fn assert_send_sync<T: Send + Sync>() {}` over `RhaiPredicateGroup` (the trait requires it; guards the test-instrumentation fields from regressing the bound).

Acceptance:
- `cargo test -p camel-language-rhai --lib` passes (all 13 new tests green).
- `cargo clippy -p camel-language-rhai --all-targets -- -D warnings` exits 0.
- Existing `eval_overhead` bench still compiles: `cargo bench -p camel-language-rhai --no-run`.

- [x] 473-t2a
- [x] 473-t2b

## Task 473-t3: processor ChoiceGroupSegment

Files:
- `crates/camel-processor/src/choice.rs` (modified)

Steps:
1. Add `pub struct ChoiceGroupSegment { pub group: Arc<dyn camel_language_api::PredicateGroup>, pub metas: Vec<camel_language_api::EvalMeta>, pub bodies: Vec<camel_api::OutcomeSegment>, pub otherwise: Option<camel_api::OutcomeSegment> }` with doc comment: grouped evaluation of same-language `when` clauses — one group evaluation per message replaces per-clause `PredicateSource::matches`; clause order, first-match, and error semantics identical to `ChoiceSegment`.
2. Add a constructor `ChoiceGroupSegment::new(group, metas, bodies, otherwise) -> Result<Self, CamelError>` that validates `metas.len() == bodies.len()` and `!bodies.is_empty()` (empty groups must not be built; the compiler falls back for 0/1 clauses).
3. Implement `camel_api::OutcomePipeline for ChoiceGroupSegment` (mirror `ChoiceSegment`'s clone/clone_box/run shape): `group.first_match(&exchange).await` → `Ok(Some(i))` → `self.bodies[i].run(exchange).await`; `Ok(None)` → otherwise or `PipelineOutcome::Completed(exchange)`; `Err(indexed)` → `camel_api::PipelineOutcome::Failed(camel_language_api::eval::to_expression_failed(indexed.error, &self.metas[indexed.index]))`. Never evaluate `bodies[j]` for `j != i` on a match; never call `first_match` more than once per `run`.

Tests (in `choice.rs` tests module; command `cargo test -p camel-processor --lib`; reuse the existing `append_body`/`completed_exchange` helpers):
- `choice_group_first_when_matches` — mock group returning `Ok(Some(0))`; expect clause 0 body ran.
- `choice_group_no_match_runs_otherwise` / `choice_group_no_match_passthrough_without_otherwise` — mock group `Ok(None)`; expect otherwise ran / `Completed` passthrough.
- `choice_group_error_maps_through_offending_clause_meta` — mock group returning `Err(IndexedPredicateError { index: 2, error: LanguageError::NotSupported{..} })`; metas built with per-clause `verb: format!("choice/when[{i}]")`; expect `Failed(ExpressionFailed { step_id/verb of clause 2, .. })` — assert the carried `verb` equals `choice/when[2]` (clause attribution) and the error is `CamelError::ExpressionFailed`.
- `choice_group_runs_body_of_selected_clause_only` — bodies append distinct suffixes; match on clause 1 of 3; expect only clause 1's suffix present.
- `choice_group_new_rejects_metas_body_mismatch` — `metas.len() != bodies.len()` → `Err`; empty bodies → `Err`.

Acceptance:
- `cargo test -p camel-processor --lib` passes.
- `cargo clippy -p camel-processor --all-targets -- -D warnings` exits 0.
- `cargo fmt --check -p camel-processor` exits 0.

- [x] 473-t3

## Task 473-t4: core wiring — group selection in DeclarativeChoice

Files:
- `crates/camel-core/src/lifecycle/adapters/step_resolution.rs` (modified)
- `crates/camel-core/src/lifecycle/adapters/step_compilers/control_flow.rs` (modified)

Steps:
1. In `step_resolution.rs`, add `pub(crate) fn compile_choice_group(languages: &SharedLanguageRegistry, whens: &[DeclarativeWhenStep], ctx: &crate::lifecycle::adapters::step_compilers::CompilationContext<'_>, step_index: usize) -> Result<Option<(Arc<dyn camel_language_api::PredicateGroup>, Vec<EvalMeta>)>, CamelError>` (registry/context types exactly as `compile_filter_predicate` and `eval_meta` take them): return `None` unless `whens.len() >= 2` AND every `when.predicate.language` is the same non-empty string; resolve that language via the existing `resolve_language` helper; call `create_predicate_group(&[sources])` — `Ok(None)` → `Ok(None)` (fallback); `Err(e)` → compile error WITHOUT inventing an index (the hook returns an unindexed `LanguageError`): when `e` is a parse/compile variant carrying the offending source text (e.g. `ParseError { expr, .. }`), match `expr` against the submitted scripts to locate the clause position and say so ("clause {i}: failed to compile {language} predicate `{source}`: {e}", same wording family as `compile_language_predicate`); for variants without source text, emit a group-level message naming the language and that one of the N `choice/when` clauses failed to compile — never fabricate a position. On success, build one `EvalMeta` per clause by calling the existing `eval_meta` helper with the UNCHANGED verb `"choice/when"` (this keeps `step_id = "choice/when#{step_index}"` identical to the sequential path), then override ONLY `meta.verb = format!("choice/when[{i}]")` per clause — the clause position lives in the verb, the shared outer step id is preserved.
2. In `control_flow.rs` `DeclarativeChoice` arm: call `compile_choice_group` first. `Some((group, metas))` → build `ChoiceGroupSegment::new(group, metas, bodies_in_clause_order, otherwise_seg)` where `bodies[i]` is exactly the composed segment `whens[i]` would have had on the sequential path (same `compile_children_segments` + `compose_outcome_segment` calls, evaluated for every clause — body compilation is unchanged). `None` → the existing per-clause loop, byte-for-byte unchanged. Programmatic `BuilderStep::Choice` arm: untouched. Filter/DoTry/split paths: untouched.
3. Mixed-language choices (clauses differing in `.language`) must hit the `None` arm automatically (no special-casing beyond the all-equal check).

Tests (command `cargo test -p camel-core --lib --features lang-rhai` — camel-core gates rhai behind the `lang-rhai` feature; every Rhai-using test is `#[cfg(feature = "lang-rhai")]`):
- Mock-based selection tests (no feature gate needed — use a test-local `CountingGroupLanguage` implementing `Language` whose `create_predicate_group` returns a counting `PredicateGroup` mock; register it in the test registry; the mock counts `create_predicate` and `create_predicate_group` calls and records `first_match` invocations):
  - `declarative_choice_hook_selected_for_group_capable_language` — 3 same-language whens under `CountingGroupLanguage`; compile; expect `create_predicate_group` called ONCE with all 3 sources, `create_predicate` NOT called for the clauses, and running one exchange invokes the group's `first_match` exactly once.
  - `declarative_choice_unsupported_hook_falls_back_to_sequential` — same-language whens under a mock whose `create_predicate_group` returns `Ok(None)`; expect per-clause `create_predicate` calls (sequential path), no group built.
  - `declarative_choice_single_clause_falls_back` — one when under `CountingGroupLanguage`; expect sequential (no `create_predicate_group` call).
  - `declarative_choice_mixed_languages_falls_back_to_sequential` — alternate `CountingGroupLanguage`/simple clauses; expect per-clause compilation, no group call.
- Rhai behavior tests (feature-gated, mirroring the existing `DeclarativeChoice` scaffolding at `step_resolution.rs:860-910`):
  - `declarative_choice_same_language_rhai_uses_group_path` — 3 rhai whens with distinct header-matching scripts; compile; run an exchange matching clause 1 through the compiled pipeline; expect clause 1's body effect AND (probe) clause 2's `throw` script never evaluated (no error surfaces).
  - `declarative_choice_group_compile_error_propagates` — rhai clause with a syntax error among valid same-language clauses; the compile error is a source-carrying parse variant, so expect the message to name the bad source AND the derived clause position (compile-time, before any exchange).
  - `declarative_choice_group_error_attributed_to_clause` — rhai clauses where clause 2 throws at runtime; run; expect `ExpressionFailed` whose `verb` is `choice/when[2]` AND whose `step_id` is the shared `choice/when#<step_index>` (identical to the sequential path's outer identity).

Acceptance:
- `cargo test -p camel-core --lib --features lang-rhai` passes (mock + rhai tests green; feature-gated tests compile).
- `cargo clippy -p camel-core --all-targets --features lang-rhai -- -D warnings` exits 0.
- `cargo build --workspace` (worktree) passes — confirms no other DeclarativeChoice consumer broke.

- [x] 473-t4

## Task 473-t5: grouped bench shapes + gate evidence

Files:
- `crates/camel-processor/benches/choice.rs` (modified)
- `benchmarks/records/<new-timestamp>/` (new)

Steps:
1. Extend the `choice_lang` bench group with grouped shapes built via `RhaiLanguage::create_predicate_group(&scripts)` + `ChoiceGroupSegment::new` (mock metas with verb `choice/when[i]`): `rhai_group_10_match_first`, `rhai_group_10_match_middle`, `rhai_group_10_match_last`, `rhai_group_10_no_match`, and control `rhai_group_1_match`. Scripts identical to the sequential shapes' (`header("match-<i>") == "yes"`).
2. Run TWO commands (criterion panics when `--baseline` names a shape with no saved baseline, so the new grouped shapes and the existing controls are measured separately):
   - Grouped shapes (fresh numbers, no baseline): `TMPDIR=/home/shared/tmp cargo bench -p camel-processor --bench choice -- "choice_lang/rhai_group"`.
   - Sequential controls vs baseline (anchored regex so the new `rhai_group_*` ids are NOT selected — they have no `choice-pre` baseline and criterion would panic): `TMPDIR=/home/shared/tmp cargo bench -p camel-processor --bench choice -- '^choice_lang/(rhai_1_match$|rhai_10_|mixed_10_no_match$)' --baseline choice-pre`. If sccache exits 254, `export RUSTC_WRAPPER=` and retry.
3. Gate check (from criterion output/estimates.json): every grouped 10-clause shape median <= 250 us (including no-match worst case); every sequential control median within criterion-reported noise of its `choice-pre` value. If the gate fails, STOP and report — do not tune the scripts to pass.
4. Record `benchmarks/records/<new-ts>/`: criterion estimates for the new grouped shapes + updated control estimates, and a `summary.md` following the Mission-457 convention (title "Mission 473 benchmark record — rhai choice batching grouped shapes + gate (bd rc-1y2qs)"; What this record is [mission-level Criterion record, no run.json, index.json not updated]; Exact commands; Results table shape|pre median|post median|delta; Gate section stating PASS/FAIL against <=250us per shape and control no-regression; Reading).

Tests: none (bench task); the executable gate evidence is the recorded estimates.

Acceptance:
- Gate: all grouped 10-clause medians <= 250 us (record PASS in summary.md).
- Sequential controls: no regression beyond criterion noise vs `choice-pre`.
- `cargo clippy -p camel-processor --all-targets -- -D warnings` exits 0.
- Record dir committed.

- [x] 473-t5

## Task 473-t6: docs — batching behavior + mixed fallback

Files:
- `docs/src/eip/content-based-router.md` (modified — the canonical Choice EIP page; verify with `ls docs/src/eip/` and grep for the when/otherwise section before editing; if the page does not exist under that exact name, locate the Choice page via `grep -rln "otherwise" docs/src --include='*.md'` and name it in the report)
- `docs/src/languages/rhai.md` (modified)
- `crates/languages/camel-language-api/CONTEXT.md` (modified, CONDITIONAL — only if its existing text already describes the predicate/expression surface; add one sentence naming `PredicateGroup`)
- `crates/camel-processor/CONTEXT.md` (modified, CONDITIONAL — only if its existing text already describes Choice/segment evaluation; add one sentence naming `ChoiceGroupSegment`)

Steps:
1. In `docs/src/eip/content-based-router.md`: add a short paragraph — when ALL `when` clauses use the same script language that supports group evaluation (rhai today), predicates are compiled and evaluated as one group per message (one snapshot, first-match short-circuit); mixed-language choices and single-clause choices keep per-clause sequential evaluation; behavior, ordering, and errors are identical from the route author's perspective; no configuration.
2. In `docs/src/languages/rhai.md`: add the same statement scoped to rhai (grouped evaluation, per-clause budgets and timeouts still apply per predicate, skipped clauses never run).
3. Apply the two conditional CONTEXT edits ONLY where the precondition holds; state explicitly in the report which applied and which were skipped (with the grep evidence).

Tests: `cd docs && mdbook test` passes (no broken links/code fences).

Acceptance:
- `mdbook test` passes.
- Both pages state batching + mixed fallback + "no user-facing config".
- `cargo xtask lint-context-citations` exits 0.

- [x] 473-t6

## Task 473-t7: workspace build + touched-crate test sweep + gates

Files: none new (verification task).

Steps:
1. `TMPDIR=/home/shared/tmp cargo build --workspace` in the worktree.
2. `TMPDIR=/home/shared/tmp cargo test -p camel-language-api -p camel-language-rhai -p camel-processor --lib` and `TMPDIR=/home/shared/tmp cargo test -p camel-core --lib --features lang-rhai`.
3. `cargo fmt --check --all`.
4. `cargo clippy -p camel-language-api -p camel-language-rhai -p camel-processor --all-targets -- -D warnings` and `cargo clippy -p camel-core --all-targets --features lang-rhai -- -D warnings`.
5. Docs build gate for touched crates: `RUSTDOCFLAGS="-D warnings" cargo doc -p camel-language-api -p camel-language-rhai -p camel-processor -p camel-core --no-deps`.
6. Xtask lint sweep on this change's surface: `cargo xtask lint-unwrap`, `cargo xtask lint-glob-imports`, `cargo xtask lint-missing-docs-ratchet`, `cargo xtask lint-secrets`, `cargo xtask lint-context-citations`.
7. `openspec validate --all --strict` from the worktree.

Tests: the seven command groups above are the executable checks.

Acceptance: all commands exit 0. Fix-forward any breakage discovered here (within this change's scope); report anything out of scope instead of expanding. The conductor re-runs the FULL AGENTS.md quality-gate registry at close; this sweep is the worker-level pre-gate.

- [x] 473-t7
