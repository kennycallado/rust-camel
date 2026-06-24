//! ## Stop semantics (ADR-0025)
//!
//! This segment implements `OutcomePipeline` and propagates `PipelineOutcome::Stopped(ex)`
//! with the exchange state intact. See ADR-0025 §3.

use crate::do_try::CatchMatcher;
use camel_api::error_handler::ExceptionDisposition;
use camel_api::outcome_pipeline::OutcomePipeline;
use camel_api::pipeline_outcome::PipelineOutcome;
use camel_api::{CamelError, Exchange, FilterPredicate};
use std::future::Future;
use std::pin::Pin;

/// - `Handled`: the exchange continues through finally and downstream.
/// - `Propagate`: the catch body runs for side effects, finally runs with the
///   catch body's exchange, and the original try-error re-throws as `Failed`.
#[derive(Clone)]
pub struct CatchClauseSegment {
    pub matcher: CatchMatcher,
    pub on_when: Option<FilterPredicate>,
    pub body: camel_api::OutcomeSegment,
    pub disposition: ExceptionDisposition,
}

/// Compilable segment for a `doFinally` clause within a `DoTrySegment`.
#[derive(Clone)]
pub struct FinallyClauseSegment {
    pub on_when: Option<FilterPredicate>,
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
/// This is a private, transient type — the large-enum-variant lint fires
/// because Exchange is a fat struct, but boxing it here would just add
/// allocation churn in a hot path where the outcome is immediately consumed
/// at the call site.
#[allow(clippy::large_enum_variant)]
enum FinallyOutcome {
    Stopped(Exchange),
    Failed(CamelError),
}

/// Run the finally body if present and on_when permits.
/// Free function (not a method) to avoid borrow conflict with
/// `self.catches.iter_mut()` inside the catch loop.
async fn run_finally_body(
    finally: &mut Option<FinallyClauseSegment>,
    ex: Exchange,
) -> Result<Exchange, FinallyOutcome> {
    let Some(f) = finally.as_mut() else {
        return Ok(ex);
    };
    if !f.on_when.as_ref().map(|p| p(&ex)).unwrap_or(true) {
        return Ok(ex);
    }
    match f.body.run(ex).await {
        PipelineOutcome::Completed(e) => Ok(e),
        PipelineOutcome::Stopped(e) => Err(FinallyOutcome::Stopped(e)),
        PipelineOutcome::Failed(e) => Err(FinallyOutcome::Failed(e)),
    }
}

impl OutcomePipeline for DoTrySegment {
    fn clone_box(&self) -> Box<dyn OutcomePipeline> {
        Box::new(self.clone())
    }

    fn run<'a>(
        &'a mut self,
        exchange: Exchange,
    ) -> Pin<Box<dyn Future<Output = PipelineOutcome> + Send + 'a>> {
        Box::pin(async move {
            // Pre-clone for the "no catch matched" path; `exchange` itself is
            // consumed by `try_body.run(...)` below.
            let exchange_for_unmatched = exchange.clone();

            // 1. Run try_body. If Stopped, return immediately (skip catch AND finally).
            let try_outcome = self.try_body.run(exchange).await;
            let returned_ex = match try_outcome {
                PipelineOutcome::Stopped(ex) => return PipelineOutcome::Stopped(ex),
                PipelineOutcome::Completed(ex) => ex,
                PipelineOutcome::Failed(err) => {
                    // 2. try failed — try each catch in order. Walk the catch
                    // chain starting from the failed try-exchange. On the first
                    // matching catch whose `on_when` is true (or unset), run it.
                    let mut current_ex = exchange_for_unmatched;
                    current_ex.set_error(err.clone());
                    for catch in self.catches.iter_mut() {
                        if catch.matcher.matches(&err, &current_ex)
                            && catch
                                .on_when
                                .as_ref()
                                .map(|p| p(&current_ex))
                                .unwrap_or(true)
                        {
                            match catch.body.run(current_ex).await {
                                // Stop in catch: skip finally AND outer route halts.
                                PipelineOutcome::Stopped(stopped_ex) => {
                                    return PipelineOutcome::Stopped(stopped_ex);
                                }
                                // Catch handled — exit catch chain with this exchange.
                                PipelineOutcome::Completed(next) => {
                                    match catch.disposition {
                                        ExceptionDisposition::Propagate => {
                                            // Run finally with the exchange from the catch body,
                                            // then propagate the original try error.
                                            match run_finally_body(&mut self.finally, next).await {
                                                Ok(_) => {}
                                                Err(FinallyOutcome::Stopped(e)) => {
                                                    return PipelineOutcome::Stopped(e);
                                                }
                                                Err(FinallyOutcome::Failed(_finally_err)) => {
                                                    tracing::warn!(
                                                        error = %err,
                                                        "doFinally threw during Propagate; \
                                                         restoring original"
                                                    );
                                                    return PipelineOutcome::Failed(err);
                                                }
                                            }
                                            return PipelineOutcome::Failed(err);
                                        }
                                        ExceptionDisposition::Handled => {
                                            current_ex = next;
                                            break;
                                        }
                                        ExceptionDisposition::Continued => {
                                            current_ex = next;
                                            break;
                                        }
                                    }
                                }
                                // Catch-body Failed: surface THAT error to outer
                                // route. Per ADR-0025 invariant #4 ("doTry is a
                                // local error-handler island"), a failing catch
                                // body propagates as Failed — it does NOT re-enter
                                // the catch chain (no recursive catch-of-catch in
                                // Camel). Skip remaining catches and finally.
                                PipelineOutcome::Failed(catch_err) => {
                                    return PipelineOutcome::Failed(catch_err);
                                }
                            }
                        }
                    }
                    current_ex
                }
            };
            // 3. Run finally if present (skip if try/catch returned Stopped —
            // already returned above; skip if catch-body Failed — also returned).
            match run_finally_body(&mut self.finally, returned_ex).await {
                Ok(ex) => PipelineOutcome::Completed(ex),
                Err(FinallyOutcome::Stopped(e)) => PipelineOutcome::Stopped(e),
                Err(FinallyOutcome::Failed(finally_err)) => {
                    tracing::warn!(
                        error = %finally_err,
                        "doFinally threw during/after catch; surfacing finally error"
                    );
                    PipelineOutcome::Failed(finally_err)
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::do_try::CatchMatcher;
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
                    on_when: Some(Arc::new(|_ex| false)),
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
                on_when: Some(Arc::new(|_ex| false)),
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
}
