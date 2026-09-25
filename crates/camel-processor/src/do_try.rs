//! ## Stop semantics (ADR-0025)
//!
//! This segment implements `OutcomePipeline` and propagates `PipelineOutcome::Stopped(ex)` with the exchange state intact (including mutations made inside the segment body before Stop fired). See ADR-0025 §3 (stopped-exchange-state-preservation invariant).
//!
//! Catch-block failure envelope (bd rc-zgbqq): if a catch body fails, the catch error stays the main error in every disposition; the original error lives in the `warn` record ("do_try catch block failed; catch error supersedes original", fields `original_error` and `catch_error`) and, when a span is active, in the span event.
//! See the error-handler spec, requirement "delegate failure propagates the original error" (that rule covers `handled_by` delegates, not catch bodies).

use camel_api::error_handler::ExceptionDisposition;
use camel_api::exchange::PROPERTY_EXCEPTION_HANDLED;
use camel_api::{BoxProcessor, CamelError, Exchange, FilterPredicate};
use tower::Service;
use tower::ServiceExt;

use crate::error_handler::record_span_error;

/// Matcher for a `doCatch` clause.
#[derive(Clone)]
pub enum CatchMatcher {
    /// Match by CamelError variant name (e.g. ["ProcessorError", "Io"]).
    /// `"*"` matches any variant — equivalent to Camel's `doCatch(Throwable.class)`.
    ByVariant(Vec<String>),
    /// Match by predicate over the Exchange.
    Predicate(FilterPredicate),
}

impl CatchMatcher {
    /// Returns `true` if this matcher matches the given error and exchange.
    pub fn matches(&self, err: &CamelError, ex: &Exchange) -> bool {
        match self {
            CatchMatcher::ByVariant(names) => {
                if names.iter().any(|n| n == "*") {
                    return true;
                }
                names.iter().any(|n| n == err.variant_name())
            }
            CatchMatcher::Predicate(p) => p(ex),
        }
    }
}

/// A single `doCatch` clause.
#[derive(Clone)]
pub struct CatchClause {
    /// The main matcher (variant-name list or predicate).
    pub matcher: CatchMatcher,
    /// Optional sub-predicate evaluated AFTER the main matcher passes.
    pub on_when: Option<FilterPredicate>,
    /// Sub-pipeline executed when the clause matches.
    pub steps: Vec<BoxProcessor>,
    /// ADR-0019 disposition: Handled (default), Propagate, or Continued (rejected at parse time).
    /// In YAML, use lowercase: `handled`, `propagate`, `continued`.
    pub disposition: ExceptionDisposition,
}

/// The `doTry` processor. Wrap with `BoxProcessor::new(DoTryService::new(...))`.
#[derive(Clone)]
pub struct DoTryService {
    /// Steps in the try block.
    pub try_steps: Vec<BoxProcessor>,
    /// Catch clauses evaluated first-match-wins.
    pub catch_clauses: Vec<CatchClause>,
    /// Steps in the finally block (empty = no finally).
    pub finally_steps: Vec<BoxProcessor>,
    /// Optional onWhen predicate for finally.
    pub finally_on_when: Option<FilterPredicate>,
}

impl DoTryService {
    /// Create a new `DoTryService` with the given try steps.
    pub fn new(try_steps: Vec<BoxProcessor>) -> Self {
        Self {
            try_steps,
            catch_clauses: Vec::new(),
            finally_steps: Vec::new(),
            finally_on_when: None,
        }
    }

    /// Full constructor used by the compile pipeline (Task 10b control_flow.rs).
    /// Builder API (Task 8) constructs via `new()` + field mutation.
    pub fn with_catch_and_finally(
        try_steps: Vec<BoxProcessor>,
        catch_clauses: Vec<CatchClause>,
        finally_steps: Vec<BoxProcessor>,
        finally_on_when: Option<FilterPredicate>,
    ) -> Self {
        Self {
            try_steps,
            catch_clauses,
            finally_steps,
            finally_on_when,
        }
    }
}

/// Run a sequence of steps, preserving the last exchange state on error.
/// Returns `Err(Box<(last_ex, err)>)` so DoTry can populate exception properties.
async fn run_pipeline(
    steps: Vec<BoxProcessor>,
    mut ex: Exchange,
) -> Result<Exchange, Box<(Exchange, CamelError)>> {
    for mut svc in steps {
        match svc.ready().await {
            Ok(ready) => {
                let snapshot = ex.clone();
                match ready.call(ex).await {
                    Ok(new_ex) => ex = new_ex,
                    Err(err) => return Err(Box::new((snapshot, err))),
                }
            }
            Err(err) => return Err(Box::new((ex, err))),
        }
    }
    Ok(ex)
}

/// Run the finally block. Camel parity for finally-throws:
/// - If finally succeeds (or is skipped): `Completed` with its exchange.
/// - If finally throws AND there was a previous error: `Restore` (caller
///   logs and restores the previous error).
/// - If finally throws AND no previous error: `NoPreviousFail` (caller logs
///   and propagates finally_err).
///
/// Logging lives at the CALLERS so the restore record's field names can
/// match the calling flow (`previous_error` vs `catch_error`, bd rc-zgbqq).
async fn run_finally(
    finally_steps: Vec<BoxProcessor>,
    finally_on_when: Option<FilterPredicate>,
    ex: Exchange,
    previous_err: Option<CamelError>,
) -> FinallyOutcome {
    if finally_steps.is_empty() {
        return FinallyOutcome::Completed(ex);
    }
    if let Some(on_when) = &finally_on_when
        && !on_when(&ex)
    {
        return FinallyOutcome::Completed(ex);
    }
    match run_pipeline(finally_steps, ex).await {
        Ok(ex) => FinallyOutcome::Completed(ex),
        Err(failed) => {
            let (_, finally_err) = *failed;
            match previous_err {
                Some(prev) => FinallyOutcome::Restore {
                    previous: prev,
                    finally_err,
                },
                None => FinallyOutcome::NoPreviousFail(finally_err),
            }
        }
    }
}

/// Result of `run_finally`. Tower-local to this module — unrelated to the
/// identically-named enum in `do_try_segment.rs`.
// The variant shape is fixed by the sealed do_try ruling (bd rc-zgbqq);
// Exchange simply dwarfs the error payloads, so allow the lint instead of
// boxing and drifting from the specified envelope.
#[allow(clippy::large_enum_variant)]
enum FinallyOutcome {
    /// Finally ran (or was skipped) successfully; carries its exchange.
    Completed(Exchange),
    /// Finally threw with no previous error; the finally error wins.
    NoPreviousFail(CamelError),
    /// Finally threw with a previous error; the previous error is restored.
    Restore {
        previous: CamelError,
        finally_err: CamelError,
    },
}

impl tower::Service<Exchange> for DoTryService {
    type Response = Exchange;
    type Error = CamelError;
    type Future = std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Self::Response, Self::Error>> + Send>,
    >;

    fn poll_ready(
        &mut self,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        std::task::Poll::Ready(Ok(()))
    }

    fn call(&mut self, mut exchange: Exchange) -> Self::Future {
        // Clear stale CamelExceptionHandled marker from prior handlers in the same route.
        // Exchange::clear_error() does NOT touch HANDLED, so direct map access is required.
        exchange.properties.remove(PROPERTY_EXCEPTION_HANDLED);

        let try_steps = self.try_steps.clone();
        let catch_clauses = self.catch_clauses.clone();
        let finally_steps = self.finally_steps.clone();
        let finally_on_when = self.finally_on_when.clone();

        Box::pin(async move {
            let try_result = run_pipeline(try_steps, exchange).await;
            match try_result {
                Ok(ex) => match run_finally(finally_steps, finally_on_when, ex, None).await {
                    FinallyOutcome::Completed(ex) => Ok(ex),
                    FinallyOutcome::NoPreviousFail(fin) => {
                        tracing::warn!(error = %fin, "doFinally threw");
                        Err(fin)
                    }
                    // Unreachable: the try-Ok flow passes no previous error.
                    FinallyOutcome::Restore { previous, .. } => Err(previous),
                },
                Err(failed) => {
                    let (failed_ex, original_err) = *failed;
                    let mut ex = failed_ex;
                    ex.set_error(original_err.clone());

                    for clause in catch_clauses {
                        let CatchClause {
                            matcher,
                            on_when,
                            steps,
                            disposition,
                        } = clause;
                        if !matcher.matches(&original_err, &ex) {
                            continue;
                        }
                        if let Some(ref on_when) = on_when
                            && !on_when(&ex)
                        {
                            continue;
                        }

                        let catch_result = run_pipeline(steps, ex.clone()).await;

                        return match catch_result {
                            Ok(ok_ex) => {
                                // Determine previous-error threading based on disposition.
                                // IMPORTANT: do NOT call handle_error() before run_finally() —
                                // handle_error() calls clear_error() which removes
                                // PROPERTY_EXCEPTION_MESSAGE/KIND/CAUGHT, preventing finally
                                // steps from inspecting the caught exception.
                                //
                                // disposition semantics (ADR-0019 strict):
                                //   Handled    -> catch output is final, no propagation
                                //   Propagate  -> catch ran for side-effects, original propagates
                                //   Continued  -> rejected at parse time (defensive: treat as
                                //                 Propagate + log if we ever reach runtime)
                                let prev = match disposition {
                                    ExceptionDisposition::Handled => None,
                                    ExceptionDisposition::Continued => {
                                        tracing::warn!(
                                            "ExceptionDisposition::Continued reached doTry runtime; \
                                             treating as Propagate. Should have been rejected at parse time."
                                        );
                                        Some(original_err.clone())
                                    }
                                    // Propagate and any future variant thread the original error.
                                    _ => Some(original_err.clone()),
                                };
                                let mut ex = match run_finally(
                                    finally_steps.clone(),
                                    finally_on_when.clone(),
                                    ok_ex,
                                    prev,
                                )
                                .await
                                {
                                    FinallyOutcome::Completed(ex) => ex,
                                    FinallyOutcome::NoPreviousFail(fin) => {
                                        tracing::warn!(error = %fin, "doFinally threw");
                                        return Err(fin);
                                    }
                                    FinallyOutcome::Restore {
                                        previous,
                                        finally_err,
                                    } => {
                                        tracing::warn!(
                                            finally_error = %finally_err,
                                            previous_error = %previous,
                                            "doFinally threw; restoring previous exception (Camel parity)"
                                        );
                                        return Err(previous);
                                    }
                                };
                                // AFTER finally has run (and had access to exception props),
                                // apply handle_error() for Handled disposition to clear the
                                // error state and set CamelExceptionHandled=true marker.
                                if matches!(disposition, ExceptionDisposition::Handled) {
                                    ex.handle_error();
                                }
                                match disposition {
                                    ExceptionDisposition::Handled => Ok(ex),
                                    _ => Err(original_err),
                                }
                            }
                            Err(failed) => {
                                // Catch threw. Sealed failure envelope (bd rc-zgbqq):
                                // the catch error stays the main error (returned
                                // below and threaded into finally); the original is
                                // surfaced through the unconditional warn record
                                // and, best-effort, the active span. The event
                                // lands on the current span when one is entered —
                                // no new span, no second event.
                                let (catch_ex, catch_err) = *failed;
                                tracing::warn!(
                                    original_error = %original_err,
                                    catch_error = %catch_err,
                                    "do_try catch block failed; catch error supersedes original"
                                );
                                record_span_error(&catch_err);
                                // Run finally with previous=catch_err. Per Camel
                                // parity, if finally itself throws, catch_err is
                                // restored.
                                let outcome = run_finally(
                                    finally_steps.clone(),
                                    finally_on_when.clone(),
                                    catch_ex,
                                    Some(catch_err.clone()),
                                )
                                .await;
                                if let FinallyOutcome::Restore {
                                    previous,
                                    finally_err,
                                } = outcome
                                {
                                    tracing::warn!(
                                        catch_error = %previous,
                                        finally_error = %finally_err,
                                        "doFinally threw after failed catch; restoring catch error"
                                    );
                                    return Err(previous);
                                }
                                Err(catch_err)
                            }
                        };
                    }

                    // No catch matched. Run finally with previous=original. Propagate original.
                    let outcome = run_finally(
                        finally_steps,
                        finally_on_when,
                        ex,
                        Some(original_err.clone()),
                    )
                    .await;
                    if let FinallyOutcome::Restore {
                        previous,
                        finally_err,
                    } = outcome
                    {
                        tracing::warn!(
                            finally_error = %finally_err,
                            previous_error = %previous,
                            "doFinally threw; restoring previous exception (Camel parity)"
                        );
                        return Err(previous);
                    }
                    Err(original_err)
                }
            }
        })
    }
}

// ── DoTrySegment (ADR-0025 OutcomePipeline) ──────────────────────────────

/// Compilable segment for a `doCatch` clause within a `DoTrySegment`.
///
/// `disposition` controls outcome when the catch body completes:
#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_log_capture::{capture_debugs_with_span_records, captured_field, record_field};
    use camel_api::{BoxProcessor, BoxProcessorExt};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn passthrough() -> BoxProcessor {
        BoxProcessor::from_fn(move |ex| Box::pin(async move { Ok(ex) }))
    }

    fn record_call(flag: Arc<AtomicU32>) -> BoxProcessor {
        BoxProcessor::from_fn(move |ex| {
            let f = flag.clone();
            Box::pin(async move {
                f.fetch_add(1, Ordering::SeqCst);
                Ok(ex)
            })
        })
    }

    fn always_fail(err: CamelError) -> BoxProcessor {
        BoxProcessor::from_fn(move |_ex| {
            let e = err.clone();
            Box::pin(async move { Err(e) })
        })
    }

    #[tokio::test]
    async fn happy_path_try_succeeds_finally_runs() {
        let finally_flag = Arc::new(AtomicU32::new(0));
        let mut svc = DoTryService::new(vec![passthrough()]);
        svc.finally_steps = vec![record_call(finally_flag.clone())];

        let mut boxed = BoxProcessor::new(svc);
        let result = boxed.ready().await.unwrap().call(Exchange::default()).await;
        assert!(result.is_ok());
        assert_eq!(finally_flag.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn catch_by_variant_handled_returns_ok() {
        let try_step = always_fail(CamelError::ProcessorError("boom".into()));
        let mut svc = DoTryService::new(vec![try_step]);
        svc.catch_clauses.push(CatchClause {
            matcher: CatchMatcher::ByVariant(vec!["ProcessorError".into()]),
            on_when: None,
            steps: vec![passthrough()],
            disposition: ExceptionDisposition::Handled,
        });

        let mut boxed = BoxProcessor::new(svc);
        let result = boxed.ready().await.unwrap().call(Exchange::default()).await;
        assert!(result.is_ok(), "Handled must return Ok");
        let ex = result.unwrap();
        assert_eq!(
            ex.properties.get(PROPERTY_EXCEPTION_HANDLED),
            Some(&camel_api::Value::Bool(true)),
            "CamelExceptionHandled must be set via handle_error()"
        );
    }

    #[tokio::test]
    async fn catch_by_variant_propagate_runs_side_effects_and_rethrows() {
        let original = CamelError::ProcessorError("boom".into());
        let try_step = always_fail(original.clone());
        let side_effect = Arc::new(AtomicU32::new(0));
        let catch_step = record_call(side_effect.clone());
        let mut svc = DoTryService::new(vec![try_step]);
        svc.catch_clauses.push(CatchClause {
            matcher: CatchMatcher::ByVariant(vec!["ProcessorError".into()]),
            on_when: None,
            steps: vec![catch_step],
            disposition: ExceptionDisposition::Propagate,
        });

        let mut boxed = BoxProcessor::new(svc);
        let result = boxed.ready().await.unwrap().call(Exchange::default()).await;
        assert!(result.is_err(), "Propagate must rethrow original");
        assert!(matches!(result.unwrap_err(), CamelError::ProcessorError(_)));
        assert_eq!(
            side_effect.load(Ordering::SeqCst),
            1,
            "catch branch must have run for side-effects"
        );
    }

    #[tokio::test]
    async fn catch_by_predicate_matches_via_exception_kind() {
        let try_step = always_fail(CamelError::Io("disk full".into()));
        let predicate = FilterPredicate::new(|ex: &Exchange| {
            ex.properties
                .get(camel_api::exchange::PROPERTY_EXCEPTION_KIND)
                .map(|v| matches!(v, camel_api::Value::String(s) if s == "io"))
                .unwrap_or(false)
        });
        let mut svc = DoTryService::new(vec![try_step]);
        svc.catch_clauses.push(CatchClause {
            matcher: CatchMatcher::Predicate(predicate),
            on_when: None,
            steps: vec![passthrough()],
            disposition: ExceptionDisposition::Handled,
        });

        let mut boxed = BoxProcessor::new(svc);
        let result = boxed.ready().await.unwrap().call(Exchange::default()).await;
        assert!(
            result.is_ok(),
            "Predicate matcher must catch the error and Handled must return Ok"
        );
    }

    #[tokio::test]
    async fn on_when_filters_clause_and_next_evaluated() {
        let try_step = always_fail(CamelError::ProcessorError("boom".into()));
        let first_call = Arc::new(AtomicU32::new(0));
        let second_call = Arc::new(AtomicU32::new(0));

        let mut svc = DoTryService::new(vec![try_step]);
        svc.catch_clauses.push(CatchClause {
            matcher: CatchMatcher::ByVariant(vec!["ProcessorError".into()]),
            on_when: Some(FilterPredicate::new(|_ex| false)),
            steps: vec![record_call(first_call.clone())],
            disposition: ExceptionDisposition::Handled,
        });
        svc.catch_clauses.push(CatchClause {
            matcher: CatchMatcher::ByVariant(vec!["*".into()]),
            on_when: None,
            steps: vec![record_call(second_call.clone())],
            disposition: ExceptionDisposition::Handled,
        });

        let mut boxed = BoxProcessor::new(svc);
        let _ = boxed.ready().await.unwrap().call(Exchange::default()).await;
        assert_eq!(first_call.load(Ordering::SeqCst), 0);
        assert_eq!(second_call.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn first_match_wins_subsequent_clauses_not_evaluated() {
        let try_step = always_fail(CamelError::Io("err".into()));
        let first_call = Arc::new(AtomicU32::new(0));
        let second_call = Arc::new(AtomicU32::new(0));

        let mut svc = DoTryService::new(vec![try_step]);
        svc.catch_clauses.push(CatchClause {
            matcher: CatchMatcher::ByVariant(vec!["Io".into()]),
            on_when: None,
            steps: vec![record_call(first_call.clone())],
            disposition: ExceptionDisposition::Handled,
        });
        svc.catch_clauses.push(CatchClause {
            matcher: CatchMatcher::ByVariant(vec!["*".into()]),
            on_when: None,
            steps: vec![record_call(second_call.clone())],
            disposition: ExceptionDisposition::Handled,
        });

        let mut boxed = BoxProcessor::new(svc);
        let _ = boxed.ready().await.unwrap().call(Exchange::default()).await;
        assert_eq!(first_call.load(Ordering::SeqCst), 1);
        assert_eq!(second_call.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn no_clause_matches_propagates_original() {
        let try_step = always_fail(CamelError::CircuitOpen("cb".into()));
        let mut svc = DoTryService::new(vec![try_step]);
        svc.catch_clauses.push(CatchClause {
            matcher: CatchMatcher::ByVariant(vec!["ProcessorError".into()]),
            on_when: None,
            steps: vec![passthrough()],
            disposition: ExceptionDisposition::Handled,
        });

        let mut boxed = BoxProcessor::new(svc);
        let result = boxed.ready().await.unwrap().call(Exchange::default()).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), CamelError::CircuitOpen(_)));
    }

    #[tokio::test]
    async fn catch_branch_throws_new_error_wins() {
        let try_step = always_fail(CamelError::ProcessorError("orig".into()));
        let catch_step = always_fail(CamelError::Io("catch-fail".into()));
        let mut svc = DoTryService::new(vec![try_step]);
        svc.catch_clauses.push(CatchClause {
            matcher: CatchMatcher::ByVariant(vec!["ProcessorError".into()]),
            on_when: None,
            steps: vec![catch_step],
            disposition: ExceptionDisposition::Handled,
        });

        let mut boxed = BoxProcessor::new(svc);
        let result = boxed.ready().await.unwrap().call(Exchange::default()).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), CamelError::Io(_)));
    }

    #[tokio::test]
    async fn finally_throws_with_no_previous_error_propagates_finally_error() {
        let finally_step = always_fail(CamelError::Config("fin".into()));
        let mut svc = DoTryService::new(vec![passthrough()]);
        svc.finally_steps = vec![finally_step];

        let mut boxed = BoxProcessor::new(svc);
        let result = boxed.ready().await.unwrap().call(Exchange::default()).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), CamelError::Config(_)));
    }

    #[tokio::test]
    async fn finally_throws_with_previous_error_restores_previous() {
        let try_step = always_fail(CamelError::ProcessorError("orig".into()));
        let finally_step = always_fail(CamelError::Config("fin".into()));
        let mut svc = DoTryService::new(vec![try_step]);
        svc.finally_steps = vec![finally_step];

        let mut boxed = BoxProcessor::new(svc);
        let result = boxed.ready().await.unwrap().call(Exchange::default()).await;
        assert!(result.is_err());
        assert!(
            matches!(result.unwrap_err(), CamelError::ProcessorError(_)),
            "previous error must be restored when finally throws (Camel parity)"
        );
    }

    #[tokio::test]
    async fn finally_on_when_false_skips_finally() {
        let finally_call = Arc::new(AtomicU32::new(0));
        let mut svc = DoTryService::new(vec![passthrough()]);
        svc.finally_steps = vec![record_call(finally_call.clone())];
        svc.finally_on_when = Some(FilterPredicate::new(|_ex| false));

        let mut boxed = BoxProcessor::new(svc);
        let _ = boxed.ready().await.unwrap().call(Exchange::default()).await;
        assert_eq!(finally_call.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn stale_handled_marker_cleared_on_entry() {
        let mut ex = Exchange::default();
        ex.set_property(PROPERTY_EXCEPTION_HANDLED, camel_api::Value::Bool(true));
        let svc = DoTryService::new(vec![passthrough()]);
        let mut boxed = BoxProcessor::new(svc);
        let result = boxed.ready().await.unwrap().call(ex).await;
        let ex = result.unwrap();
        assert!(
            !ex.properties.contains_key(PROPERTY_EXCEPTION_HANDLED),
            "stale CamelExceptionHandled must be cleared on entry"
        );
    }

    #[tokio::test]
    async fn nested_do_try_inner_catch_does_not_leak_to_outer() {
        let inner = {
            let try_step = always_fail(CamelError::Io("inner".into()));
            let mut d = DoTryService::new(vec![try_step]);
            d.catch_clauses.push(CatchClause {
                matcher: CatchMatcher::ByVariant(vec!["Io".into()]),
                on_when: None,
                steps: vec![passthrough()],
                disposition: ExceptionDisposition::Handled,
            });
            BoxProcessor::new(d)
        };
        let mut outer = DoTryService::new(vec![inner]);
        outer.catch_clauses.push(CatchClause {
            matcher: CatchMatcher::ByVariant(vec!["Io".into()]),
            on_when: None,
            steps: vec![passthrough()],
            disposition: ExceptionDisposition::Handled,
        });

        let mut boxed = BoxProcessor::new(outer);
        let result = boxed.ready().await.unwrap().call(Exchange::default()).await;
        assert!(
            result.is_ok(),
            "outer must see Ok because inner handled its own error"
        );
    }

    #[tokio::test]
    async fn catch_all_only_fires_when_no_specific_clause_matches() {
        let try_step = always_fail(CamelError::Io("err".into()));
        let processor_call = Arc::new(AtomicU32::new(0));
        let catch_all_call = Arc::new(AtomicU32::new(0));

        let mut svc = DoTryService::new(vec![try_step]);
        // First clause (specific) targets ProcessorError — won't match Io error.
        svc.catch_clauses.push(CatchClause {
            matcher: CatchMatcher::ByVariant(vec!["ProcessorError".into()]),
            on_when: None,
            steps: vec![record_call(processor_call.clone())],
            disposition: ExceptionDisposition::Handled,
        });
        // Second clause is the catch-all — should fire.
        svc.catch_clauses.push(CatchClause {
            matcher: CatchMatcher::ByVariant(vec!["*".into()]),
            on_when: None,
            steps: vec![record_call(catch_all_call.clone())],
            disposition: ExceptionDisposition::Handled,
        });

        let mut boxed = BoxProcessor::new(svc);
        let _ = boxed.ready().await.unwrap().call(Exchange::default()).await;
        assert_eq!(
            processor_call.load(Ordering::SeqCst),
            0,
            "specific ProcessorError clause must not fire on Io error"
        );
        assert_eq!(
            catch_all_call.load(Ordering::SeqCst),
            1,
            "catch-all clause must fire when no specific clause matches"
        );
    }

    #[tokio::test]
    async fn catch_throws_with_finally_runs_finally_and_propagates_catch_err() {
        let try_step = always_fail(CamelError::ProcessorError("orig".into()));
        let catch_step = always_fail(CamelError::Io("catch-fail".into()));
        let finally_flag = Arc::new(AtomicU32::new(0));
        let finally_step = record_call(finally_flag.clone());

        let mut svc = DoTryService::new(vec![try_step]);
        svc.catch_clauses.push(CatchClause {
            matcher: CatchMatcher::ByVariant(vec!["ProcessorError".into()]),
            on_when: None,
            steps: vec![catch_step],
            disposition: ExceptionDisposition::Handled,
        });
        svc.finally_steps = vec![finally_step];

        let mut boxed = BoxProcessor::new(svc);
        let result = boxed.ready().await.unwrap().call(Exchange::default()).await;

        assert!(result.is_err());
        assert!(
            matches!(result.unwrap_err(), CamelError::Io(_)),
            "catch_err must propagate (not original ProcessorError)"
        );
        assert_eq!(
            finally_flag.load(Ordering::SeqCst),
            1,
            "doFinally must run even when catch throws"
        );
    }

    #[tokio::test]
    async fn catch_throws_and_finally_throws_restores_catch_err() {
        let try_step = always_fail(CamelError::ProcessorError("orig".into()));
        let catch_step = always_fail(CamelError::Io("catch-fail".into()));
        let finally_step = always_fail(CamelError::Config("fin-fail".into()));

        let mut svc = DoTryService::new(vec![try_step]);
        svc.catch_clauses.push(CatchClause {
            matcher: CatchMatcher::ByVariant(vec!["ProcessorError".into()]),
            on_when: None,
            steps: vec![catch_step],
            disposition: ExceptionDisposition::Handled,
        });
        svc.finally_steps = vec![finally_step];

        let mut boxed = BoxProcessor::new(svc);
        let result = boxed.ready().await.unwrap().call(Exchange::default()).await;

        assert!(result.is_err());
        assert!(
            matches!(result.unwrap_err(), CamelError::Io(_)),
            "catch_err (Io) must be restored over finally_err (Config) per Camel parity"
        );
    }

    #[tokio::test]
    async fn finally_on_when_false_with_previous_error_still_propagates_original() {
        let try_step = always_fail(CamelError::ProcessorError("orig".into()));
        let finally_flag = Arc::new(AtomicU32::new(0));
        let finally_step = record_call(finally_flag.clone());

        let mut svc = DoTryService::new(vec![try_step]);
        // No catch clauses → original error stays.
        svc.finally_steps = vec![finally_step];
        svc.finally_on_when = Some(FilterPredicate::new(|_ex| false));

        let mut boxed = BoxProcessor::new(svc);
        let result = boxed.ready().await.unwrap().call(Exchange::default()).await;

        assert!(result.is_err());
        assert!(
            matches!(result.unwrap_err(), CamelError::ProcessorError(_)),
            "original error must propagate even when finally_on_when skips finally"
        );
        assert_eq!(
            finally_flag.load(Ordering::SeqCst),
            0,
            "doFinally must NOT run when on_when returns false"
        );
    }

    fn catch_fails_service() -> BoxProcessor {
        let try_step = always_fail(CamelError::ProcessorError("orig-lost".into()));
        let catch_step = always_fail(CamelError::Io("catch-fail".into()));
        let mut svc = DoTryService::new(vec![try_step]);
        svc.catch_clauses.push(CatchClause {
            matcher: CatchMatcher::ByVariant(vec!["ProcessorError".into()]),
            on_when: None,
            steps: vec![catch_step],
            disposition: ExceptionDisposition::Handled,
        });
        BoxProcessor::new(svc)
    }

    #[tokio::test]
    async fn catch_throws_under_propagate_disposition_returns_catch_err() {
        let try_step = always_fail(CamelError::ProcessorError("orig".into()));
        let catch_step = always_fail(CamelError::Io("catch-fail".into()));
        let mut svc = DoTryService::new(vec![try_step]);
        svc.catch_clauses.push(CatchClause {
            matcher: CatchMatcher::ByVariant(vec!["ProcessorError".into()]),
            on_when: None,
            steps: vec![catch_step],
            disposition: ExceptionDisposition::Propagate,
        });

        let mut boxed = BoxProcessor::new(svc);
        let result = boxed.ready().await.unwrap().call(Exchange::default()).await;
        assert!(
            matches!(result, Err(CamelError::Io(_))),
            "Propagate-disposition catch failure must return the catch error, \
             same envelope as Handled, got: {result:?}"
        );
    }

    #[test]
    fn catch_throws_logs_original_and_catch_error() {
        let mut boxed = catch_fails_service();
        let (result, captured, _span_records) = capture_debugs_with_span_records(|| {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("current-thread runtime")
                .block_on(async { boxed.ready().await.unwrap().call(Exchange::default()).await })
        });

        assert!(
            matches!(result, Err(CamelError::Io(_))),
            "catch failure must return Err(Io), got: {result:?}"
        );
        // Structured field lookup on the envelope record, not message
        // formatting.
        let original = record_field(&captured, "do_try catch block failed", "original_error")
            .unwrap_or_else(|| {
                panic!("envelope record missing original_error field, captured: {captured:?}")
            });
        let catch = record_field(&captured, "do_try catch block failed", "catch_error")
            .unwrap_or_else(|| {
                panic!("envelope record missing catch_error field, captured: {captured:?}")
            });
        assert!(
            original.contains("orig-lost"),
            "original_error must carry the original error, got: {original}"
        );
        assert!(
            catch.contains("catch-fail"),
            "catch_error must carry the catch error, got: {catch}"
        );
    }

    #[test]
    fn catch_throws_marks_span_error_and_event() {
        let mut boxed = catch_fails_service();
        let (result, captured, span_records) = capture_debugs_with_span_records(|| {
            // Declared `error` field so record_span_error's record lands.
            let span = tracing::info_span!("dotry_test", error = tracing::field::Empty);
            let _guard = span.enter();
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("current-thread runtime")
                .block_on(async { boxed.ready().await.unwrap().call(Exchange::default()).await })
        });

        assert!(
            matches!(result, Err(CamelError::Io(_))),
            "catch failure must return Err(Io), got: {result:?}"
        );
        // The span recorded the `error` field with the CATCH error.
        // Structured field lookup: substring `error=` would also match
        // `original_error=` suffixes.
        assert!(
            span_records.iter().any(|line| {
                captured_field(line, "error").is_some_and(|v| v.contains("catch-fail"))
            }),
            "expected span error record carrying the catch error, span records: {span_records:?}"
        );
        // The WARN event still carries original_error while a span is active.
        let original = record_field(
            &captured,
            "do_try catch block failed",
            "original_error",
        )
        .unwrap_or_else(|| {
            panic!(
                "envelope record missing original_error field under active span, captured: {captured:?}"
            )
        });
        assert!(
            original.contains("orig-lost"),
            "original_error must carry the original error under an active span, got: {original}"
        );
    }

    #[test]
    fn catch_and_finally_throw_logs_finally_error() {
        let try_step = always_fail(CamelError::ProcessorError("orig".into()));
        let catch_step = always_fail(CamelError::Io("catch-fail".into()));
        let finally_step = always_fail(CamelError::Config("fin-fail".into()));
        let mut svc = DoTryService::new(vec![try_step]);
        svc.catch_clauses.push(CatchClause {
            matcher: CatchMatcher::ByVariant(vec!["ProcessorError".into()]),
            on_when: None,
            steps: vec![catch_step],
            disposition: ExceptionDisposition::Handled,
        });
        svc.finally_steps = vec![finally_step];

        let mut boxed = BoxProcessor::new(svc);
        let (result, captured, _span_records) = capture_debugs_with_span_records(|| {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("current-thread runtime")
                .block_on(async { boxed.ready().await.unwrap().call(Exchange::default()).await })
        });

        assert!(
            matches!(result, Err(CamelError::Io(_))),
            "catch error must be restored over finally error, got: {result:?}"
        );
        let catch = record_field(
            &captured,
            "doFinally threw after failed catch; restoring catch error",
            "catch_error",
        )
        .unwrap_or_else(|| {
            panic!("catch-failed restore record missing catch_error field, captured: {captured:?}")
        });
        let finally = record_field(
            &captured,
            "doFinally threw after failed catch; restoring catch error",
            "finally_error",
        )
        .unwrap_or_else(|| {
            panic!(
                "catch-failed restore record missing finally_error field, captured: {captured:?}"
            )
        });
        assert!(
            catch.contains("catch-fail"),
            "catch_error must carry the catch error, got: {catch}"
        );
        assert!(
            finally.contains("fin-fail"),
            "finally_error must carry the finally error, got: {finally}"
        );
    }
}
