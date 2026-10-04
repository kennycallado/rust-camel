# Tasks: language-value-boundary

Phases mirror `design.md ## Phases`. Phase 1 is BREAKING (bd rc-16aft,
sealed Q1-Q5). Phase 2 = bd rc-33q5v + rc-7qv3r. Phase 3 = bd rc-qzd64.

Spec coverage note: tasks implement requirements E1-E7, V1-V3, V5, B1-B4,
P1-P2. Requirements V4, B5, O1, O2, O3 are contract canon implemented by
follow-up changes (ruling section 6, P2/P3) — no task owns them, by design.

Audit intake (mission 340, `.opencode/fleet/language-boundary-audit-matrix-20261002.md`):
- js zero-redaction is a LIVE credential leak today via `script:` (repro R5,
  `worker.rs:427-438` forwards the full Boa error Display). Its fix is INSIDE
  Phase 1 (task 1.8) alongside rhai redaction — E1 without E5 is a leak.
- simple coercion-error value embedding (`evaluator.rs:220-226`), jsonpath
  (`lib.rs:139`) and minijinja (`engine.rs:272-273`) Display passthrough are
  latent leaks activated by P1.1 — also fixed in task 1.8. xpath is already
  redaction-conformant except `warn!` level ownership (task 1.8).
- Per-language follow-ups OUT OF SCOPE for this change (matrix section 3,
  findings 3/4/7/8/10; tracked as bd follow-ups): simple B1/V1 body
  stringification + lossy bytes + stream-null (`evaluator.rs:23-30`); xpath
  V1 nodeset flattening + V2 Inf/NaN-to-Null (`lib.rs:152-169`); js B3
  wholesale header/property rewrite (`expression.rs:60-62`); js B1 Bytes body
  to null (`expression.rs:48-49`); simple parser leniency for unterminated
  `${` (`parser.rs:135-140`); jsonpath single-result unwrap ambiguity
  (JPT-004). These land with the P2 conformance-kit changes (R2/R3/R4
  flip-tests seed it).
- Audit repros R1 (jsonpath filter swallow) and R5 (js live leak) verify
  Phase 1 behavior and join the regression suite (task 3.3). R2 (xpath
  Inf-to-Null) requires the DEFERRED xpath V2 refusal — it moves to the xpath
  follow-up change with R3/R4 as conformance-kit seeds.

## Phase 1: fallible expression glue + redaction + strict predicates

### camel-api

#### Task 1.1: typed evaluation error `ExpressionFailed` + class enum
**Files:**
- `crates/camel-api/src/error.rs` (modified)
- `crates/camel-api/src/lib.rs` (modified, re-export if error types live in a module)

**Steps:**
1. Add `pub enum ExpressionErrorClass { Runtime, Arithmetic, TypeMismatch, FunctionNotFound, Limit, Timeout, Conversion, Parse }` with `Display` (lowercase kebab strings: `runtime`, `arithmetic`, `type-mismatch`, `function-not-found`, `limit`, `timeout`, `conversion`, `parse`).
2. Add `#[derive(Clone, Copy, Debug, PartialEq, Eq)] pub struct ErrorPosition { pub line: u32, pub column: u32 }` with `Display` as `{line}:{column}`.
3. Add `pub struct ConversionDetail { pub source_type: String, pub target: String }` (both are TYPE NAMES / trusted config strings, never runtime values). Add variant `CamelError::ExpressionFailed { language: String, route_id: String, step_id: String, verb: String, class: ExpressionErrorClass, position: Option<ErrorPosition>, conversion: Option<ConversionDetail>, cause: Option<Box<CamelError>> }`. `thiserror` Display: `expression failed: {class} in {language} `{verb}` step `{step_id}` (route `{route_id}`)` plus ` at {position}` when present, plus ` while converting {source_type} to {target}` when `conversion` is set. No exchange data in any field or in Display. Wire `cause` as `#[source]`.
4. Extend `variant_name()` (error.rs:380) with `"ExpressionFailed"`; keep the exhaustiveness test (`variant_name_covers_all_variants`) green.
5. `CamelError` carries a manual `Clone` impl that cannot derive through `Box<CamelError>` recursion: extend it so `ExpressionFailed` clones deeply (`cause` boxed value cloned). Keep sibling variants' existing Clone behavior and redaction-safe `Debug` conventions.

**Tests:**
- `expression_failed_display_has_no_exchange_data`: construct `ExpressionFailed` with class `Arithmetic`, position `3:8` → render `to_string()` → assert it contains `arithmetic`, `rhai`, `set_property`, `3:8` and contains no payload field (nothing beyond metadata fields).
- `expression_failed_variant_name`: `CamelError::ExpressionFailed{..}.variant_name() == "ExpressionFailed"`.
- Command: `cargo test -p camel-api --lib expression_failed` — expected: pass after implementation, fail before (variant does not exist).

**Acceptance:**
- `cargo clippy -p camel-api -- -D warnings` exits 0.
- `cargo test -p camel-api --lib` passes.

- [x] 1.1

### camel-api (sources)

#### Task 1.2: fallible source enums for predicates and values
**Files:**
- `crates/camel-api/src/filter.rs` (modified)
- `crates/camel-api/src/lib.rs` (modified, re-exports)

**Steps:**
1. Add `pub type BoxValueFuture = std::pin::Pin<Box<dyn std::future::Future<Output = Result<Value, CamelError>> + Send>>;` and `pub type BoxBoolFuture = std::pin::Pin<Box<dyn std::future::Future<Output = Result<bool, CamelError>> + Send>>;` in `filter.rs` (or a sibling module if cleaner; keep re-exported at crate root).
2. Add `#[derive(Clone)] pub enum PredicateSource { Sync(FilterPredicate), Async(Arc<dyn Fn(&Exchange) -> BoxBoolFuture + Send + Sync>) }` with `pub async fn matches(&self, exchange: &Exchange) -> Result<bool, CamelError>` (Sync arm wraps the bool in `Ok`).
3. Add `#[derive(Clone)] pub enum ValueSource { Sync(Arc<dyn Fn(&Exchange) -> Value + Send + Sync>), Async(Arc<dyn Fn(&Exchange) -> BoxValueFuture + Send + Sync>) }` with `pub async fn evaluate(&self, exchange: &Exchange) -> Result<Value, CamelError>`.
4. Add `#[derive(Clone)] pub enum TargetSource { Sync(Arc<dyn Fn(&Exchange) -> Option<String> + Send + Sync>), Async(Arc<dyn Fn(&Exchange) -> BoxValueFuture + Send + Sync>) }` with `pub async fn resolve(&self, exchange: &Exchange) -> Result<Option<String>, CamelError>` (Async arm coerces: `Value::Null` → `None`, `Value::String(s)` → `Some(s)`, other scalars → `Some(v.to_string())`, arrays/objects → `Err(CamelError::ProcessorError("router target expression returned a non-scalar value (array/object); expected a string target"))`. Add `#[derive(Clone)] pub enum RecipientSource { Sync(Arc<dyn Fn(&Exchange) -> String + Send + Sync>), Async(Arc<dyn Fn(&Exchange) -> BoxValueFuture + Send + Sync>) }` with `pub async fn resolve(&self, exchange: &Exchange) -> Result<String, CamelError>` (same coercion, empty string allowed).

**Tests:**
- `predicate_source_sync_returns_bool` / `predicate_source_async_propagates_error`: Sync arm returns true; Async arm returns future resolving `Err(ExpressionFailed)` → `matches()` yields the error (not `false`).
- `value_source_async_propagates_error`: Async arm yields `Err` → `evaluate()` yields `Err` (not `Null`).
- `target_source_null_maps_to_none_and_error_propagates`: Async arm `Ok(Value::Null)` → `None`; Async arm `Err(..)` → `Err(..)`.
- `target_source_array_is_error`: Async arm `Ok(Value::Array(..))` → `Err(ProcessorError(..))` naming the non-scalar target.
- Command: `cargo test -p camel-api --lib source` — expected: fail before (types absent), pass after.

**Acceptance:**
- `cargo clippy -p camel-api -- -D warnings` exits 0.
- `cargo test -p camel-api --lib` passes.

- [x] 1.2

### camel-language-api

#### Task 1.3: structured error transport + evaluation carriers
**Files:**
- `crates/languages/camel-language-api/src/error.rs` (modified)
- `crates/languages/camel-language-api/src/eval.rs` (new)
- `crates/languages/camel-language-api/src/lib.rs` (modified, `pub mod eval;` + re-exports)

**Steps:**
1. Extend `LanguageError` with three structured variants: `EvalFailure { class: ExpressionErrorClass, position: Option<ErrorPosition>, detail: Option<String> }` (detail MUST already be redacted by the emitting crate), `TypeMismatch { expected: String, actual: String, position: Option<ErrorPosition> }`, `ConversionError { source_type: String, target: String }`. Keep existing variants. Add methods `pub fn class(&self) -> Option<ExpressionErrorClass>` and `pub fn position(&self) -> Option<ErrorPosition>` covering all variants (ParseError → `Parse`, EvalError → `Runtime`, others as appropriate; unknown → None).
2. In `eval.rs` add `pub struct EvalMeta { pub language: String, pub route_id: String, pub step_id: String, pub verb: String, pub target: Option<String> }` — `target` is the TRUSTED compile-time destination (property/header key or `body`; operator config per ADR-0032). It never contains runtime exchange data, and nested value keys inside a map are NOT appended to it.
3. Add `pub struct LanguageExpressionEval { expr: Arc<dyn Expression>, meta: EvalMeta }` with `new(expr, meta)`, `pub fn meta(&self) -> &EvalMeta`, `pub async fn evaluate(&self, exchange: &Exchange) -> Result<Value, CamelError>` (maps `LanguageError` via `to_expression_failed`), and `pub fn into_value_fn(self) -> Arc<dyn Fn(&Exchange) -> camel_api::BoxValueFuture + Send + Sync>`.
4. Add `pub struct LanguagePredicateEval { pred: Arc<dyn Predicate>, meta: EvalMeta }` with `new`, `meta()`, `pub async fn matches(&self, exchange: &Exchange) -> Result<bool, CamelError>`, and `pub fn into_bool_fn(self) -> Arc<dyn Fn(&Exchange) -> camel_api::BoxBoolFuture + Send + Sync>`.
5. Add `pub fn to_expression_failed(err: LanguageError, meta: &EvalMeta) -> CamelError` building `CamelError::ExpressionFailed` from `err.class()`/`err.position()` (defaults: class `Runtime`, position `None`). When `err` is `ConversionError { source_type, target }` and the target is the rhai generic placeholder (`"value"`) or otherwise less specific, REWRITE the target from the trusted `meta.target` (e.g. `property m`, `header x`, `body`) so route-level diagnostics name the real destination; populate the `conversion: Some(ConversionDetail{..})` field. Runtime-derived strings (script map keys, exchange header keys) are never written into `ConversionDetail`.

**Tests:**
- `to_expression_failed_maps_class_and_position`: `LanguageError::EvalFailure{class: Arithmetic, position: Some(ErrorPosition{line:3,column:8}), detail:None}` + meta{language:"rhai", verb:"set_property", ..} → assert `matches!(err, CamelError::ExpressionFailed{class: ExpressionErrorClass::Arithmetic, position: Some(p) if p.line==3, ..})`.
- `to_expression_failed_defaults_eval_error_to_runtime`: `LanguageError::EvalError("x".into())` → class `Runtime`, position `None`.
- `language_expression_eval_wraps_error`: hand-rolled `Expression` impl returning `Err(EvalFailure{..})` → `LanguageExpressionEval::evaluate` returns `Err(ExpressionFailed{..})` carrying meta verb.
- Command: `cargo test -p camel-language-api --lib` — expected: pass after 1.1+1.3 types exist.

**Acceptance:**
- `cargo clippy -p camel-language-api -- -D warnings` exits 0.
- `cargo test -p camel-language-api --lib` passes.

- [x] 1.3

### camel-processor

#### Task 1.4: fallible predicate path in step segments
**Files:**
- `crates/camel-processor/src/filter.rs` (modified)
- `crates/camel-processor/src/choice.rs` (modified)
- `crates/camel-processor/src/do_try.rs` (modified)
- `crates/camel-processor/src/do_try_segment.rs` (modified)
- `crates/camel-api/src/loop_eip.rs` (modified, `LoopMode` lives here)
- `crates/camel-processor/src/validate.rs` (modified, `ValidateService`/`from_predicate` live here)
- `crates/camel-builder/src/do_try.rs` (modified)
- `crates/camel-builder/src/lib.rs` (modified)
- `crates/camel-processor/src/loop_eip.rs` (modified, loop service consuming `LoopMode::While`)
- any remaining `FilterPredicate` holders in camel-processor (audit `grep -rn "FilterPredicate" crates/camel-processor/src/` and migrate every hit in this task)

**Steps:**
1. Change `FilterSegment { predicate }`, `WhenClauseSegment { predicate }`, `CatchMatcher::Predicate(..)`, `FinallyClauseSegment { on_when }`, `LoopMode::While(..)` (camel-api `loop_eip.rs:10`) from `FilterPredicate` to `camel_api::PredicateSource`. Update constructors (`FilterSegment` construction sites, `ValidateService::from_predicate` in `validate.rs:13,31`) to take `PredicateSource`.
1b. Update camel-builder call sites that construct these segments with plain `FilterPredicate`s (`do_try.rs:85-88` `do_catch_when`, `lib.rs:691,1650` `LoopMode::While(FilterPredicate::new(..))`) to wrap in `PredicateSource::Sync(..)`.
2. Restructure each service's async evaluation: `Service::call` is sync — no `.await` inside the call body. Return one owned boxed future that awaits `predicate_source.matches(&exchange)` and, on error, produces that segment's failure transport: outcome-composed segments (filter, choice, do_try arms) yield `PipelineOutcome::Failed(err)` per `segment_outcome_composition` (follow the existing throw-style failure path in those segments); plain Tower services yield `Err(err)`. Semantics: filter error no longer drops the exchange; choice/when error fails instead of falling to `otherwise`; loop-while error ends the loop with failure, not Ok; validate surfaces the typed error, not `ValidationError`; finally `on_when` error fails the segment. Preserve Tower readiness: `poll_ready` keeps delegating to inner services.
3. Make `CatchMatcher::matches` async and fallible over `PredicateSource`, and migrate EVERY on_when/finally evaluation site: catch `when`, catch `on_when`, finally `on_when`, plus legacy field shapes in `do_try_segment.rs` (audit both do_try files for `FilterPredicate`-typed fields and retype them).
3b. In the do_try path, BOTH predicate routes chain the original error: a failed catch `when` predicate OR a failed catch `on_when` predicate replaces the original error and preserves it as cause. Predicate errors from `LanguagePredicateEval` are always `CamelError::ExpressionFailed` with `cause: None`; copy the error and set `cause: Some(Box::new(original_error))`, all other fields unchanged.
4. Keep existing Sync behavior: programmatic `FilterPredicate` closures still construct via `PredicateSource::Sync`.

**Tests:**
- `filter_segment_propagates_predicate_error`: `PredicateSource::Async` returning `Err(ExpressionFailed{..})` → the outcome segment yields `PipelineOutcome::Failed(..)` carrying that error (assert the Failed variant explicitly), exchange not dropped silently.
- `choice_when_predicate_error_fails_step`: when-clause Async predicate `Err(ExpressionFailed{..})` → the choice outcome segment yields `PipelineOutcome::Failed(ExpressionFailed{..})` (assert the Failed variant explicitly) and the `otherwise` branch never executes.
- `loop_while_predicate_error_fails_loop`: `LoopMode::While` Async predicate `Err` → loop service returns `Err` (not loop-end Ok).
- `validate_predicate_error_is_typed_not_validation`: `ValidateService` with Async predicate `Err(ExpressionFailed)` → returned error is `ExpressionFailed`, not `ValidationError`.
- `catch_when_predicate_error_chains_original`: inner step fails with `ProcessorError("boom")`, catch `PredicateSource::Async` → `Err`; assert result error is `ExpressionFailed` whose `cause` Display contains `boom`.
- Command: `cargo test -p camel-processor --lib` — expected: new tests fail before segment changes, pass after.

**Acceptance:**
- `cargo clippy -p camel-processor -p camel-api -- -D warnings` exits 0.
- `cargo test -p camel-processor --lib` passes.

NOTE (shared compile checkpoint, tasks 1.4-1.6): re-typing shared segment
types breaks camel-core compilation until task 1.6 rewires it — and
camel-builder transitively (it depends on camel-core). Per-task gates are
STRICTLY crate-local (camel-processor + camel-api here); `cargo check
--workspace` AND `cargo clippy -p camel-builder -- -D warnings` AND
`cargo test -p camel-builder --lib` are verified once, at the task 1.6
checkpoint.

- [x] 1.4

#### Task 1.5: fallible expression path in dynamic setters + carrier configs
**Files:**
- `crates/camel-processor/src/dynamic_set_property.rs` (modified)
- `crates/camel-processor/src/dynamic_set_header.rs` (modified)
- `crates/camel-processor/src/set_body.rs` (modified)
- `crates/camel-processor/src/script_mutator.rs` (modified)
- `crates/camel-processor/src/lib.rs` (modified, re-exports)
- `crates/camel-api/src/dynamic_router.rs` (modified, `DynamicRouterConfig` at :9)
- `crates/camel-api/src/routing_slip.rs` (modified, `RoutingSlipConfig` at :8)
- `crates/camel-api/src/recipient_list.rs` (modified, `RecipientListConfig` at :17)
- `crates/camel-processor/src/dynamic_router.rs` (modified)
- `crates/camel-processor/src/routing_slip.rs` (modified)
- `crates/camel-processor/src/recipient_list.rs` (modified)
- `crates/camel-processor/src/idempotent_consumer.rs` (modified)
- `crates/camel-processor/src/claim_check.rs` (modified)
- `crates/camel-processor/src/sort.rs` (modified, `SortExpression` alias lives at sort.rs:59)
- `crates/camel-processor/src/log.rs` (modified, `DynamicLog`)
- `crates/camel-processor/src/cache_eip.rs` (modified, `MessageIdExpression` consumers at :174,193,638,640,812,821)

**Steps:**
1. `DynamicSetProperty`/`DynamicSetHeader`/`DynamicSetHeaderIfAbsent`: replace the generic `F: Fn(&Exchange) -> Value` field with `camel_api::ValueSource`; keep constructor `new(inner, key, value_source)` accepting `impl Into<ValueSource>` with `From<Arc<dyn Fn(&Exchange) -> Value + Send + Sync>>` and `From<Arc<dyn Fn(&Exchange) -> BoxValueFuture + Send + Sync>>`. Restructure `Service::call` (sync — no `.await` in the call body): return ONE owned boxed future moving the needed state (value source clone, key clone, exchange) into the future, which (a) awaits `value_source.evaluate(&exchange)`, (b) on `Err` yields `Err(e)` with NO mutation applied (property/header NOT set, inner NOT called), (c) on `Ok(v)` applies the mutation then awaits `inner.call(exchange)` inside the same future. The future OWNS everything it touches (value-source clone, key clone, the exchange) — nothing borrows from `self`. Invoking inner follows the clone-and-replace pattern: `let mut inner = core::mem::replace(&mut self.inner, pending_inner)` (or the crate's established equivalent) so the async context holds an owned service; `poll_ready` still delegates to `self.inner` and the swap is readiness-compatible (filter/setters are ready when inner is ready). `DynamicSetHeaderIfAbsent` runs its presence check BEFORE evaluation: when the header is already present the expression is NOT evaluated at all (an erroring expression on the absent path still errors; the present path skips evaluation — preserving today's semantics). Update the `*Layer` companions and crate re-exports (`crates/camel-processor/src/lib.rs`) in the same commit, and migrate the crates' existing unit tests to the new constructors.
2. `SetBody` dynamic form: same treatment; on `Ok(v)` map through the existing `value_to_body` (Null→Empty, String→Text, other→Json).
2b. `DynamicLog` (log.rs): replace the sync closure field with `camel_api::ValueSource`; on `Err` return the error BEFORE logging (a failed log-message expression fails the step; it does not log `Null`).
3. `ScriptMutator`: add `pub fn with_meta(expression: Box<dyn MutatingExpression>, meta: camel_language_api::EvalMeta) -> Self` storing meta; replace `language_err_to_camel` with `camel_language_api::to_expression_failed(e, &meta)` for the mutating-eval error path (keeps `ParseError` nuance: route through `to_expression_failed` as class `Parse`). Existing `new` (no meta) builds a default meta `{language:"unknown"}` — used only by tests.
4. `DynamicRouterConfig`, `RoutingSlipConfig`: closure field type → `camel_api::TargetSource` (constructors take `impl Into<TargetSource>`; provide `From` for the existing sync closure shape). `RecipientListConfig`: → `RecipientSource`. Services evaluate `resolve(&exchange).await` and propagate `Err`.
5. Sort: replace `SortExpression` alias with `#[derive(Clone)] pub enum SortKeySource { Sync(Arc<dyn Fn(&serde_json::Value) -> Result<SortKey, CamelError> + Send + Sync>), Async(Arc<dyn Fn(&serde_json::Value) -> BoxValueFuture + Send + Sync>) }` + `pub async fn key(&self, value) -> Result<SortKey, CamelError>` (Async arm keeps the non-scalar rejection: Array/Object → `Err(ProcessorError("sort expression returned a non-scalar value (array/object); expected null/bool/number/string"))`). Update the sort/resequencer service to await it and propagate.
6. `MessageIdExpression` → `pub enum MessageIdSource { Sync(Arc<dyn Fn(&Exchange) -> Option<String> + Send + Sync>), Async(Arc<dyn Fn(&Exchange) -> BoxValueFuture + Send + Sync>) }`. Consumers to update: `idempotent_consumer.rs`, `claim_check.rs`, and `cache_eip.rs` (`CacheInvalidateTarget::{Key,Prefix}`, `CachePeekStaleService`) whose key expressions are compiled via `compile_message_id_expression`.
7. Split source: camel-api `splitter.rs` already defines `pub type SplitExpression = Arc<dyn Fn(&Exchange) -> Result<Vec<Exchange>, CamelError> ...>` (see splitter.rs:16 and the `Arc::new(|_| Ok(Vec::new())) as SplitExpression` test at :685). Add `#[derive(Clone)] pub enum SplitSource { Sync(SplitExpression), Async(Arc<dyn Fn(&Exchange) -> BoxValueFuture + Send + Sync>) }` with `pub async fn split(&self, ex: &Exchange) -> Result<Vec<Exchange>, CamelError>`: Sync arm invokes the existing splitter; Async arm AWAITS the language evaluation (`Result<Value, CamelError>`) and only on success derives fragments with today's rules (string → non-empty line fragments; JSON array → per-element fragments — the rules currently inlined in the splitting.rs closure at :148-160, moved into the arm implementation). Evaluation errors propagate BEFORE any fragment is produced. Re-type `SplitterConfig.expression` (splitter.rs:217) to `SplitSource` and migrate the split segment/service in camel-processor (`grep -rn "SplitterConfig" crates/camel-processor/src/`) to await it. Built-in splitters (`split_body_lines`, `split_body_json_array`, `split_body`) keep compiling through the Sync arm unchanged. + `pub async fn message_id(&self, ex) -> Result<Option<String>, CamelError>` (Async arm: Null/empty string → None, String → Some, other scalars → Some(to_string)). `KeyExpression` → `pub enum ClaimKeySource { Sync(Arc<dyn Fn(&Exchange) -> Result<String, CamelError> + Send + Sync>), Async(Arc<dyn Fn(&Exchange) -> BoxValueFuture + Send + Sync>) }` + `pub async fn key(&self, ex) -> Result<String, CamelError>` (Null/empty → `Err(ValidationError("claim_check key expression evaluated to null or empty"))` kept). Services propagate evaluation errors before any registry mutation.

**Tests:**
- `dynamic_set_property_error_sets_nothing`: ValueSource Async `Err` → service returns Err; exchange properties unchanged.
- `dynamic_set_header_if_absent_present_header_skips_evaluation`: header already present + Async value source that returns `Err` if evaluated → service SUCCEEDS (presence check first), header value untouched.
- `setter_works_on_current_thread_runtime`: `#[tokio::test(flavor = "current_thread")]` runs a `DynamicSetProperty` Async evaluation to completion — no `block_in_place` panic (regression for the removed bridge).
- `setter_poll_ready_delegates_to_inner`: readiness-sensitive test — an inner service that is pending once then ready produces correct sequencing (evaluation happens only after readiness).
- `dynamic_set_header_error_propagates` / `dynamic_set_header_if_absent_error_propagates`: same shape.
- `set_body_dynamic_error_keeps_body`: Async Err → Err and body unchanged.
- `script_mutator_error_is_expression_failed`: MutatingExpression returning `Err(EvalFailure{..})` + meta{language:"rhai", verb:"script"} → Err is `ExpressionFailed` with language "rhai".
- `dynamic_router_target_error_fails_step` / `routing_slip_target_error_fails_step` / `recipient_list_error_fails_step`: Async Err → service Err (not silent end / empty recipient list).
- `dynamic_log_error_fails_step`: `DynamicLog` with Async Err value source → service Err (no log record emitted with a null message).
- `sort_async_error_propagates` / `message_id_async_error_propagates` / `claim_key_async_error_propagates`: Async Err → Err reaches caller (idempotent consumer does not treat as absent id; claim check does not emit ValidationError-with-wrong-cause).
- `split_sync_builtin_still_works`: `SplitSource::Sync(split_body_lines())` on a two-line body → two fragments (regression: built-ins unchanged).
- `split_async_error_propagates_before_fragments`: `SplitSource::Async` returning `Err(ExpressionFailed)` → split service yields `Err`, zero fragments produced.
- Command: `cargo test -p camel-processor --lib` — expected: new tests red before, green after.

**Acceptance:**
- `cargo clippy -p camel-processor -- -D warnings` exits 0.
- `cargo test -p camel-processor --lib` passes.
- `grep -rn "block_in_place\|block_on" crates/camel-processor/src/dynamic_set_property.rs crates/camel-processor/src/dynamic_set_header.rs crates/camel-processor/src/set_body.rs crates/camel-processor/src/log.rs crates/camel-processor/src/sort.rs crates/camel-processor/src/cache_eip.rs crates/camel-processor/src/idempotent_consumer.rs crates/camel-processor/src/claim_check.rs` returns 0 hits (test-local runtimes elsewhere, e.g. do_try.rs:810, are out of scope).
- `cargo test -p camel-processor --lib log` covers the DynamicLog error path.

- [x] 1.5

### camel-core

#### Task 1.6: delete `await_eval`/`await_matches`; wire carriers into all compiler sites
**Files:**
- `crates/camel-core/src/lifecycle/adapters/step_resolution.rs` (modified)
- `crates/camel-core/src/lifecycle/adapters/step_compilers/core.rs` (modified)
- `crates/camel-core/src/lifecycle/adapters/step_compilers/control_flow.rs` (modified)
- `crates/camel-core/src/lifecycle/adapters/step_compilers/splitting.rs` (modified)
- `crates/camel-core/src/lifecycle/adapters/step_compilers/routing.rs` (modified)

**Steps:**
1. Delete `await_eval` and `await_matches` from `step_resolution.rs` entirely (removes `block_in_place`).
2. Add `fn eval_meta(ctx, language: &str, verb: &str, index: usize, target: Option<String>) -> EvalMeta` helper: `route_id` from `ctx.route_id.unwrap_or("(unassigned)")`, `step_id` = `format!("{verb}#{index}")`. Thread the step ordinal from `compile_steps` into every compiler arm — the index is REQUIRED (no verb-only identification fallback; extend the compile loop to pass it if missing). `target` carries the property/header key or `"body"` where the verb has one.
3. `compile_filter_predicate` → returns `camel_api::PredicateSource::Async(pred.into_bool_fn())` using `LanguagePredicateEval` + meta from a new parameter (thread verb/index through existing call sites: `DeclarativeFilter`, `DeclarativeChoice`, `DeclarativeLoop` while, `Validate`, do_try catch `when`/`on_when`, finally `on_when`).
4. `compile_sort_expression` → `SortKeySource::Async` (wrap `LanguageExpressionEval::into_value_fn` over the per-element exchange). `compile_message_id_expression` → `MessageIdSource::Async`. `compile_key_expression` → `ClaimKeySource::Async`.
5. `core.rs`: set_header (:361), set_header_if_absent (:407), set_property (:439) → `DynamicSet*::new(inner, key, LanguageExpressionEval into_value_fn + meta)`. set_body dynamic (:475) and script non-mutating fallback (:509) → `ValueSource::Async`. `ValidateService::from_predicate` → `PredicateSource` from step 3. The `DeclarativeLog` Expression arm (core.rs:312-330) inlines its own `block_in_place`+`block_on`+`unwrap_or_else(|e| { warn!(..); Value::Null })` — replace it with `DynamicLog` fed by `ValueSource::Async` from `LanguageExpressionEval` (verb `log`), deleting the inline bridge and the `warn!`. The cache compile arms (`CacheInvalidateTarget::{Key,Prefix}`, `CachePeekStaleService` key) switch to the new `MessageIdSource::Async` from step 4 of task 1.5.
6. `control_flow.rs`: all `compile_filter_predicate` call sites pass through the new signature (verb strings: `filter`, `choice/when`, `loop while`, `catch when`, `catch on_when`, `finally on_when`).
7. `splitting.rs` (:150) and `routing.rs` (:61, :120, :180): replace closures with `SplitSource`/`TargetSource`/`RecipientSource` Async arms carrying meta (verbs: `split`, `dynamic_router`, `routing_slip`, `recipient_list`).
8. Script verb (`DeclarativeScript` mutating path in core.rs:509 region): construct `ScriptMutator::with_meta(expr, meta)` with verb `script`. The read-only fallback arm (core.rs:503-511) for languages without `MutatingExpression` uses `ValueSource::Async` and propagates errors like every other verb.
9. Shared checkpoint gate: after this task the whole workspace compiles — run `cargo check --workspace` AND `cargo check -p camel-core --features lang-rhai,lang-jsonpath,lang-js,lang-xpath` (feature-gated code compiles too; Simple is UNCONDITIONAL — there is no `lang-simple` feature) plus `cargo clippy -p camel-builder -- -D warnings` and `cargo test -p camel-builder --lib` before declaring 1.6 done.

**Tests:**
- `no_await_eval_remains`: compile-time guarantee via `grep -rn "await_eval\|await_matches" crates/camel-core/src/` returning 0 hits (assert as acceptance, not a Rust test).
- `set_property_expression_error_fails_step` (camel-core integration test, rhai): route `direct:a` → `set_property {name: x, rhai: '"no-es-un-numero".parse_float()'}` → `to: direct:b`; send exchange; assert consumer receives `Err` whose variant is `ExpressionFailed` and property `x` absent.
- `filter_predicate_error_propagates`: same with `filter` + failing predicate → Err (exchange not silently dropped).
- Command: `cargo test -p camel-core --lib --features lang-rhai,lang-jsonpath,lang-js,lang-xpath && cargo test -p camel-core --test expression_failure_propagation_test --features lang-rhai,lang-jsonpath,lang-js,lang-xpath` (note: the --test expression_failure_propagation_test target is authored in task 1.9; task 1.6's coverage ran via the --lib half — tests live in route_compiler_tests::expression_error_tests) — expected: red before wiring, green after (run BOTH targets explicitly; the feature set is verified against `cargo metadata` first — if a feature name differs, fix the command, never drop a language).

**Acceptance:**
- `grep -rn "await_eval\|await_matches" crates/ --include="*.rs"` returns zero hits (the functions no longer exist).
- `grep -rn "block_in_place\|block_on" crates/camel-core/src/lifecycle/adapters/step_resolution.rs crates/camel-core/src/lifecycle/adapters/step_compilers/` returns zero hits (the DeclarativeLog inline bridge is gone; test-local runtimes such as body_coercing.rs:153 and do_try.rs:810 are the only permitted remaining hits and are untouched).
- `cargo clippy -p camel-core -- -D warnings` exits 0.
- `cargo test -p camel-core --lib` passes.

- [x] 1.6

### camel-language-rhai

#### Task 1.7: rhai redaction, strict bool, log levels, read-only mutation rejection
**Files:**
- `crates/languages/camel-language-rhai/src/lib.rs` (modified)

**Steps:**
1. Error mapping (lib.rs:314-332 region and mutating path :369-386): map `rhai::EvalAltResult` to `LanguageError::EvalFailure { class, position, detail }`. Add `fn eval_alt_class(e: &rhai::EvalAltResult) -> ExpressionErrorClass`: `ErrorArithmetic`→Arithmetic, `ErrorMismatchOutputType|ErrorMismatchDataType`→TypeMismatch, `ErrorFunctionNotFound`→FunctionNotFound, `ErrorTooManyOperations|ErrorTooManyModules|ErrorStackOverflow|ErrorDataTooLarge`→Limit, `ErrorTerminated` (timeout token)→Timeout, `ErrorParsing`→Parse, `ErrorRuntime`→Runtime, everything else→Runtime. The timeout wrappers in `eval_async` (tokio `Elapsed` → currently plain `EvalError("rhai execution timeout")`) and the `spawn_blocking` join error map to `EvalFailure { class: Timeout, .. }` and `{ class: Runtime, .. }` respectively — no class may collapse to plain EvalError. Classify the DEEPEST error via `e.unwrap_inner()` before mapping (wrapped `ErrorInExpr`/nested variants classify by their innermost kind, not Runtime). Position: use the deepest available `position()` (inner first, outer as fallback). Add `ErrorTooManyVariables` to the Limit set. `detail` is `None` for value-bearing kinds (`ErrorArithmetic`, `ErrorRuntime`, `ErrorMismatchDataType`, `ErrorIndexingType`, `ErrorPropertyNotFound`); for operand-free kinds (`ErrorFunctionNotFound`, limit/timeout kinds) a short static string without operands is allowed. STREAM MARKER RULE: the `StreamBodyRef`-to-conversion mapping (task 2.2) applies ONLY to the dedicated guard sentinel and the type-signature/type-list fields of `ErrorFunctionNotFound` and `ErrorIndexingType` — never to `ErrorRuntime` payloads or free Display text (a `throw "StreamBodyRef"` script stays class `runtime`). Never include `e` Display for value-bearing kinds. Delete the `warn!` lines (ADR-0012).
2. Strict bool (`RhaiPredicate::matches` lib.rs:528-532): `Value::Bool(b) => Ok(b)`; any other `Value` (including `Null`) → `Err(LanguageError::TypeMismatch { expected: "bool".to_string(), actual: value_type_name(&val).to_string(), position: None })`. Add `fn value_type_name(v: &Value) -> &'static str` returning "string" | "number" | "bool" | "null" | "array" | "object".
3. Read-only mutation rejection (B4): enable the `internals` feature on the `rhai` dependency in `crates/languages/camel-language-rhai/Cargo.toml` (AST walking requires it). In `create_expression` and `create_predicate`, use a TWO-AST discipline: (1) compile a WALK-AST with `OptimizationLevel::None` (walking unoptimized ASTs preserves call expressions the optimizer may fold) and walk all statements/expressions recursively (`AST` statement iteration; recurse into `Stmt`/`Expr` enums, including nested functions/blocks/if/switch/loop bodies) and reject any function call whose callee name is `set_property` or `set_header` with `LanguageError::ParseError { expr: <source>, reason: "set_property()/set_header() cannot be used in a read-only expression; use a script: step instead" }`. Also stop registering `set_property`/`set_header` functions on the read-only eval engine so a dynamic call cannot slip through. (2) The EXECUTION-AST is a SECOND compilation at `OptimizationLevel::Simple` (explicit, both modes — task 2.2's discarded-statement exemption depends on it: a bare `body;` statement clones the marker under `None` (spike: reads=1) but is optimized away under `Simple` (reads=0)). Both ASTs are built once at compile time and cached. The mutating path (`create_mutating_expression`) does NOT run the walk (execution AST still Simple).
4. Keep `throw` redaction behavior (ErrorRuntime detail stays None).
5. Audit the compile-failure `warn!` records (lib.rs:619, 637, 662 region): compile failures already surface as `CamelError::RouteError` at route add; downgrade these records to `debug!` (same ADR-0012 reasoning).

**Tests:**
- `parse_float_error_is_class_and_position_only`: eval `"SECRET".parse_float()` → `Err(EvalFailure{class: Arithmetic, ..})`; the full error chain rendered (`{:?}` + Display + `to_expression_failed` Display) contains no `SECRET`.
- `predicate_non_bool_is_type_mismatch`: predicate script `"false"` (string literal) → `Err(TypeMismatch { expected: "bool", .. })`; predicate `42` → same; predicate `true` → Ok(true).
- `predicate_null_is_type_mismatch`: predicate `()` → `Err(TypeMismatch)` (not coerced false).
- `read_only_set_property_is_compile_error`: `create_expression("set_property(\"k\", 1)")` → `Err(ParseError{..})` whose reason contains `script:`; same for `set_header`.
- `limit_error_class_is_limit_not_runtime`: script `loop { }` with a small `max_operations` limit → `Err(EvalFailure{class: Limit, ..})`.
- `wrapped_error_in_function_classifies_inner`: `fn f() { "no".parse_float(); } f()` → class Arithmetic (not Runtime) with the innermost position.
- `throw_text_mentioning_marker_stays_runtime`: script `throw "StreamBodyRef"` → class `runtime`, fully redacted (NOT conversion; payload text is never matched).
- `timeout_maps_to_timeout_class`: expression exceeding a tiny `execution_timeout_ms` (no `std::thread::sleep` — use `loop {}` guarded by the timeout) → `Err(EvalFailure{class: Timeout, ..})`.
- `nested_read_only_setter_is_compile_error`: `if true { set_property(\"k\", 1) }` → rejected (walk recurses).
- `throw_still_redacted`: `throw "SECRET"` → no `SECRET` anywhere in error output.
- Command: `cargo test -p camel-language-rhai --lib` — expected red before, green after.

**Acceptance:**
- `cargo clippy -p camel-language-rhai -- -D warnings` exits 0.
- `cargo test -p camel-language-rhai --lib` passes.
- `grep -n "warn!" crates/languages/camel-language-rhai/src/lib.rs` returns 0 hits (evaluation AND compile-failure warns are gone; `debug!` is permitted).

- [x] 1.7

### languages (other)

#### Task 1.8: redaction + strict-bool audit for simple, js, jsonpath, xpath, minijinja
**Files:**
- `crates/languages/camel-language-js/src/expression.rs` (modified)
- `crates/languages/camel-language-simple/src/lib.rs` (modified if audit finds defects)
- `crates/languages/camel-language-jsonpath/src/lib.rs` (modified if audit finds defects)
- `crates/languages/camel-language-xpath/src/lib.rs` (modified if audit finds defects)
- `crates/languages/camel-language-minijinja/src/lib.rs` (modified if audit finds defects)

**Steps (scope fixed by mission-340 audit matrix — no discovery phase needed):**
1. **js redaction (LIVE leak, repro R5)** — `crates/languages/camel-language-js/src/engines/worker.rs:427-438`: the full `JsError` Display propagates into `LanguageError` (a `throw new Error('LEAKED-' + camel.body)` reaches `CamelError` verbatim today via `script:`). Replace with a structured class mapping: Boa syntax errors → `EvalFailure{class: Parse, position: None}`; runtime/throw → `EvalFailure{class: Runtime, position: None, detail: None}`; type errors → `TypeMismatch`; engine conversion refusals (already typed in `value.rs`) → `ConversionError`; execution timeout/quota → Timeout/Limit classes. NOTE: Boa 0.22 exposes NO public line/column accessor on `JsError` — position is `None` where inaccessible; never recover positions by scraping the secret-bearing Display text. `detail` may carry only static kind strings. Also fix `expression.rs:21` (`eval_error(source, other)` Display forwarding) and audit `expression.rs:44`.
2. **simple redaction (latent leak activated by P1.1)** — `crates/languages/camel-language-simple/src/evaluator.rs:220-226`: coercion errors embed the operand (`cannot coerce string "{s}" to number`, `expected number, got {v}`). Rewrite to `LanguageError::TypeMismatch { expected, actual, position: None }` with type names only, no operand values.
3. **jsonpath redaction** — `lib.rs:139` `body is not valid JSON: {e}` can quote body snippets: replace with `EvalFailure{class: Conversion, position: None, detail: Some("body is not valid JSON")}` (no serde Display).
4. **minijinja redaction** — `engine.rs:272-273` (`render: {e}`) and `:445-446`: minijinja error Display can quote rendered data. Replace with class+template-position mapping (`minijinja::Error` carries `line()`); detail restricted to the error kind.
5. **xpath log levels** — downgrade the `warn!` records at `lib.rs:101, 109` (compile) and `lib.rs:134, 146` (evaluation) to `debug!` (ADR-0012; the handler owns the level). Redaction already conformant (keep).
6. **Strict bool (sealed Q3) at the audited sites**: simple `lib.rs:57-63` (`_ => true` arm — also resolves the in-crate contradiction with `evaluator.rs:139-147` `is_truthy`), js `expression.rs:225-233` (JS truthiness), jsonpath `lib.rs:193-213` (`is_truthy` incl. the `[]` false vs `{}` true asymmetry), xpath `lib.rs:187-194`. Each becomes: non-bool result → `LanguageError::TypeMismatch { expected: "bool".to_string(), actual: <engine type name>.to_string(), position: None }`. minijinja already refuses predicates at compile time — no change.
7. **js read-only mutation rejection (B4)**: mutations in js use the `camel.*` surface (`camel.headers.set(..)`, `camel.properties.set(..)`, `camel.set_property(..)`, assignment to `camel.body`), NOT bare globals. The crate's existing validation hook returns unit and Boa 0.22's runtime error path is not walkable — do NOT rely on it. Instead, in `create_expression`/`create_predicate` ONLY (read-only modes), parse the RAW SOURCE with the public Boa parser (`boa_parser::Parser` → `boa_ast::StatementList`) and walk the AST statements/expressions for: call expressions whose callee is a member chain rooted at `camel` (`camel.headers.set`, `camel.properties.set`, `camel.set_property`, and any `camel.*` mutation surface), and assignment expressions whose target is a member chain rooted at `camel` (`camel.body`, `camel.headers`, `camel.properties`). Reject with `LanguageError::ParseError { expr, reason: "exchange mutation is not allowed in a read-only expression; use a script: step instead" }`. The MUTATING path (`create_mutating_expression`) skips the walk entirely and keeps accepting all of them (preserve the existing mutating-script acceptance tests).

**Tests:**
- `js_thrown_error_is_class_only` (camel-language-js, repro R5 unit form): evaluate a `script:`-style mutating expression whose script does `throw new Error('LEAKED-' + camel.body)` with body `SECRETVAL` → full error chain (LanguageError Display, `to_expression_failed` CamelError Display, Debug) contains neither `LEAKED-` nor `SECRETVAL`; carries class Runtime (position may be `None` — Boa 0.22 has no public accessor).
- `js_conversion_refusal_maps_to_conversion_class`: a `camel.headers` value beyond ±2^53 round-tripped by a mutating script → `Err` with class Conversion (not Runtime).
- `js_predicate_non_bool_is_type_mismatch`: predicate returning `"false"` / `0` / `[]` → `Err(TypeMismatch)`.
- `simple_coercion_error_has_no_operand`: `${header.secret} > 1` with header `SECRETVAL` and a non-numeric secret → `Err(TypeMismatch)` whose rendering contains no `SECRETVAL`.
- `simple_predicate_empty_string_is_type_mismatch` (repro R4 unit form): predicate `${header.flag}` with flag `""` → `Err(TypeMismatch)` (today: true).
- `jsonpath_invalid_body_error_is_conversion_class_no_snippet`: non-JSON Text body → `Err(EvalFailure{class: Conversion, ..})`, message contains no body fragment.
- `jsonpath_predicate_non_bool_is_type_mismatch` (`"false"` true today, `[]`/`{}` asymmetry): both → `Err(TypeMismatch)`.
- `xpath_predicate_non_bool_is_type_mismatch`: non-empty string / non-zero number predicate results → `Err(TypeMismatch)`.
- `xpath_no_warn_on_eval_failure`: with a capturing tracing subscriber, an evaluation failure emits no `WARN` record from the crate.
- `minijinja_render_error_is_class_and_position`: template raising a type error on body data → class + template line, no rendered-data fragment.
- `js_read_only_mutation_is_compile_error`: `create_expression("camel.headers.set(\"k\", 1)")` and `create_expression("camel.body = 1")` → `Err(ParseError{..})` whose reason mentions `script:`; `create_mutating_expression` with the same sources still compiles (mutating acceptance preserved).
- Command: `cargo test -p camel-language-js -p camel-language-simple -p camel-language-jsonpath -p camel-language-xpath -p camel-language-minijinja --lib` — expected: new tests red before fix, green after.

**Acceptance:**
- `cargo clippy -p camel-language-js -p camel-language-simple -p camel-language-jsonpath -p camel-language-xpath -p camel-language-minijinja -- -D warnings` exits 0.
- All five crates' `--lib` tests pass.
- `grep -rn "warn!\|error!" crates/languages/camel-language-xpath/src/lib.rs` returns 0 hits.
- js mutating-script acceptance tests (body write-back, rollback) still pass unchanged.

- [x] 1.8

### camel-core (integration proof)

#### Task 1.9: per-verb propagation + redaction + catch-when chain integration tests
**Files:**
- `crates/camel-core/tests/expression_failure_propagation_test.rs` (new)

**Steps:**
1. Table-driven integration test. FEATURE PREREQUISITE (verified via `cargo metadata`): rhai, jsonpath, and js are OPTIONAL camel-core features (`lang-rhai`, `lang-jsonpath`, `lang-js` in crates/camel-core/Cargo.toml:54-57) and are NOT in the default graph — the test commands below MUST pass `--features` or the language rows silently skip. The test context constructs its language registry the way camel-core's own route tests do (default registry registers every compiled-in language). For each verb `set_property`, `set_header`, `set_header_if_absent`, `set_body` (dynamic), `script` read-only fallback (a language WITHOUT `MutatingExpression`, e.g. `simple:`, whose expression errors), `script` mutating (rhai `throw` — ScriptMutator+meta path), `filter`, `choice/when`, `loop while`, `validate`, catch `when`, catch `on_when`, finally `on_when`, `split`, `dynamic_router`, `routing_slip`, `recipient_list`, `sort`, `claim_check`, `idempotent_consumer` message id, `log` message expression, `cache invalidate` key, `cache peek-stale` key (two separate rows): build a minimal route with a failing rhai expression (`"no-es-un-numero".parse_float()` or `throw` form as fits the verb), send one exchange, assert the step result is `Err(CamelError::ExpressionFailed{..})`. Each row asserts the FULL E3 diagnostic set: language, route id, step id with verb, position, and class.
2. Handler visibility is part of the table, not a sample: EVERY verb row runs in TWO route variants — (a) wrapped in `do_try/catch {exception: [ExpressionFailed]}` asserting the catch clause runs and the route completes (E2), (b) under `on_exception` asserting the handler observes the `ExpressionFailed` route error (E2).
3. Cross-language rows: the same failing-expression shape for `simple:` and `jsonpath:` (invalid JSON body) through `set_property`/`filter` asserts the swallow fix is language-agnostic (Q5).
4. Catch-when chain: inner step fails with `ProcessorError("boom-A")`; catch `when: rhai: '"no-es-un-numero".parse_float()'` → assert resulting error is `ExpressionFailed` and its `cause` renders `boom-A` (P2).
5. Redaction end-to-end (NOT optional): `set_property {name: s, rhai: '"SECRET".parse_float()'}` → (a) returned error Display contains no `SECRET`; (b) with a capturing `tracing` subscriber (the workspace has one in camel-test; otherwise install `tracing-subscriber` with a `MakeWriter` into a `Vec<u8>` buffer scoped to the test), the emitted records contain no `SECRET`; (c) route the failure into a dead-letter endpoint (mock component capture, following the DLC pattern in existing camel-test suites) and assert the DLC payload error text contains no `SECRET`.
6. Strict bool e2e: `filter` with `rhai: '"false"'` → assert `Err` with class `type-mismatch` (P1).
7. Read-only mutation rejection e2e (B4): YAML route whose `set_property` rhai expression contains `set_property("k", 1)` → assert route ADDITION fails with a compile error whose text contains `script:` (mirror the DSL-route loading pattern used by existing camel-core route-compile tests) (implemented via BuilderStep add_route_definition — same compile path as YAML route add).

**Tests:**
- Table rows named `verb_<verb>_expression_error_fails_step` (one per verb from step 1), each asserting the E3 diagnostic set; handler-visibility variants `verb_<verb>_do_try_visible` and `verb_<verb>_on_exception_visible`; plus `cross_language_simple_and_jsonpath_rows`, `catch_when_error_chains_original`, `catch_on_when_error_chains_original`, `redaction_no_secret_in_error_logs_or_dlc`, `strict_bool_filter_errors`, `read_only_setter_rejected_at_route_add`.
- Command: `cargo test -p camel-core --test expression_failure_propagation_test --features lang-rhai,lang-jsonpath,lang-js,lang-xpath` — expected: all fail on the pre-change tree (errors swallowed), pass after Phase 1.

**Acceptance:**
- All tests in the new file pass.
- `cargo clippy -p camel-core --all-targets -- -D warnings` exits 0.
- PHASE 1 EXIT GATE (after tasks 1.1-1.9): `cargo check --workspace` exits 0; `cargo check -p camel-core --features lang-rhai,lang-jsonpath,lang-js,lang-xpath` exits 0; `cargo clippy --workspace --all-features --exclude camel-cli --exclude camel-component-kafka --exclude security-keycloak --exclude security-wasm-policy -- -D warnings` exits 0; `cargo test -p camel-api -p camel-language-api -p camel-processor -p camel-core -p camel-builder --lib --features camel-core/lang-rhai,lang-jsonpath,lang-js,lang-xpath` passes (feature syntax per-crate as cargo requires); `cargo test -p camel-core --test expression_failure_propagation_test --features lang-rhai,lang-jsonpath,lang-js,lang-xpath` passes.

- [x] 1.9

## Phase 2: native structured values + script integrity

### camel-language-rhai

#### Task 2.1: unified recursive fallible converter + inbound u64 refusal
**Files:**
- `crates/languages/camel-language-rhai/src/lib.rs` (modified)
- `docs/src/languages/rhai.md` (modified)

**Steps:**
1. Replace `dynamic_to_json` and the old infallible `dynamic_to_value` with ONE function: `fn dynamic_to_value(d: rhai::Dynamic, target: &str) -> Result<Value, LanguageError>` — String→String; bool→Bool; finite float→Number; int→Number; unit→Null; `rhai::Map`→object (recurse, passing the SAME top-level `target` unchanged — nested keys are runtime data and MUST NOT be appended to the trusted target string); `rhai::Array`→array (recurse, same target); `char`→one-char String; `rhai::Blob`→array of 0-255 ints; NaN/±Inf→`Err(ConversionError{source_type:"float (non-finite)", target})`; `FnPtr`→`Err(ConversionError{source_type:"FnPtr", target})`; timestamp→`Err(ConversionError{source_type:"timestamp", target})`; any other type→`Err(ConversionError{source_type: d.type_name().to_string(), target})`. No `d.to_string()` fallback anywhere.
2. `json_to_dynamic` becomes fallible `fn json_to_dynamic(v: &Value, target: &str) -> Result<rhai::Dynamic, LanguageError>`: u64 values `> i64::MAX` → `Err(ConversionError{source_type:"u64 > i64::MAX", target})` (sealed Q4); object/array recurse; f64 NaN/Inf cannot occur in valid JSON.
3. `rhai_map_to_value_map` becomes `fn rhai_map_to_value_map(map: &rhai::Map, target: &str) -> Result<std::collections::HashMap<String, Value>, LanguageError>`.
4. Update every caller: read-only `eval_sync` result, mutating result, `json_to_dynamic` callers (property/header reads in `make_scope` and the mutating scope), inbound body conversion. Inside the rhai crate the converter receives only GENERIC targets (`"value"`, `"body"`, `"header entry"`, `"property entry"` — entry-level conversions in the mutating transaction use the generic form because script-map keys are RUNTIME data); the carrier (`LanguageExpressionEval`/`to_expression_failed`) enriches the target from trusted `EvalMeta.target` for read-only verbs, per task 1.3 step 5.
5. Add to `docs/src/languages/rhai.md`: the one-way conversion table (char → one-character string, Blob → byte array 0-255), a "values across the boundary" subsection mirroring the spec's type matrix (V3 documentation duty), the stream-body exposure rules (registered guard surface, the two documented exceptions — `type_of` and the optimized-away discarded statement — and the stream residue split: caught MATERIALIZING operations outside the guard set still fail, while caught pre-clone failures like indexing are ordinary suppressible script errors), and the two-AST note (B4 walk at `None`, execution at `Simple`).

**Tests:**
- `map_round_trips_to_object_property` (GH#62 repro 2 core): `dynamic_to_value(#{"a":1,"b":[2,3]}, "property m")` → object; then `json_to_dynamic` back → equal map.
- `nan_refused_with_target`: `dynamic_to_value(f64::NAN, "property x")` → `Err(ConversionError{source_type:"float (non-finite)", target:"property x"})`.
- `fnptr_refused`, `blob_becomes_byte_array` (`[104,105]` → `[104,105]`), `char_becomes_one_char_string`.
- `u64_above_i64_max_refused_inbound`: `json_to_dynamic(Value::Number(u64::MAX), "property big")` → `Err(ConversionError{..})`.
- `empty_containers_and_unicode_keys`: `#{}` → `{}`, `[]` → `[]`, `#{"café":1}` round-trips.
- Command: `cargo test -p camel-language-rhai --lib` — red before (fallback exists), green after.

**Acceptance:**
- `grep -n "d.to_string()" crates/languages/camel-language-rhai/src/lib.rs` → 0 hits.
- `cargo clippy -p camel-language-rhai -- -D warnings` exits 0; `--lib` tests pass.
- `docs/src/languages/rhai.md` contains the one-way conversion table and the boundary type matrix.
- Route-level conversion assertion (camel-core integration, quick row): `set_property {name: p, rhai: '0.0/0.0'}` → `Err(ExpressionFailed)` with class `conversion`, `conversion.source_type` naming the non-finite float, `conversion.target` = the trusted bare key (`p`), and no data-derived key text.

- [x] 2.1

#### Task 2.2: native body exposure (B1) + stream refusal
**Files:**
- `crates/languages/camel-language-rhai/src/lib.rs` (modified)

**Steps:**
1. Add `fn body_to_dynamic(body: &Body, target: &str) -> Result<rhai::Dynamic, LanguageError>` for the EAGER variants: `Text(s)`/`Xml(s)` → Dynamic string; `Json(v)` → `json_to_dynamic(v, "body")` (object/array/number/bool native); `Empty` → unit; `Bytes(b)` → `rhai::Blob`.
2. `Stream` is ACCESS-AWARE, not eager: bind `body` in the scope as a custom marker type `StreamBodyRef` (a unit struct registered with the engine). Guard EVERY access path, not just two methods:
   - SPIKE-VERIFIED MECHANISM v4 (rhai 1.26.0 — the WORKSPACE-PINNED version — via the real precompiled-AST path: `compile` at an explicit `OptimizationLevel`, then `eval_ast_with_scope`; three spikes. Rejected earlier: `on_var` (fires for plain-assignment LHS; returning an override makes rhai treat the variable as CONSTANT, breaking `body = <value>`), terminal scope sweeps (miss vanished reads), single-AST `None` compilation (breaks the discarded-statement case). Bind `body` as a plain scope variable holding `StreamBodyRef { reads: Arc<AtomicU64>, hits: Arc<AtomicU64> }` with a MANUAL `Clone` impl incrementing `reads` — every materializing read counts: captures (`let t = body`), block-scoped locals, overwritten aliases, array nesting, function arguments, operand clones, result-position reads. Registered guards increment `hits` BY THEIR MARKER-OPERAND COUNT (unary receiver +1; binary op with two marker operands +2) and return the sentinel `STREAM_SENTINEL`. Register guards for `to_string`, `to_debug`, and the operators `==`, `!=`, `<`, `>`, `+`, `-`, `*`, `/` for `(marker, marker)` and `(marker, T)`/`(T, marker)` scalar shapes — this enumerated set IS the registered guard surface.
     POST-EVAL RULE (authoritative, the ONLY rule): on eval `Err`, propagate the mapped error (guard sentinel or `ErrorFunctionNotFound`/`ErrorIndexingType` type-signature → `ConversionError{source_type: "Body::Stream", target: <generic>}`); on eval `Ok`, fail with that conversion error IFF `reads > hits` — guard hits whose failures were handled in-script (try/catch, per the in-script error-handling requirement) are forgiven; unhandled materializations (raw captures, fn args, nesting) are not.
     Spike table (pinned 1.26.0, exec AST Simple — the implementing task MUST re-run it as unit tests so a rhai upgrade changing clone or optimization semantics surfaces): untouched → 0/0 Ok; `x = 42; x` → 0/0 Ok; `x; 42` → 0/0 Ok (Simple optimizes the discarded statement away); result-position `x` → reads=1 marker result (rejected by the result converter); `x.to_string()` uncaught → Err sentinel (1/1); `let r = 0; try { x.to_string(); r = 1; } catch (err) { r = 42; } r` → Ok(42), 1/1 forgiven; `try { let a = x[0]; 1 } catch (err) { 42 }` → Ok, 0/0 (indexing raises `ErrorIndexingType` before cloning; caught); `let r = 0; try { r = (x == x); } catch (err) { r = 42; } r` → Ok(42), 2/2 forgiven; `x == x` uncaught → Err sentinel; `let t = x; 42` → Ok, 1/0 FAIL; caught-guard-then-raw-capture `let r = 0; try { x.to_string(); r = 1; } catch (err) { r = 2; }; let t = x; 99` → Ok, 2/1 FAIL; `fn f(v) { 42 } f(x)` → Ok, 1/0 FAIL; `type_of(x)` → Ok, 0/0.
     DOCUMENTED RESIDUE (normative exception, regression-tested — NOT a mere doc note): an operation OUTSIDE the registered guard surface (for example `body % 1` → `ErrorFunctionNotFound`) that is CAUGHT in-script still fails post-eval (reads=1, hits=0): rhai exposes no caught-error hook, so the boundary refusal cannot see the catch. Registered-guard failures ARE suppressible; unsupported-op refusals are NOT. Pre-clone structured failures (indexing raises `ErrorIndexingType` before cloning, reads=0) are ORDINARY script errors — fully suppressible in-script. Documented in `docs/src/languages/rhai.md` (task 2.1 step 5) and pinned by named regressions.
     DOCUMENTED EXCEPTIONS (both verified reads=0 under the Simple execution AST — this is WHY task 1.7 mandates the Simple exec-AST; neither accesses stream data): `type_of(body)` returns the static string `"StreamBodyRef"` (no clone under either optimization level); and a bare discarded statement-expression `body;` is optimized away by `Simple` (reads=1 under `None`, reads=0 under `Simple`). Both documented in `docs/src/languages/rhai.md` and pinned by tests.
   - A marker-typed result that escapes (defense-in-depth path) is rejected by the post-eval result converter (`dynamic_to_value`) with `ConversionError{source_type: "Body::Stream", target: <generic target>}`.
   - SENTINEL MAPPING RULE (task 1.7 dependency): the sentinel `STREAM_SENTINEL` and the type name `StreamBodyRef` are classified to `ConversionError{source_type: "Body::Stream"}` ONLY through (i) the dedicated guard sentinel and (ii) the TYPE-SIGNATURE fields of structured engine variants (`ErrorFunctionNotFound`'s signature, `ErrorIndexingType`/"Indexer unavailable" type list). `ErrorRuntime` payloads (e.g. `throw "StreamBodyRef"`) are NEVER text-matched — a thrown string that happens to contain the type name stays class `runtime` and redacted.
   - Unregistered operations outside the guard set fail with structured engine errors (`ErrorFunctionNotFound` with the operand-type signature, `ErrorIndexingType` — the actual indexing variant — with the type list). The task 1.7 mapper classifies these to `ConversionError{source_type: "Body::Stream"}` ONLY through the type-signature/type-list FIELDS of those structured variants. `ErrorRuntime` payloads and free Display text are NEVER matched — a thrown string that happens to contain `StreamBodyRef` stays class `runtime`, redacted.
   - The mutating transaction (task 2.3) NEVER writes a marker-typed body back: a post-eval body that is still the marker is classified unassigned (comparison marker==marker) and any attempted conversion of it is the conversion error above.
   A script that never touches `body` succeeds; the stream itself is never read (the marker holds no handle) and its identity/unconsumed state is preserved.
3. Replace `exchange.input.body.as_text().unwrap_or("")` at the read-only eval site (lib.rs:502 region) and in `make_scope` (lib.rs:223 region) with `body_to_dynamic` for eager variants and the `StreamBodyRef` marker for `Stream`. Thread the resulting `Dynamic` through `eval_sync` (change the `body_text: String` parameter to `body: rhai::Dynamic`).
4. Mutating path (lib.rs:360, :398-401): same exposure for the mutating scope. Body write-back from a `StreamBodyRef` value is impossible by construction (the transaction refuses it with the conversion error) — an untouched stream body stays a stream.

**Tests:**
- `json_body_reads_as_map`: exchange body `Body::Json({"n":1})`; script `type_of(body)` via a predicate/expr returning `type_of(body) == "map"` → Ok(true).
- `xml_body_reads_as_string_and_keeps_variant` (read-only): `Body::Xml("<a/>")` → script sees string; exchange untouched by read-only eval (assert variant still Xml).
- `bytes_body_reads_as_blob`: `Body::Bytes(vec![1,2])` → `type_of(body) == "blob"`.
- `empty_body_is_unit`: `Body::Empty` → `type_of(body) == "()"`.
- `stream_body_fails_loudly_when_read`: `Body::Stream(..)` + script that reads body (`body.to_string()` or arithmetic on body) → `Err` with class `conversion` and source_type `Body::Stream` (not empty string).
- `stream_body_untouched_script_succeeds`: `Body::Stream(..)` + script that only sets a header/reads a property → step SUCCEEDS; the exchange body is still `Body::Stream` with its original identity (a captured stream handle equals by pointer), unconsumed.
- `stream_body_index_and_arithmetic_fail_loudly`: `body[0]` and `body + 1` on a stream body → `Err` with class `conversion` and source_type `Body::Stream` (`+` is a registered guard; indexing raises `ErrorIndexingType` — both mapped via sentinel/type-signature, never free text).
- `stream_body_bare_read_in_result_position_fails`: read-only expression evaluating to bare `body` on a stream → `Err` with class `conversion` (marker rejected by result conversion).
- `stream_body_alias_fails_on_use`: `let x = body; x.to_string()` → `Err` class `conversion` (registered method guard).
- `stream_body_type_of_returns_marker_name_documented`: `type_of(body)` on a stream → succeeds returning the static string `"StreamBodyRef"` (documented exception; no stream data accessed) — pinned so a rhai upgrade that changes this surfaces.
- `stream_body_read_discarded_result_fails`: `let x = body; 42` → `Err` class `conversion` (clone counter catches the vanished capture).
- `stream_body_caught_method_read_is_suppressed_e7`: `let r = 0; try { body.to_string(); r = 1; } catch (err) { r = 42; } r` → step SUCCEEDS, result 42 (outer fallback variable — the try STATEMENT form returns unit; guard hit forgiven; E7 honored).
- `stream_body_caught_index_read_is_suppressed_e7`: `try { let a = body[0]; 1 } catch (err) { 42 }` → SUCCEEDS (indexing raises before cloning; caught).
- `stream_body_caught_comparison_is_suppressed_e7`: `let r = 0; try { r = (body == body); } catch (err) { r = 42; } r` → SUCCEEDS, result 42 (binary guard counts both operand clones).
- `stream_body_capture_after_caught_guard_still_fails`: `let r = 0; try { body.to_string(); r = 1; } catch (err) { r = 2; }; let x = body; 99` → `Err` class `conversion` (raw capture unhandled).
- `stream_body_caught_unsupported_op_still_fails`: `let r = 0; try { let q = body % 1; r = 1; } catch (err) { r = 42; } r` → step FAILS with class `conversion` (unsupported-op residue: boundary refusal is not a suppressible script error); the exchange body is STILL `Body::Stream`, unconsumed, identity unchanged; no partial mutation applied (rollback verified).
- `stream_body_block_capture_fails`: `{ let x = body; } 42` → `Err` class `conversion`.
- `stream_body_alias_overwritten_fails`: `let x = body; x = 0; 42` → `Err` class `conversion`.
- `stream_body_nested_in_array_fails`: `let x = [body]; 42` → `Err` class `conversion`.
- `stream_body_fn_arg_fails`: `fn f(v) { 42 } f(body)` → `Err` class `conversion`.
- `stream_body_discarded_statement_is_noop`: `body; 42` → step SUCCEEDS (documented exception 2; engine materializes nothing).
- `stream_body_overwrite_assignment_allowed`: script `body = "replacement";` on a stream body → step SUCCEEDS; exchange body is `Body::Text("replacement")`; a subsequent read in the SAME script (`body = "r"; body`) returns the replaced string (spike-verified).
- `stream_body_read_after_replacement_succeeds`: `body = "r"; body.to_string()` → succeeds (guards see the replaced value, not the marker).
- Command: `cargo test -p camel-language-rhai --lib` — red before, green after.

**Acceptance:**
- `grep -n 'as_text().unwrap_or("")' crates/languages/camel-language-rhai/src/lib.rs` → 0 hits.
- `cargo clippy -p camel-language-rhai -- -D warnings` exits 0; `--lib` tests pass.

- [x] 2.2

#### Task 2.3: mutating-script transaction (B2 + B3)
**Files:**
- `crates/languages/camel-language-rhai/src/lib.rs` (modified)

**Steps:**
1. Rework `eval_mut_sync` write-back into a validate-all-then-commit transaction. Pre-eval: push `body` (native via `body_to_dynamic`), `headers` map, `properties` map into scope. Post-eval: compute the pending change set INSIDE the evaluator using EXECUTED-WRITE tracking plus value comparison, never snapshot inequality alone:
   - Comparison helper `fn rhai_values_differ(pre: &rhai::Dynamic, post: &rhai::Dynamic) -> bool` is RECURSIVE AND TYPE-SENSITIVE, not a bare rhai `==` (rhai's `==` treats `1 == 1.0` as true, which would suppress an intended int-to-float type change — a V1 violation). Algorithm: if the type discriminants differ (`is_int` vs `is_float`, `is_string`, `is_bool`, `is_unit`, map, array, blob, char) → CHANGED; for scalars of the same type, compare with a dedicated frozen comparison `Engine` evaluating `a == b` over the two bound values (never Rust `PartialEq` on `Dynamic`); for maps, recurse per key over the union of keys; for arrays, recurse pairwise by index. Any structural difference at any depth → CHANGED.
   - Body: `rhai_values_differ(pre_body, post_body)` → assigned / unassigned. Same-value, same-type assignment detects as unassigned — no write needed, and that is the PINNED behavior (see the spec scenario "Same-value assignment writes nothing").
   - Headers/properties: for each key in either snapshot or post-eval map, classify added / removed / changed via the same helper on the per-key values (generic targets only — no runtime key names in diagnostics).
   - Nested changes inside a stored map value: the recursion sees the whole value as changed; the converter writes the whole (recursively converted) value back.
2. Convert EVERY pending change with the task 2.1 converter using GENERIC entry targets — `property entry`, `header entry`, `body` — NEVER `property <k>`/`header <k>`: script-map keys are RUNTIME data and can carry exchange secrets; they must not enter diagnostics. Trusted destination enrichment (`property m`) happens only at the carrier from `EvalMeta.target` for declarative verbs (task 1.3 step 5). Conversion happens BEFORE any mutation: any error → `Err` with NO mutation applied (existing caller rollback stays as second line of defense).
3. Apply: body only when assigned (a header-only script leaves any body variant bit-identical; `Empty` never becomes `Text("")`); per-key writes only for added/changed entries (untouched entries keep their original `serde_json::Value` handle); removed keys removed from the exchange.
4. `body = #{...}` assignment: script assigning a map to `body` writes `Body::Json(object)` (via the existing value→body mapping); assigning a string writes `Body::Text`.

**Tests:**
- `header_only_script_preserves_json_body_bit_identical`: body `Body::Json({"a":[1,2]})`; script sets one header → after eval, body is `Body::Json` and `serde_json::to_string` equal to input; header set.
- `header_only_script_preserves_xml_body_variant`: `Body::Xml("<a/>")` + header-only script → variant still `Xml`, content equal.
- `empty_body_stays_empty`: `Body::Empty` + header-only script → still `Body::Empty` (not `Text("")`).
- `untouched_map_property_stays_object`: property `m = {"a":[1,2]}`; script modifies property `other` → `m` still a JSON object (not a string).
- `removed_header_is_removed`: script `headers.remove("x")` → header `x` gone.
- `body_assignment_writes_json`: script `body = #{ "k": 1 };` → `Body::Json({"k":1})`.
- `conversion_failure_commits_nothing`: property `ok` set to `1` and property `bad` set to `0.0/0.0` in one script → Err; exchange keeps pre-step values for both and the header diff is NOT applied.
- `same_value_body_assignment_writes_nothing`: script `body = body;` on a `Body::Json` input → comparison detects equality → no write-back; body bit-identical (same Value instance semantics, variant unchanged).
- `unexecuted_assignment_is_untouched`: script that never mentions `body` → identical outcome (control row for the test above).
- `nested_map_change_is_detected`: property `m` pre-set to `#{...}`-shaped object; script mutates `properties.m.inner = 42` via the map → post-eval comparison marks `m` changed → property rewritten as object with `inner: 42` and other keys intact.
- `numeric_type_change_is_a_change`: property `n` pre-set to rhai int `1`; script assigns `1.0` (float) → comparison detects the discriminant change → property rewritten as JSON number `1.0` (not silently kept as `1`). Same test for body: `Body::Json(1)` reassigned `1.0` → body value becomes `1.0`.
- `same_value_type_preserved_assignment_writes_nothing`: property `n` = `1`; script assigns `1` again → unassigned; property keeps its original `serde_json::Value` handle (identity-stable).
- `secret_key_never_enters_diagnostics`: exchange header `Authorization=tok` drives a script that writes `properties["SECRET-KEY-" + headers.Authorization] = 0.0/0.0` (NaN) → step fails; the LanguageError, `ExpressionFailed` Display, and Debug contain neither `SECRET-KEY-tok` nor the key fragment — target reads `property entry` only.
- Command: `cargo test -p camel-language-rhai --lib` — red before, green after.

**Acceptance:**
- `cargo clippy -p camel-language-rhai -- -D warnings` exits 0; `--lib` tests pass.
- `cargo test -p camel-test --test do_try_test` still passes (script rollback semantics intact).
- PHASE 2 EXIT GATE (after tasks 2.1-2.3): `cargo test -p camel-language-rhai --lib` passes in full; `cargo check --workspace` exits 0.

- [x] 2.3

## Phase 3: GH #62 regression suite

### camel-test

#### Task 3.1: GH#62 repro 1 (silent error swallow) end-to-end + try statement form
**Files:**
- `crates/camel-test/tests/gh62_language_boundary_regression_test.rs` (new)

**Steps:**
1. Embed the GH#62 repro 1 YAML VERBATIM (issue #62, hallazgo 1):
```yaml
routes:
  - id: probe
    from: "direct:probe"
    steps:
      - set_property:
          name: x
          rhai: |-
            "no-es-un-numero".parse_float()
      - set_body:
          rhai: |-
            "x=" + property("x").to_string()
```
2. Run it through the camel-test harness (follow the pattern of `crates/camel-test/tests/do_try_test.rs` for route loading + exchange send + error capture). Assert: the route fails at the `set_property` step with `CamelError::ExpressionFailed` (class `arithmetic`); the `set_body` step never runs; no `x=` body is produced.
3. Add a wrapped variant: same steps inside `do_try/catch {exception: [ExpressionFailed]}` → assert catch runs, route completes, and the caught error is class `arithmetic`.
4. Add the in-script try STATEMENT form (E7): script `let x = (); try { x = "no".parse_float(); } catch (err) { x = 42; } x` via `set_property` → assert property is `42` and no route error.

**Tests:**
- `gh62_repro1_error_fails_step_loudly`, `gh62_repro1_do_try_catches_expression_failed`, `gh62_in_script_try_statement_suppresses_error`.
- Command: `cargo test -p camel-test --test gh62_language_boundary_regression_test` — expected: repro1 test fails on pre-change tree (swallowed), passes after Phase 1.

**Acceptance:**
- All three tests pass; the YAML is byte-identical to the issue text (store it in the test file as a raw string).

- [x] 3.1

#### Task 3.2: GH#62 repro 2 (map degrades to string) end-to-end
**Files:**
- `crates/camel-test/tests/gh62_language_boundary_regression_test.rs` (modified, append)

**Steps:**
1. Embed the GH#62 repro 2 YAML VERBATIM (issue #62, hallazgo 2):
```yaml
routes:
  - id: probe
    from: "direct:probe"
    steps:
      - set_property:
          name: m
          rhai: |-
            #{ "a": 1, "b": 2 }
      - set_body:
          rhai: |-
            let x = property("m");
            "type=" + type_of(x) + " str=" + x.to_string()
```
2. Run it; assert the resulting body text starts with `type=map` (not `type=string`), and the property `m` on the final exchange is the JSON object `{"a":1,"b":2}`.
3. Add a cross-step indexing assertion: second script reads `property("m")["a"]` equals `1` (the failure mode reported in the issue).

**Tests:**
- `gh62_repro2_map_round_trips_as_map`, `gh62_repro2_property_indexing_works`.
- Command: `cargo test -p camel-test --test gh62_language_boundary_regression_test` — repro2 fails on pre-Phase-2 tree, passes after.

**Acceptance:**
- Both tests pass; YAML byte-identical to the issue text.
- Full suite: `cargo test -p camel-test --test gh62_language_boundary_regression_test` green.

- [x] 3.2

#### Task 3.3: mission-340 audit repros R1, R5 end-to-end (R2 deferred)
**Files:**
- `crates/camel-test/tests/gh62_language_boundary_regression_test.rs` (modified, append)

**Steps:**
1. Repro R1 (jsonpath filter swallow, audit matrix section 2): route `direct:r1` with a Text non-JSON body, `filter: {jsonpath: '$.x'}` inside `do_try/catch {exception: [ExpressionFailed]}` sending to `mock:caught`. Assert: catch fires (caught count 1). On the pre-change tree this repro printed `FAIL #caught — expected 1, got 0` (exchange silently dropped). Use the audit's YAML shape (stored at `/tmp/nix-shell.fOYIGv/opencode/langaudit-repro/r1-route.yaml` — embed the route inline in the test; the /tmp copy is ephemeral).
2. Repro R5 (js live leak, audit matrix section 2): route `direct:r5` with body `SECRETVAL`, `script: {js: 'throw new Error("LEAKED-" + camel.body);'}` → assert the route error (CamelError Display AND the captured stderr/log assertion the harness exposes) contains neither `LEAKED-` nor `SECRETVAL`, and carries class Runtime (position may be None). Pre-change: stderr printed the verbatim leak. NOTE: audit repro R2 (xpath Inf-to-Null) is NOT here — it requires the deferred xpath V2 refusal and moves to that follow-up change.

**Tests:**
- `audit_r1_jsonpath_filter_error_caught`, `audit_r5_js_leak_redacted_end_to_end` (R5 asserts captured log/stderr AND the DLC-style route-error payload contain no leak, matching task 1.9 step 5's non-optional redaction assertions).
- Command: `cargo test -p camel-test --test gh62_language_boundary_regression_test` — R1/R5 fail on the pre-change tree, pass after Phase 1. PREREQUISITE: camel-test's default graph has NO js language — add `camel-language-js` as a dev-dependency of camel-test (workspace dep, mirrors the existing `camel-language-rhai`/`camel-language-jsonpath` entries in its Cargo.toml) and register it in the harness language registry the same way rhai is registered. Verify with `cargo tree -p camel-test | grep camel-language-js` before writing the R5 row.

**Acceptance:**
- Both tests pass.
- The suite file documents each repro's origin (audit matrix section 2 + GH #62).

- [x] 3.3
