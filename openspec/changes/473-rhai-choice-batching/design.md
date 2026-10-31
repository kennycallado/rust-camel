# Design: 473-rhai-choice-batching

## Approach

The Choice runtime path evaluates `PredicateSource` clauses that are already
opaque `Async` closures (built by `compile_filter_predicate`), so batching
must be selected at route-compilation time, before the closures exist.

1. **API hook (`camel-language-api`, `lib.rs`)** — additive, defaulted:
   - `#[async_trait] trait PredicateGroup: Send + Sync { async fn
     first_match(&self, exchange: &Exchange) ->
     Result<Option<usize>, IndexedPredicateError> }` — evaluates the group's
     predicates in declaration order, returns the first matching clause index
     (`Ok(None)` = no match), or an indexed `LanguageError`. Object-safe via
     the crate's existing `#[async_trait]` convention (same as
     `Expression`/`Predicate`), invocable through `dyn PredicateGroup`.
   - `Language::create_predicate_group(&self, scripts: &[&str]) ->
     Result<Option<Box<dyn PredicateGroup>>, LanguageError>` with a default
     body returning `Ok(None)` ("group not supported"). `IndexedPredicateError
     { index, error }` is a plain struct in the same crate.
2. **Rhai group (`camel-language-rhai`)** — `create_predicate_group` compiles
   every script with the existing read-only two-AST path and returns a
   `RhaiPredicateGroup`. `first_match` uses ONE exchange snapshot and ONE
   evaluation engine per message: the body Dynamic and header/property maps
   are converted once via the existing converters, and one engine is built
   once (prebuilt-package construction keeps this cheap). Each clause then
   evaluates against that engine with a FRESH `Scope`; per-clause resource
   budgets stay per-predicate because rhai's operation counter is per
   top-level eval call (`GlobalState.num_operations` starts at 0 for every
   `eval_ast_with_scope` — rhai 1.26.1 `src/eval/global_state.rs:96`), never
   accumulated across clauses; a dedicated test pins this. Per-clause
   execution timeouts are preserved with the same shape as today's
   `eval_async` (`crates/languages/camel-language-rhai/src/lib.rs:954-986`):
   each clause evaluation is independently wrapped in
   `tokio::time::timeout(execution_timeout_ms, spawn_blocking(...))` against
   the shared engine (rhai `Engine` evals take `&self`, so the per-message
   engine serves every clause), so individually in-budget clauses succeed
   even when their aggregate duration exceeds one timeout, and a clause
   exceeding its own timeout yields an indexed `Timeout`-class error and
   never evaluates later clauses. `Ok(true)` on AST
   i returns immediately; later clauses are not touched (no snapshot reads,
   no evaluation). `Err` returns `IndexedPredicateError { index: i, error }`.
   Stream-marker bodies keep today's per-clause conversion (no snapshot
   share, no clone-count side effects); their stream counters are per-predicate
   by construction.
3. **Processor segment (`camel-processor`, `choice.rs`)** — new
   `ChoiceGroupSegment { group: Arc<dyn PredicateGroup>, metas: Vec<EvalMeta>,
   bodies: Vec<OutcomeSegment>, otherwise: Option<OutcomeSegment> }` implementing
   `OutcomePipeline`: `Some(i)` → `bodies[i].run(exchange)`; `None` →
   otherwise/passthrough; `Err(e)` → `Failed(to_expression_failed(e.error,
   &metas[e.index]))` — identical carrier and redaction to the sequential
   path. Clause attribution: the grouped path builds one `EvalMeta` per clause
   whose `verb` carries the trusted clause position (`choice/when[<index>]`,
   compile-time config only — never runtime data), so `ExpressionFailed`
   errors and logs identify the offending clause even though all clauses share
   the outer step id. `camel-processor` already depends on
   `camel-language-api`; no new dependency edges.
4. **Compile-time selection (`camel-core`)** —
   `step_resolution.rs` gains `compile_choice_group(...)`: when every
   `DeclarativeWhenStep.predicate.language` is the same string AND the
   registry's `Language` returns `Some` from `create_predicate_group`, build
   `ChoiceGroupSegment` with one `EvalMeta` per clause (same `eval_meta`
   helper). Any other shape (mixed languages, `Ok(None)`, zero/one clauses)
   keeps the existing per-clause `compile_filter_predicate` sequential path.
   `control_flow.rs` `DeclarativeChoice` arm calls it; programmatic
   `BuilderStep::Choice` (closure predicates) is untouched. Filter/Split and
   DoTry catch predicates keep per-clause compilation — no context sharing.
5. **Bench-first order** — extend `crates/camel-processor/benches/choice.rs`
   (compiles on base): sequential rhai 1/10-clause shapes via the production
   closure shape, mixed rhai/simple 10-clause shape; run with
   `--save-baseline choice-pre` and record in `benchmarks/records/` per the
   Mission-457 record convention (Criterion estimates + summary.md, no
   run.json). After implementation add grouped shapes and re-run vs
   `choice-pre`. Grouped 10-clause same-language shapes cover match-first,
   match-middle, match-last, and no-match (all clauses evaluated — worst
   case); the 1-clause and mixed-language shapes stay sequential controls.
   Gate: 10-clause same-language grouped median <= 250 us at every covered
   match position including no-match; sequential shapes must not regress.

Semantics invariants (all mandatory): clause order preserved; first match
wins; failed predicate evaluation fails the step (never falls through to
otherwise); skipped clauses are never evaluated (side-effect probe test);
one engine and one snapshot per message with per-clause budgets via rhai's
per-eval operation counter; error/log attribution with the offending clause
index through the clause-distinguishable `EvalMeta` (verb
`choice/when[<index>]`); redaction via the existing `to_expression_failed`
cause rules; no mutable context shared between clauses.

## Affected crates

- `camel-language-api`: `PredicateGroup` + `IndexedPredicateError` + defaulted
  `Language::create_predicate_group`.
- `camel-language-rhai`: `RhaiPredicateGroup` implementation.
- `camel-core`: group selection in `step_resolution.rs` +
  `step_compilers/control_flow.rs` `DeclarativeChoice`.
- `camel-processor`: `ChoiceGroupSegment` + bench extension (dev-dep
  `camel-language-rhai`).

## Architecture boundaries

Data/control plane split (ADR-0032): selection happens once at route
compilation (control plane, trusted config only); evaluation stays data plane
over the untrusted exchange. Error contract unchanged:
`CamelError::ExpressionFailed` via `to_expression_failed` with per-clause
`EvalMeta` (ADR-0006; CONTEXT-MAP "ExpressionFailed / EvalMeta"). Segment
semantics follow `OutcomePipeline`/`PipelineOutcome` (ADR-0024/0025) and the
`segment-outcome-composition` invariant. Read-only boundary per the
`language-value-boundary` spec (predicates cannot mutate the exchange; the
group snapshot is immutable after construction). Engine construction follows
the `rhai-engine-reuse` spec (prebuilt shared modules, no thread-locals, no
engine mutex). No user-facing config; the optimization is automatic with
mixed-language fallback.

## Alternatives rejected

- Batch inside `PredicateSource::Async` closures — impossible without leaking
  the opaque closures (non-goal).
- A `Vec<Predicate>` batch constructor on `Language` returning concrete
  closures per clause — cannot express shared-snapshot evaluation; violates
  the hook shape from the e_gpt ruling.
- Grouping inside `camel-core` only (no language-api hook) — core cannot
  compile rhai ASTs; would leak language internals across the boundary.
