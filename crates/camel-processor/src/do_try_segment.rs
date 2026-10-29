//! ## Stop semantics (ADR-0025)
//!
//! This segment implements `OutcomePipeline` and propagates `PipelineOutcome::Stopped(ex)`
//! with the exchange state intact. See ADR-0025 §3.

use crate::do_try::CatchMatcher;
use crate::do_try::chain_predicate_error;
use crate::error_handler::record_span_error;
use camel_api::error_handler::ExceptionDisposition;
use camel_api::exchange::PROPERTY_EXCEPTION_HANDLED;
use camel_api::outcome_pipeline::OutcomePipeline;
use camel_api::pipeline_outcome::PipelineOutcome;
use camel_api::{CamelError, Exchange, PredicateSource};
use std::future::Future;
use std::pin::Pin;

/// - `Handled`: the exchange continues through finally and downstream.
/// - `Propagate`: the catch body runs for side effects, finally runs with the
///   catch body's exchange, and the original try-error re-throws as `Failed`.
#[derive(Clone)]
pub struct CatchClauseSegment {
    pub matcher: CatchMatcher,
    pub on_when: Option<PredicateSource>,
    pub body: camel_api::OutcomeSegment,
    pub disposition: ExceptionDisposition,
}

/// Compilable segment for a `doFinally` clause within a `DoTrySegment`.
#[derive(Clone)]
pub struct FinallyClauseSegment {
    pub on_when: Option<PredicateSource>,
    pub body: camel_api::OutcomeSegment,
}

/// Outcome-aware structural EIP segment for the doTry/doCatch/doFinally pattern.
///
/// Operates at the `PipelineOutcome` layer so that `Stopped(ex)` from a
/// sub-step (e.g. Stop EIP) is preserved with the exchange including all
/// mutations. See ADR-0025 §5.1 for full semantics.
pub struct DoTrySegment {
    pub try_body: camel_api::OutcomeSegment,
    pub catches: Vec<CatchClauseSegment>,
    pub finally: Option<FinallyClauseSegment>,
}

impl Clone for DoTrySegment {
    fn clone(&self) -> Self {
        Self {
            try_body: self.try_body.clone(),
            catches: self.catches.clone(),
            finally: self.finally.clone(),
        }
    }
}

/// Narrow outcome type for `run_finally_body` so the compiler enforces
/// exhaustiveness at the two call sites instead of `unreachable!()`.
/// This is a private, transient type; the `Stopped` exchange is boxed to keep
/// the `Err` variant of `run_finally_body` small (clippy::result_large_err).
enum FinallyOutcome {
    Stopped(Box<Exchange>),
    Failed(CamelError),
    /// The finally `on_when` predicate itself failed; the typed error fails
    /// the segment.
    PredicateFailed(CamelError),
}

/// Run the finally body if present and on_when permits.
/// Free function (not a method) to avoid borrow conflict with
/// `self.catches.iter_mut()` inside the catch loop.
/// The `FinallyOutcome` variants move the `Exchange` (the catch loop needs
/// its state); boxing would rewrite the segment call chain for a lint
/// threshold.
#[allow(clippy::result_large_err)]
async fn run_finally_body(
    finally: &mut Option<FinallyClauseSegment>,
    ex: Exchange,
) -> Result<Exchange, FinallyOutcome> {
    let Some(f) = finally.as_mut() else {
        return Ok(ex);
    };
    if let Some(on_when) = &f.on_when {
        match on_when.matches(&ex).await {
            Ok(true) => {}
            Ok(false) => return Ok(ex),
            Err(err) => return Err(FinallyOutcome::PredicateFailed(err)),
        }
    }
    match f.body.run(ex).await {
        PipelineOutcome::Completed(e) => Ok(e),
        PipelineOutcome::Stopped(e) => Err(FinallyOutcome::Stopped(Box::new(e))),
        PipelineOutcome::Failed(e) => Err(FinallyOutcome::Failed(e)),
    }
}

/// Surface `failure` after running finally.
///
/// Used for a failed catch predicate (the predicate error surfaced as the
/// main failure), the no-catch-matched path, the Propagate / fail-closed
/// path (the original try error), and the catch-body-failure path (the catch
/// error is the surfaced failure).
/// `failure` becomes the main failure (Camel parity: a throwing finally body
/// restores it; Stop in finally still stops the outer route; a failed finally
/// `on_when` predicate supersedes it).
async fn fail_after_finally(
    finally: &mut Option<FinallyClauseSegment>,
    ex: Exchange,
    failure: CamelError,
) -> PipelineOutcome {
    match run_finally_body(finally, ex).await {
        Ok(_) => PipelineOutcome::Failed(failure),
        Err(FinallyOutcome::Stopped(e)) => PipelineOutcome::Stopped(*e),
        Err(FinallyOutcome::Failed(_finally_err)) => {
            tracing::warn!(
                error = %failure,
                "doFinally threw; restoring previous error (Camel parity)"
            );
            PipelineOutcome::Failed(failure)
        }
        Err(FinallyOutcome::PredicateFailed(pred_err)) => PipelineOutcome::Failed(pred_err),
    }
}

impl OutcomePipeline for DoTrySegment {
    fn clone_box(&self) -> Box<dyn OutcomePipeline> {
        Box::new(self.clone())
    }

    fn run<'a>(
        &'a mut self,
        mut exchange: Exchange,
    ) -> Pin<Box<dyn Future<Output = PipelineOutcome> + Send + 'a>> {
        Box::pin(async move {
            // Stale handled-marker protocol inherited from the deleted
            // `DoTryService` (clear-on-entry; `Exchange::clear_error` does not
            // touch HANDLED).
            exchange.properties.remove(PROPERTY_EXCEPTION_HANDLED);
            // Pre-clone for the "no catch matched" path; `exchange` itself is
            // consumed by `try_body.run(...)` below.
            let exchange_for_unmatched = exchange.branch_clone();

            // 1. Run try_body. If Stopped, return immediately (skip catch AND finally).
            let try_outcome = self.try_body.run(exchange).await;
            let returned_ex = match try_outcome {
                PipelineOutcome::Stopped(ex) => return PipelineOutcome::Stopped(ex),
                PipelineOutcome::Completed(ex) => ex,
                PipelineOutcome::Failed(err) => {
                    // 2. try failed — try each catch in order. Walk the catch
                    // chain starting from the failed try-exchange. On the first
                    // matching catch whose `on_when` is true (or unset), run it.
                    // A failed catch `when`/`on_when` predicate replaces the
                    // original error (preserved as `cause`) and fails the scope.
                    let mut current_ex = exchange_for_unmatched;
                    current_ex.set_error(err.clone());
                    // Distinguishes "a catch handled the error" (loop breaks
                    // with `current_ex` = catch output) from "the loop
                    // exhausted with no match" (must propagate the original).
                    let mut catch_matched = false;
                    for catch in self.catches.iter_mut() {
                        let matched = match catch.matcher.matches(&err, &current_ex).await {
                            Ok(m) => m,
                            Err(pred_err) => {
                                let chained = chain_predicate_error(pred_err, err);
                                return fail_after_finally(&mut self.finally, current_ex, chained)
                                    .await;
                            }
                        };
                        if !matched {
                            continue;
                        }
                        if let Some(on_when) = &catch.on_when {
                            match on_when.matches(&current_ex).await {
                                Ok(true) => {}
                                Ok(false) => continue,
                                Err(pred_err) => {
                                    let chained = chain_predicate_error(pred_err, err);
                                    return fail_after_finally(
                                        &mut self.finally,
                                        current_ex,
                                        chained,
                                    )
                                    .await;
                                }
                            }
                        }
                        // Pre-clone for the catch-body-failure path:
                        // `PipelineOutcome::Failed` drops the exchange, so the
                        // finally body still needs a pre-catch snapshot (same
                        // pattern as `exchange_for_unmatched` above).
                        let catch_input_ex = current_ex.branch_clone();
                        match catch.body.run(current_ex).await {
                            // Stop in catch: skip finally AND outer route halts.
                            PipelineOutcome::Stopped(stopped_ex) => {
                                return PipelineOutcome::Stopped(stopped_ex);
                            }
                            // Catch handled — exit catch chain with this exchange.
                            PipelineOutcome::Completed(next) => {
                                match catch.disposition {
                                    ExceptionDisposition::Handled => {
                                        current_ex = next;
                                        catch_matched = true;
                                        break;
                                    }
                                    ExceptionDisposition::Continued => {
                                        current_ex = next;
                                        catch_matched = true;
                                        break;
                                    }
                                    // Propagate and any future variant run finally then
                                    // surface the original error (fail-closed).
                                    _ => {
                                        return fail_after_finally(&mut self.finally, next, err)
                                            .await;
                                    }
                                }
                            }
                            // Catch-body Failed: Java try/catch/finally
                            // parity (JLS §14.20.2) — the finally body runs
                            // exactly once even when the catch body fails,
                            // and the catch error remains the surfaced
                            // failure. No catch-of-catch: the doTry island
                            // invariant (ADR-0025 #4) is unchanged, so the
                            // catch chain is NOT re-entered.
                            PipelineOutcome::Failed(catch_err) => {
                                // Sealed failure envelope: the catch error
                                // stays the main error (returned below);
                                // the original try error is surfaced
                                // through the unconditional warn record
                                // and, when a span is active, the span
                                // error record. No new span is created.
                                tracing::warn!(
                                    original_error = %err,
                                    catch_error = %catch_err,
                                    "do_try catch block failed; catch error supersedes original"
                                );
                                record_span_error(&catch_err);
                                return fail_after_finally(
                                    &mut self.finally,
                                    catch_input_ex,
                                    catch_err,
                                )
                                .await;
                            }
                        }
                    }
                    if catch_matched {
                        // A catch handled the error: continue to the shared
                        // finally path and return its exchange as Completed.
                        current_ex
                    } else {
                        // No catch matched: run finally, then propagate the
                        // original try error as `Failed`. Falling through to the
                        // shared finally path would return `Completed` and
                        // swallow the error, letting the route continue past
                        // the scope (the segment is the only doTry runtime).
                        return fail_after_finally(&mut self.finally, current_ex, err).await;
                    }
                }
            };
            // 3. Run finally if present (skip if try/catch returned Stopped —
            // already returned above; catch-body failures flow through
            // `fail_after_finally` and return there).
            match run_finally_body(&mut self.finally, returned_ex).await {
                Ok(ex) => PipelineOutcome::Completed(ex),
                Err(FinallyOutcome::Stopped(e)) => PipelineOutcome::Stopped(*e),
                Err(FinallyOutcome::Failed(finally_err)) => {
                    tracing::warn!(
                        error = %finally_err,
                        "doFinally threw during/after catch; surfacing finally error"
                    );
                    PipelineOutcome::Failed(finally_err)
                }
                // Failed finally `on_when` predicate: the typed error fails
                // the segment.
                Err(FinallyOutcome::PredicateFailed(pred_err)) => PipelineOutcome::Failed(pred_err),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::do_try::CatchMatcher;
    use crate::test_log_capture::{capture_debugs_with_span_records, captured_field, record_field};
    use camel_api::pipeline_outcome::PipelineOutcome;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};

    // ── Helpers for DoTrySegment tests ──

    struct CompleteSegment;

    impl OutcomePipeline for CompleteSegment {
        fn clone_box(&self) -> Box<dyn OutcomePipeline> {
            Box::new(CompleteSegment)
        }
        fn run<'a>(
            &'a mut self,
            exchange: Exchange,
        ) -> Pin<Box<dyn Future<Output = PipelineOutcome> + Send + 'a>> {
            Box::pin(async move { PipelineOutcome::Completed(exchange) })
        }
    }

    fn seg_complete() -> camel_api::OutcomeSegment {
        camel_api::OutcomeSegment::new(Box::new(CompleteSegment))
    }

    struct FailSegment(CamelError);

    impl OutcomePipeline for FailSegment {
        fn clone_box(&self) -> Box<dyn OutcomePipeline> {
            Box::new(FailSegment(self.0.clone()))
        }
        fn run<'a>(
            &'a mut self,
            _exchange: Exchange,
        ) -> Pin<Box<dyn Future<Output = PipelineOutcome> + Send + 'a>> {
            let e = self.0.clone();
            Box::pin(async move { PipelineOutcome::Failed(e) })
        }
    }

    fn seg_fail(err: CamelError) -> camel_api::OutcomeSegment {
        camel_api::OutcomeSegment::new(Box::new(FailSegment(err)))
    }

    struct MutateThenStop {
        mutator: Arc<dyn Fn(&mut Exchange) + Send + Sync>,
    }

    impl OutcomePipeline for MutateThenStop {
        fn clone_box(&self) -> Box<dyn OutcomePipeline> {
            Box::new(MutateThenStop {
                mutator: Arc::clone(&self.mutator),
            })
        }
        fn run<'a>(
            &'a mut self,
            mut exchange: Exchange,
        ) -> Pin<Box<dyn Future<Output = PipelineOutcome> + Send + 'a>> {
            let m = Arc::clone(&self.mutator);
            Box::pin(async move {
                m(&mut exchange);
                PipelineOutcome::Stopped(exchange)
            })
        }
    }

    fn seg_stop_with(
        mutator: impl Fn(&mut Exchange) + Send + Sync + 'static,
    ) -> camel_api::OutcomeSegment {
        camel_api::OutcomeSegment::new(Box::new(MutateThenStop {
            mutator: Arc::new(mutator),
        }))
    }

    struct RecordCall {
        counter: Arc<AtomicU32>,
    }

    impl OutcomePipeline for RecordCall {
        fn clone_box(&self) -> Box<dyn OutcomePipeline> {
            Box::new(RecordCall {
                counter: Arc::clone(&self.counter),
            })
        }
        fn run<'a>(
            &'a mut self,
            exchange: Exchange,
        ) -> Pin<Box<dyn Future<Output = PipelineOutcome> + Send + 'a>> {
            let c = Arc::clone(&self.counter);
            Box::pin(async move {
                c.fetch_add(1, Ordering::SeqCst);
                PipelineOutcome::Completed(exchange)
            })
        }
    }

    fn seg_record(counter: Arc<AtomicU32>) -> camel_api::OutcomeSegment {
        camel_api::OutcomeSegment::new(Box::new(RecordCall { counter }))
    }

    struct CountThenFail {
        counter: Arc<AtomicU32>,
        err: CamelError,
    }

    impl OutcomePipeline for CountThenFail {
        fn clone_box(&self) -> Box<dyn OutcomePipeline> {
            Box::new(CountThenFail {
                counter: Arc::clone(&self.counter),
                err: self.err.clone(),
            })
        }
        fn run<'a>(
            &'a mut self,
            _exchange: Exchange,
        ) -> Pin<Box<dyn Future<Output = PipelineOutcome> + Send + 'a>> {
            let c = Arc::clone(&self.counter);
            let e = self.err.clone();
            Box::pin(async move {
                c.fetch_add(1, Ordering::SeqCst);
                PipelineOutcome::Failed(e)
            })
        }
    }

    fn seg_count_then_fail(counter: Arc<AtomicU32>, err: CamelError) -> camel_api::OutcomeSegment {
        camel_api::OutcomeSegment::new(Box::new(CountThenFail { counter, err }))
    }

    struct CountThenStop {
        counter: Arc<AtomicU32>,
        mutator: Arc<dyn Fn(&mut Exchange) + Send + Sync>,
    }

    impl OutcomePipeline for CountThenStop {
        fn clone_box(&self) -> Box<dyn OutcomePipeline> {
            Box::new(CountThenStop {
                counter: Arc::clone(&self.counter),
                mutator: Arc::clone(&self.mutator),
            })
        }
        fn run<'a>(
            &'a mut self,
            mut exchange: Exchange,
        ) -> Pin<Box<dyn Future<Output = PipelineOutcome> + Send + 'a>> {
            let c = Arc::clone(&self.counter);
            let m = Arc::clone(&self.mutator);
            Box::pin(async move {
                c.fetch_add(1, Ordering::SeqCst);
                m(&mut exchange);
                PipelineOutcome::Stopped(exchange)
            })
        }
    }

    fn seg_count_then_stop(
        counter: Arc<AtomicU32>,
        mutator: impl Fn(&mut Exchange) + Send + Sync + 'static,
    ) -> camel_api::OutcomeSegment {
        camel_api::OutcomeSegment::new(Box::new(CountThenStop {
            counter,
            mutator: Arc::new(mutator),
        }))
    }

    // ── 5 New tests for DoTrySegment (ADR-0025) ──

    #[tokio::test]
    async fn stop_inside_try_skips_catch_and_finally() {
        let catch_call = Arc::new(AtomicU32::new(0));
        let finally_call = Arc::new(AtomicU32::new(0));

        let mut seg = DoTrySegment {
            try_body: seg_stop_with(|ex| {
                ex.set_property("mutated", camel_api::Value::Bool(true));
            }),
            catches: vec![CatchClauseSegment {
                matcher: CatchMatcher::ByVariant(vec!["*".into()]),
                on_when: None,
                body: seg_record(catch_call.clone()),
                disposition: ExceptionDisposition::Handled,
            }],
            finally: Some(FinallyClauseSegment {
                on_when: None,
                body: seg_record(finally_call.clone()),
            }),
        };

        let result = seg.run(Exchange::default()).await;
        match result {
            PipelineOutcome::Stopped(ex) => {
                assert_eq!(
                    ex.properties.get("mutated"),
                    Some(&camel_api::Value::Bool(true)),
                    "try body mutation must be preserved in Stopped exchange"
                );
            }
            other => panic!("expected Stopped, got {:?}", other),
        }
        assert_eq!(
            catch_call.load(Ordering::SeqCst),
            0,
            "catch must NOT run when try stops"
        );
        assert_eq!(
            finally_call.load(Ordering::SeqCst),
            0,
            "finally must NOT run when try stops"
        );
    }

    #[tokio::test]
    async fn stop_inside_catch_skips_finally() {
        let finally_call = Arc::new(AtomicU32::new(0));

        let mut seg = DoTrySegment {
            try_body: seg_fail(CamelError::ProcessorError("boom".into())),
            catches: vec![CatchClauseSegment {
                matcher: CatchMatcher::ByVariant(vec!["ProcessorError".into()]),
                on_when: None,
                body: seg_stop_with(|ex| {
                    ex.set_property("catch_mutated", camel_api::Value::Bool(true));
                }),
                disposition: ExceptionDisposition::Handled,
            }],
            finally: Some(FinallyClauseSegment {
                on_when: None,
                body: seg_record(finally_call.clone()),
            }),
        };

        let result = seg.run(Exchange::default()).await;
        match result {
            PipelineOutcome::Stopped(ex) => {
                assert_eq!(
                    ex.properties.get("catch_mutated"),
                    Some(&camel_api::Value::Bool(true)),
                    "catch body mutation must be preserved in Stopped exchange"
                );
            }
            other => panic!("expected Stopped, got {:?}", other),
        }
        assert_eq!(
            finally_call.load(Ordering::SeqCst),
            0,
            "finally must NOT run when catch stops"
        );
    }

    #[tokio::test]
    async fn stop_inside_finally_stops_outer_route() {
        let mut seg = DoTrySegment {
            try_body: seg_complete(),
            catches: vec![],
            finally: Some(FinallyClauseSegment {
                on_when: None,
                body: seg_stop_with(|ex| {
                    ex.set_property("finally_mutated", camel_api::Value::Bool(true));
                }),
            }),
        };

        let result = seg.run(Exchange::default()).await;
        match result {
            PipelineOutcome::Stopped(ex) => {
                assert_eq!(
                    ex.properties.get("finally_mutated"),
                    Some(&camel_api::Value::Bool(true)),
                    "finally body mutation must be preserved in Stopped exchange"
                );
            }
            other => panic!("expected Stopped, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn catch_on_when_false_falls_through_to_next_catch() {
        let first_call = Arc::new(AtomicU32::new(0));
        let second_call = Arc::new(AtomicU32::new(0));

        let mut seg = DoTrySegment {
            try_body: seg_fail(CamelError::Io("disk err".into())),
            catches: vec![
                CatchClauseSegment {
                    matcher: CatchMatcher::ByVariant(vec!["Io".into()]),
                    on_when: Some(PredicateSource::Sync(FilterPredicate::new(|_ex| false))),
                    body: seg_record(first_call.clone()),
                    disposition: ExceptionDisposition::Handled,
                },
                CatchClauseSegment {
                    matcher: CatchMatcher::ByVariant(vec!["*".into()]),
                    on_when: None,
                    body: seg_record(second_call.clone()),
                    disposition: ExceptionDisposition::Handled,
                },
            ],
            finally: None,
        };

        let result = seg.run(Exchange::default()).await;
        assert!(
            matches!(result, PipelineOutcome::Completed(_)),
            "expected Completed after second catch"
        );
        assert_eq!(
            first_call.load(Ordering::SeqCst),
            0,
            "first catch must NOT fire (on_when=false)"
        );
        assert_eq!(
            second_call.load(Ordering::SeqCst),
            1,
            "second catch must fire"
        );
    }

    #[tokio::test]
    async fn finally_on_when_false_skips_finally_entirely() {
        let finally_call = Arc::new(AtomicU32::new(0));
        let mut ex = Exchange::default();
        ex.set_property("try_set", camel_api::Value::Bool(true));

        let mut seg = DoTrySegment {
            try_body: seg_complete(),
            catches: vec![],
            finally: Some(FinallyClauseSegment {
                on_when: Some(PredicateSource::Sync(FilterPredicate::new(|_ex| false))),
                body: seg_record(finally_call.clone()),
            }),
        };

        let result = seg.run(ex).await;
        match result {
            PipelineOutcome::Completed(ex) => {
                assert_eq!(
                    ex.properties.get("try_set"),
                    Some(&camel_api::Value::Bool(true)),
                    "exchange state from try must be preserved"
                );
            }
            other => panic!("expected Completed, got {:?}", other),
        }
        assert_eq!(
            finally_call.load(Ordering::SeqCst),
            0,
            "finally must NOT run when on_when=false"
        );
    }

    // ── Catch-body failure envelope (compiled-segment path) ──

    fn catch_fails_segment() -> DoTrySegment {
        DoTrySegment {
            try_body: seg_fail(CamelError::ProcessorError("orig".into())),
            catches: vec![CatchClauseSegment {
                matcher: CatchMatcher::ByVariant(vec!["ProcessorError".into()]),
                on_when: None,
                body: seg_fail(CamelError::Io("catch-fail".into())),
                disposition: ExceptionDisposition::Handled,
            }],
            finally: None,
        }
    }

    #[test]
    fn catch_body_failure_returns_catch_err_and_marks_span() {
        let mut seg = catch_fails_segment();
        let (result, captured, span_records) = capture_debugs_with_span_records(|| {
            // Declared `error` field so record_span_error's record lands.
            let span = tracing::info_span!("dotry_seg_test", error = tracing::field::Empty);
            let _guard = span.enter();
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("current-thread runtime")
                .block_on(async { seg.run(Exchange::default()).await })
        });

        assert!(
            matches!(result, PipelineOutcome::Failed(CamelError::Io(_))),
            "catch failure must surface the catch error as Failed(Io), got: {result:?}"
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
        let original = record_field(&captured, "do_try catch block failed", "original_error")
            .unwrap_or_else(|| {
                panic!(
                    "envelope record missing original_error field under active span, captured: {captured:?}"
                )
            });
        assert!(
            original.contains("orig"),
            "original_error must carry the original error under an active span, got: {original}"
        );
    }

    #[test]
    fn catch_body_failure_emits_envelope_log() {
        let mut seg = catch_fails_segment();
        let (result, captured, _span_records) = capture_debugs_with_span_records(|| {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("current-thread runtime")
                .block_on(async { seg.run(Exchange::default()).await })
        });

        assert!(
            matches!(result, PipelineOutcome::Failed(CamelError::Io(_))),
            "catch failure must surface the catch error as Failed(Io), got: {result:?}"
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
            original.contains("orig"),
            "original_error must carry the original error, got: {original}"
        );
        assert!(
            catch.contains("catch-fail"),
            "catch_error must carry the catch error, got: {catch}"
        );
    }

    // ── Fallible predicate path (language-value-boundary task 1.4) ──

    use camel_api::{ExpressionErrorClass, FilterPredicate, PredicateSource};

    fn expression_failed() -> CamelError {
        CamelError::ExpressionFailed {
            language: "rhai".to_string(),
            route_id: "r1".to_string(),
            step_id: "step#0".to_string(),
            verb: "on_when".to_string(),
            class: ExpressionErrorClass::Runtime,
            position: None,
            conversion: None,
            cause: None,
        }
    }

    fn async_err_predicate(err: CamelError) -> PredicateSource {
        PredicateSource::Async(Arc::new(move |_: &Exchange| {
            let err = err.clone();
            Box::pin(async move { Err(err) }) as camel_api::BoxBoolFuture
        }))
    }

    #[tokio::test]
    async fn finally_on_when_predicate_error_fails_segment() {
        let finally_call = Arc::new(AtomicU32::new(0));
        let mut seg = DoTrySegment {
            try_body: seg_complete(),
            catches: vec![],
            finally: Some(FinallyClauseSegment {
                on_when: Some(async_err_predicate(expression_failed())),
                body: seg_record(finally_call.clone()),
            }),
        };

        let result = seg.run(Exchange::default()).await;
        match result {
            PipelineOutcome::Failed(CamelError::ExpressionFailed { .. }) => {}
            other => panic!("expected Failed(ExpressionFailed), got {other:?}"),
        }
        assert_eq!(
            finally_call.load(Ordering::SeqCst),
            0,
            "finally body must not run when its on_when predicate errors"
        );
    }

    // ── No-catch-matched path (mission 353) ──

    /// doTry with no matching catch must run finally and propagate the
    /// original try error as `Failed` (the segment is the only doTry runtime).
    /// Guards against regressing back to `Completed(bugged_swallow)`.
    #[tokio::test]
    async fn no_clause_matches_runs_finally_and_propagates_original() {
        let catch_call = Arc::new(AtomicU32::new(0));
        let finally_call = Arc::new(AtomicU32::new(0));

        let mut seg = DoTrySegment {
            try_body: seg_fail(CamelError::EndpointCreationFailed(
                "direct endpoint 'direct:does-not-exist' not registered".into(),
            )),
            catches: vec![CatchClauseSegment {
                matcher: CatchMatcher::ByVariant(vec!["Io".into()]),
                on_when: None,
                body: seg_record(catch_call.clone()),
                disposition: ExceptionDisposition::Handled,
            }],
            finally: Some(FinallyClauseSegment {
                on_when: None,
                body: seg_record(finally_call.clone()),
            }),
        };

        let result = seg.run(Exchange::default()).await;
        match result {
            PipelineOutcome::Failed(CamelError::EndpointCreationFailed(msg)) => {
                assert!(
                    msg.contains("does-not-exist"),
                    "propagated error must be the original try error, got: {msg}"
                );
            }
            other => {
                panic!("no catch matched: expected Failed(EndpointCreationFailed), got {other:?}")
            }
        }
        assert_eq!(
            finally_call.load(Ordering::SeqCst),
            1,
            "finally must run exactly once when no catch matches"
        );
        assert_eq!(
            catch_call.load(Ordering::SeqCst),
            0,
            "the unmatched catch body must not run"
        );
    }
    // ── Ported from the deleted Tower doTry service (no coverage loss) ──

    use camel_language_api::{EvalMeta, LanguageError, to_expression_failed};

    fn catch_variant(names: &[&str], body: camel_api::OutcomeSegment) -> CatchClauseSegment {
        catch_variant_with(names, body, ExceptionDisposition::Handled)
    }

    fn catch_variant_with(
        names: &[&str],
        body: camel_api::OutcomeSegment,
        disposition: ExceptionDisposition,
    ) -> CatchClauseSegment {
        CatchClauseSegment {
            matcher: CatchMatcher::ByVariant(names.iter().map(|n| (*n).to_string()).collect()),
            on_when: None,
            body,
            disposition,
        }
    }

    fn finally_seg(body: camel_api::OutcomeSegment) -> FinallyClauseSegment {
        FinallyClauseSegment {
            on_when: None,
            body,
        }
    }

    #[tokio::test]
    async fn happy_path_try_succeeds_finally_runs() {
        let finally_flag = Arc::new(AtomicU32::new(0));
        let mut seg = DoTrySegment {
            try_body: seg_complete(),
            catches: vec![],
            finally: Some(finally_seg(seg_record(finally_flag.clone()))),
        };

        let result = seg.run(Exchange::default()).await;
        assert!(
            matches!(result, PipelineOutcome::Completed(_)),
            "try success must complete, got {result:?}"
        );
        assert_eq!(finally_flag.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn catch_by_variant_handled_returns_ok() {
        let mut seg = DoTrySegment {
            try_body: seg_fail(CamelError::ProcessorError("boom".into())),
            catches: vec![catch_variant(&["ProcessorError"], seg_complete())],
            finally: None,
        };

        let result = seg.run(Exchange::default()).await;
        assert!(
            matches!(result, PipelineOutcome::Completed(_)),
            "Handled must return Completed, got {result:?}"
        );
    }

    #[tokio::test]
    async fn catch_by_variant_propagate_runs_side_effects_and_rethrows() {
        let side_effect = Arc::new(AtomicU32::new(0));
        let mut seg = DoTrySegment {
            try_body: seg_fail(CamelError::ProcessorError("boom".into())),
            catches: vec![catch_variant_with(
                &["ProcessorError"],
                seg_record(side_effect.clone()),
                ExceptionDisposition::Propagate,
            )],
            finally: None,
        };

        let result = seg.run(Exchange::default()).await;
        assert!(
            matches!(
                result,
                PipelineOutcome::Failed(CamelError::ProcessorError(_))
            ),
            "Propagate must rethrow the original error, got {result:?}"
        );
        assert_eq!(
            side_effect.load(Ordering::SeqCst),
            1,
            "catch branch must have run for side-effects"
        );
    }

    #[tokio::test]
    async fn catch_by_predicate_matches_via_exception_kind() {
        let predicate = FilterPredicate::new(|ex: &Exchange| {
            ex.properties
                .get(camel_api::exchange::PROPERTY_EXCEPTION_KIND)
                .map(|v| matches!(v, camel_api::Value::String(s) if s == "io"))
                .unwrap_or(false)
        });
        let mut seg = DoTrySegment {
            try_body: seg_fail(CamelError::Io("disk full".into())),
            catches: vec![CatchClauseSegment {
                matcher: CatchMatcher::Predicate(PredicateSource::Sync(predicate)),
                on_when: None,
                body: seg_complete(),
                disposition: ExceptionDisposition::Handled,
            }],
            finally: None,
        };

        let result = seg.run(Exchange::default()).await;
        assert!(
            matches!(result, PipelineOutcome::Completed(_)),
            "Predicate matcher must catch the error and Handled must return Completed, got {result:?}"
        );
    }

    #[tokio::test]
    async fn first_match_wins_subsequent_clauses_not_evaluated() {
        let first_call = Arc::new(AtomicU32::new(0));
        let second_call = Arc::new(AtomicU32::new(0));
        let mut seg = DoTrySegment {
            try_body: seg_fail(CamelError::Io("err".into())),
            catches: vec![
                catch_variant(&["Io"], seg_record(first_call.clone())),
                catch_variant(&["*"], seg_record(second_call.clone())),
            ],
            finally: None,
        };

        let _ = seg.run(Exchange::default()).await;
        assert_eq!(first_call.load(Ordering::SeqCst), 1);
        assert_eq!(second_call.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn catch_branch_throws_new_error_wins() {
        let mut seg = DoTrySegment {
            try_body: seg_fail(CamelError::ProcessorError("orig".into())),
            catches: vec![catch_variant(
                &["ProcessorError"],
                seg_fail(CamelError::Io("catch-fail".into())),
            )],
            finally: None,
        };

        let result = seg.run(Exchange::default()).await;
        assert!(
            matches!(result, PipelineOutcome::Failed(CamelError::Io(_))),
            "catch error must win over the try error, got {result:?}"
        );
    }

    #[tokio::test]
    async fn finally_throws_with_no_previous_error_propagates_finally_error() {
        let mut seg = DoTrySegment {
            try_body: seg_complete(),
            catches: vec![],
            finally: Some(finally_seg(seg_fail(CamelError::Config("fin".into())))),
        };

        let result = seg.run(Exchange::default()).await;
        assert!(
            matches!(result, PipelineOutcome::Failed(CamelError::Config(_))),
            "finally error must propagate when no previous error exists, got {result:?}"
        );
    }

    #[tokio::test]
    async fn finally_throws_with_previous_error_restores_previous() {
        let mut seg = DoTrySegment {
            try_body: seg_fail(CamelError::ProcessorError("orig".into())),
            catches: vec![],
            finally: Some(finally_seg(seg_fail(CamelError::Config("fin".into())))),
        };

        let result = seg.run(Exchange::default()).await;
        assert!(
            matches!(
                result,
                PipelineOutcome::Failed(CamelError::ProcessorError(_))
            ),
            "previous error must be restored when finally throws (Camel parity), got {result:?}"
        );
    }

    #[tokio::test]
    async fn stale_handled_marker_cleared_on_entry() {
        let mut ex = Exchange::default();
        ex.set_property(
            camel_api::exchange::PROPERTY_EXCEPTION_HANDLED,
            camel_api::Value::Bool(true),
        );
        ex.set_error(CamelError::ProcessorError("stale".into()));
        let mut seg = DoTrySegment {
            try_body: seg_complete(),
            catches: vec![],
            finally: None,
        };

        let result = seg.run(ex).await;
        match result {
            PipelineOutcome::Completed(ex) => assert!(
                !ex.properties
                    .contains_key(camel_api::exchange::PROPERTY_EXCEPTION_HANDLED),
                "stale CamelExceptionHandled must be cleared on entry"
            ),
            other => panic!("expected Completed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn nested_do_try_inner_catch_does_not_leak_to_outer() {
        let inner = DoTrySegment {
            try_body: seg_fail(CamelError::Io("inner".into())),
            catches: vec![catch_variant(&["Io"], seg_complete())],
            finally: None,
        };
        let mut outer = DoTrySegment {
            try_body: camel_api::OutcomeSegment::new(Box::new(inner)),
            catches: vec![catch_variant(&["Io"], seg_complete())],
            finally: None,
        };

        let result = outer.run(Exchange::default()).await;
        assert!(
            matches!(result, PipelineOutcome::Completed(_)),
            "outer must see Completed because inner handled its own error, got {result:?}"
        );
    }

    #[tokio::test]
    async fn catch_all_only_fires_when_no_specific_clause_matches() {
        let processor_call = Arc::new(AtomicU32::new(0));
        let catch_all_call = Arc::new(AtomicU32::new(0));
        let mut seg = DoTrySegment {
            try_body: seg_fail(CamelError::Io("err".into())),
            catches: vec![
                catch_variant(&["ProcessorError"], seg_record(processor_call.clone())),
                catch_variant(&["*"], seg_record(catch_all_call.clone())),
            ],
            finally: None,
        };

        let _ = seg.run(Exchange::default()).await;
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

    /// Java try/catch/finally parity (JLS §14.20.2): a failing catch body
    /// does not skip finally, and the catch error remains the surfaced
    /// failure. This rebuilds the catch-and-finally coverage lost with the
    /// deleted builder-only `DoTryService`.
    #[tokio::test]
    async fn catch_body_failure_runs_finally_once_then_returns_catch_err() {
        let finally_flag = Arc::new(AtomicU32::new(0));
        let mut seg = DoTrySegment {
            try_body: seg_fail(CamelError::ProcessorError("orig".into())),
            catches: vec![catch_variant(
                &["ProcessorError"],
                seg_fail(CamelError::Io("catch-fail".into())),
            )],
            finally: Some(finally_seg(seg_record(finally_flag.clone()))),
        };

        let result = seg.run(Exchange::default()).await;
        assert!(
            matches!(result, PipelineOutcome::Failed(CamelError::Io(_))),
            "catch_err must propagate (not original ProcessorError), got {result:?}"
        );
        assert_eq!(
            finally_flag.load(Ordering::SeqCst),
            1,
            "finally must run exactly once even when the catch body fails (Java try/catch/finally parity)"
        );
    }

    /// Real exercise of the finally/catch parity rule: the finally body runs
    /// (and throws) even when the catch body fails, and the catch error still
    /// wins over the finally error (Camel parity: a throwing finally restores
    /// the previous error). Before the catch-body-failure path ran finally,
    /// this test passed vacuously.
    #[tokio::test]
    async fn catch_throws_and_throwing_finally_still_returns_catch_err() {
        let finally_flag = Arc::new(AtomicU32::new(0));
        let mut seg = DoTrySegment {
            try_body: seg_fail(CamelError::ProcessorError("orig".into())),
            catches: vec![catch_variant(
                &["ProcessorError"],
                seg_fail(CamelError::Io("catch-fail".into())),
            )],
            finally: Some(finally_seg(seg_count_then_fail(
                finally_flag.clone(),
                CamelError::Config("fin-fail".into()),
            ))),
        };

        let result = seg.run(Exchange::default()).await;
        assert!(
            matches!(result, PipelineOutcome::Failed(CamelError::Io(_))),
            "catch_err (Io) must be returned over finally_err (Config), got {result:?}"
        );
        assert_eq!(
            finally_flag.load(Ordering::SeqCst),
            1,
            "finally must run exactly once even when its own body fails"
        );
    }

    /// Stop in finally takes precedence over the pending catch failure: the
    /// outer route stops with the finally exchange (mutation preserved), and
    /// the finally body still ran exactly once. Pins the `Stopped` arm of
    /// `fail_after_finally` on the catch-body-failure path.
    #[tokio::test]
    async fn catch_body_failure_then_finally_stops_stops_outer_route() {
        let finally_flag = Arc::new(AtomicU32::new(0));
        let mut seg = DoTrySegment {
            try_body: seg_fail(CamelError::ProcessorError("orig".into())),
            catches: vec![catch_variant(
                &["ProcessorError"],
                seg_fail(CamelError::Io("catch-fail".into())),
            )],
            finally: Some(finally_seg(seg_count_then_stop(
                finally_flag.clone(),
                |ex| {
                    ex.set_property("finally_stopped_marker", camel_api::Value::Bool(true));
                },
            ))),
        };

        let result = seg.run(Exchange::default()).await;
        match result {
            PipelineOutcome::Stopped(ex) => {
                assert_eq!(
                    ex.properties.get("finally_stopped_marker"),
                    Some(&camel_api::Value::Bool(true)),
                    "finally exchange mutation must be preserved in Stopped"
                );
            }
            other => panic!("expected Stopped, got {other:?}"),
        }
        assert_eq!(
            finally_flag.load(Ordering::SeqCst),
            1,
            "finally must run exactly once before the outer route stops"
        );
    }

    #[tokio::test]
    async fn finally_on_when_false_with_previous_error_still_propagates_original() {
        let finally_flag = Arc::new(AtomicU32::new(0));
        let mut seg = DoTrySegment {
            try_body: seg_fail(CamelError::ProcessorError("orig".into())),
            catches: vec![],
            finally: Some(FinallyClauseSegment {
                on_when: Some(PredicateSource::Sync(FilterPredicate::new(|_ex| false))),
                body: seg_record(finally_flag.clone()),
            }),
        };

        let result = seg.run(Exchange::default()).await;
        assert!(
            matches!(
                result,
                PipelineOutcome::Failed(CamelError::ProcessorError(_))
            ),
            "original error must propagate even when finally_on_when skips finally, got {result:?}"
        );
        assert_eq!(
            finally_flag.load(Ordering::SeqCst),
            0,
            "doFinally must NOT run when on_when returns false"
        );
    }

    #[tokio::test]
    async fn catch_throws_under_propagate_disposition_returns_catch_err() {
        let mut seg = DoTrySegment {
            try_body: seg_fail(CamelError::ProcessorError("orig".into())),
            catches: vec![catch_variant_with(
                &["ProcessorError"],
                seg_fail(CamelError::Io("catch-fail".into())),
                ExceptionDisposition::Propagate,
            )],
            finally: None,
        };

        let result = seg.run(Exchange::default()).await;
        assert!(
            matches!(result, PipelineOutcome::Failed(CamelError::Io(_))),
            "Propagate-disposition catch failure must surface the catch error, got {result:?}"
        );
    }

    #[tokio::test]
    async fn catch_when_predicate_error_chains_original() {
        let mut seg = DoTrySegment {
            try_body: seg_fail(CamelError::ProcessorError("boom".into())),
            catches: vec![CatchClauseSegment {
                matcher: CatchMatcher::Predicate(async_err_predicate(expression_failed())),
                on_when: None,
                body: seg_complete(),
                disposition: ExceptionDisposition::Handled,
            }],
            finally: None,
        };

        let result = seg.run(Exchange::default()).await;
        match result {
            PipelineOutcome::Failed(CamelError::ExpressionFailed {
                cause: Some(cause), ..
            }) => {
                assert!(
                    cause.to_string().contains("boom"),
                    "cause must carry the original error, got: {cause}"
                );
            }
            other => panic!("expected Failed(ExpressionFailed) with chained cause, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn catch_on_when_predicate_error_chains_original() {
        let mut seg = DoTrySegment {
            try_body: seg_fail(CamelError::ProcessorError("boom".into())),
            catches: vec![CatchClauseSegment {
                matcher: CatchMatcher::ByVariant(vec!["*".into()]),
                on_when: Some(async_err_predicate(expression_failed())),
                body: seg_complete(),
                disposition: ExceptionDisposition::Handled,
            }],
            finally: None,
        };

        let result = seg.run(Exchange::default()).await;
        match result {
            PipelineOutcome::Failed(CamelError::ExpressionFailed {
                cause: Some(cause), ..
            }) => {
                assert!(
                    cause.to_string().contains("boom"),
                    "cause must carry the original error, got: {cause}"
                );
            }
            other => panic!("expected Failed(ExpressionFailed) with chained cause, got {other:?}"),
        }
    }

    // ── Mapper-generated cause replacement (ported from do_try.rs) ──

    fn caller_diagnostic_failed(cause: CamelError) -> CamelError {
        CamelError::ExpressionFailed {
            language: "rhai".to_string(),
            route_id: "r1".to_string(),
            step_id: "s1".to_string(),
            verb: "when".to_string(),
            class: ExpressionErrorClass::Runtime,
            position: None,
            conversion: None,
            cause: Some(Box::new(cause)),
        }
    }

    #[test]
    fn chain_replaces_mapper_generated_cause() {
        let meta = EvalMeta {
            language: "xpath".into(),
            route_id: "r1".into(),
            step_id: "s1".into(),
            verb: "when".into(),
            target: None,
        };
        let mapper_err = to_expression_failed(
            LanguageError::EvalFailure {
                class: ExpressionErrorClass::Parse,
                position: None,
                detail: Some("body is not valid XML".into()),
            },
            &meta,
        );

        let result = chain_predicate_error(mapper_err, CamelError::ProcessorError("boom".into()));

        match result {
            CamelError::ExpressionFailed {
                cause: Some(cause), ..
            } => {
                assert!(
                    cause.to_string().contains("boom"),
                    "mapper diagnostic cause must be replaced by the original, got: {cause}"
                );
            }
            other => panic!("expected ExpressionFailed with chained cause, got {other:?}"),
        }
    }

    #[test]
    fn chain_preserves_caller_cause() {
        let predicate_err =
            caller_diagnostic_failed(CamelError::ProcessorError("caller diagnostic".into()));

        let result =
            chain_predicate_error(predicate_err, CamelError::ProcessorError("boom".into()));

        match result {
            CamelError::ExpressionFailed {
                cause: Some(cause), ..
            } => {
                let rendered = cause.to_string();
                assert!(
                    rendered.contains("caller diagnostic"),
                    "caller cause must pass through untouched, got: {rendered}"
                );
                assert!(
                    !rendered.contains("boom"),
                    "caller cause must not be replaced, got: {rendered}"
                );
            }
            other => panic!("expected ExpressionFailed with caller cause, got {other:?}"),
        }
    }

    #[test]
    fn chain_preserves_caller_source_bearing_cause() {
        let predicate_err = caller_diagnostic_failed(CamelError::ProcessorErrorWithSource(
            "caller diagnostic".into(),
            Arc::new(std::io::Error::other("caller source")),
        ));

        let result =
            chain_predicate_error(predicate_err, CamelError::ProcessorError("boom".into()));

        match result {
            CamelError::ExpressionFailed {
                cause: Some(cause), ..
            } => {
                let rendered = cause.to_string();
                assert!(
                    rendered.contains("caller diagnostic"),
                    "caller cause must pass through untouched, got: {rendered}"
                );
                let source = std::error::Error::source(cause.as_ref())
                    .unwrap_or_else(|| panic!("caller source must survive, cause: {cause:?}"));
                assert!(
                    source.to_string().contains("caller source"),
                    "caller source must survive untouched, got: {source}"
                );
            }
            other => panic!("expected ExpressionFailed with caller cause, got {other:?}"),
        }
    }
}
