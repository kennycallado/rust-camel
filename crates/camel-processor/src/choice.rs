use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use tower::Service;
use tower::ServiceExt;

use camel_api::{BoxProcessor, CamelError, Exchange, PredicateSource};

/// A single when-clause: a fallible predicate + the sub-pipeline to execute
/// when it matches.
pub struct WhenClause {
    pub predicate: PredicateSource,
    pub pipeline: BoxProcessor,
}

impl Clone for WhenClause {
    fn clone(&self) -> Self {
        Self {
            predicate: self.predicate.clone(),
            pipeline: self.pipeline.clone(),
        }
    }
}

/// Tower Service implementing the Choice EIP (Content-Based Router).
///
/// Evaluates `when` clauses in order. The first matching predicate routes the
/// exchange through its sub-pipeline. If no predicate matches, the `otherwise`
/// pipeline is used (if present); otherwise the exchange passes through
/// unchanged. A failed predicate evaluation propagates as `Err` instead of
/// falling through to `otherwise`.
#[derive(Clone)]
pub struct ChoiceService {
    whens: Vec<WhenClause>,
    otherwise: Option<BoxProcessor>,
}

impl ChoiceService {
    /// Create a new `ChoiceService`.
    ///
    /// `whens` — ordered list of `(predicate, sub_pipeline)` pairs.
    /// `otherwise` — optional fallback pipeline (executed when no `when` matches).
    pub fn new(whens: Vec<WhenClause>, otherwise: Option<BoxProcessor>) -> Self {
        Self { whens, otherwise }
    }
}

impl Service<Exchange> for ChoiceService {
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, exchange: Exchange) -> Self::Future {
        // Clone-and-replace: the future owns its state so predicates can be
        // awaited in order before routing.
        let whens = self.whens.clone();
        let otherwise = self.otherwise.clone();
        Box::pin(async move {
            let mut whens = whens;
            for when in whens.iter_mut() {
                if when.predicate.matches(&exchange).await? {
                    return when.pipeline.ready().await?.call(exchange).await;
                }
            }
            if let Some(mut otherwise) = otherwise {
                otherwise.ready().await?.call(exchange).await
            } else {
                Ok(exchange)
            }
        })
    }
}

// ── ChoiceSegment + WhenClauseSegment (ADR-0025 OutcomePipeline) ──────────

/// Outcome-aware structural EIP segment for a single when clause.
pub struct WhenClauseSegment {
    pub predicate: camel_api::PredicateSource,
    pub body: camel_api::OutcomeSegment,
}

impl Clone for WhenClauseSegment {
    fn clone(&self) -> Self {
        Self {
            predicate: self.predicate.clone(),
            body: self.body.clone(),
        }
    }
}

/// Outcome-aware structural EIP segment for the Choice pattern.
///
/// Evaluates `when` clauses in order. The first matching predicate runs its
/// `body` (which can return `Completed`, `Stopped`, or `Failed`). If no
/// predicate matches, the `otherwise` segment runs (if present); otherwise
/// returns `Completed(original_exchange)`. A failed predicate evaluation
/// yields `Failed(err)` instead of falling through to `otherwise`.
///
/// Unlike `ChoiceService` (which operates at the Tower layer and cannot
/// preserve `Stopped(ex)` with mutations), `ChoiceSegment` operates at the
/// `PipelineOutcome` layer and preserves the exchange at the Stop point
/// including all mutations.
pub struct ChoiceSegment {
    pub clauses: Vec<WhenClauseSegment>,
    pub otherwise: Option<camel_api::OutcomeSegment>,
}

impl Clone for ChoiceSegment {
    fn clone(&self) -> Self {
        Self {
            clauses: self
                .clauses
                .iter()
                .map(|c| WhenClauseSegment {
                    predicate: c.predicate.clone(),
                    body: c.body.clone(),
                })
                .collect(),
            otherwise: self.otherwise.clone(),
        }
    }
}

impl camel_api::OutcomePipeline for ChoiceSegment {
    fn clone_box(&self) -> Box<dyn camel_api::OutcomePipeline> {
        Box::new(self.clone())
    }

    fn run<'a>(
        &'a mut self,
        exchange: camel_api::Exchange,
    ) -> Pin<Box<dyn Future<Output = camel_api::PipelineOutcome> + Send + 'a>> {
        Box::pin(async move {
            for clause in self.clauses.iter_mut() {
                match clause.predicate.matches(&exchange).await {
                    Ok(true) => return clause.body.run(exchange).await,
                    Ok(false) => continue,
                    Err(err) => return camel_api::PipelineOutcome::Failed(err),
                }
            }
            if let Some(otherwise) = self.otherwise.as_mut() {
                otherwise.run(exchange).await
            } else {
                camel_api::PipelineOutcome::Completed(exchange)
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_api::{Body, BoxProcessorExt, Message, Value};
    use tower::ServiceExt;

    fn append_body(suffix: &'static str) -> BoxProcessor {
        BoxProcessor::from_fn(move |mut ex: Exchange| {
            Box::pin(async move {
                if let Body::Text(s) = &ex.input.body {
                    ex.input.body = Body::Text(format!("{s}{suffix}"));
                }
                Ok(ex)
            })
        })
    }

    fn failing() -> BoxProcessor {
        BoxProcessor::from_fn(|_ex| {
            Box::pin(async { Err(CamelError::ProcessorError("boom".into())) })
        })
    }

    fn pred_header(name: &'static str) -> PredicateSource {
        PredicateSource::Sync(FilterPredicate::new(move |ex: &Exchange| {
            ex.input.header(name).is_some()
        }))
    }

    // 1. First matching when executes its pipeline.
    #[tokio::test]
    async fn test_choice_first_when_matches() {
        let whens = vec![
            WhenClause {
                predicate: pred_header("a"),
                pipeline: append_body("-A"),
            },
            WhenClause {
                predicate: pred_header("b"),
                pipeline: append_body("-B"),
            },
        ];
        let mut svc = ChoiceService::new(whens, None);
        let mut ex = Exchange::new(Message::new("x"));
        ex.input.set_header("a", Value::Bool(true));
        let result = svc.ready().await.unwrap().call(ex).await.unwrap();
        assert_eq!(result.input.body.as_text(), Some("x-A"));
    }

    // 2. Second when executes when first does not match.
    #[tokio::test]
    async fn test_choice_second_when_matches() {
        let whens = vec![
            WhenClause {
                predicate: pred_header("a"),
                pipeline: append_body("-A"),
            },
            WhenClause {
                predicate: pred_header("b"),
                pipeline: append_body("-B"),
            },
        ];
        let mut svc = ChoiceService::new(whens, None);
        let mut ex = Exchange::new(Message::new("x"));
        ex.input.set_header("b", Value::Bool(true));
        let result = svc.ready().await.unwrap().call(ex).await.unwrap();
        assert_eq!(result.input.body.as_text(), Some("x-B"));
    }

    // 3. Only the FIRST matching when fires (short-circuit — both a and b present).
    #[tokio::test]
    async fn test_choice_short_circuits_at_first_match() {
        let whens = vec![
            WhenClause {
                predicate: pred_header("a"),
                pipeline: append_body("-A"),
            },
            WhenClause {
                predicate: pred_header("b"),
                pipeline: append_body("-B"),
            },
        ];
        let mut svc = ChoiceService::new(whens, None);
        let mut ex = Exchange::new(Message::new("x"));
        ex.input.set_header("a", Value::Bool(true));
        ex.input.set_header("b", Value::Bool(true));
        let result = svc.ready().await.unwrap().call(ex).await.unwrap();
        assert_eq!(result.input.body.as_text(), Some("x-A"));
    }

    // 4. Otherwise executes when no when matches.
    #[tokio::test]
    async fn test_choice_otherwise_fires_when_no_when_matches() {
        let whens = vec![WhenClause {
            predicate: pred_header("a"),
            pipeline: append_body("-A"),
        }];
        let mut svc = ChoiceService::new(whens, Some(append_body("-else")));
        let ex = Exchange::new(Message::new("x"));
        let result = svc.ready().await.unwrap().call(ex).await.unwrap();
        assert_eq!(result.input.body.as_text(), Some("x-else"));
    }

    // 5. No match and no otherwise → exchange passes unchanged.
    #[tokio::test]
    async fn test_choice_no_match_no_otherwise_passthrough() {
        let whens = vec![WhenClause {
            predicate: pred_header("a"),
            pipeline: append_body("-A"),
        }];
        let mut svc = ChoiceService::new(whens, None);
        let ex = Exchange::new(Message::new("untouched"));
        let result = svc.ready().await.unwrap().call(ex).await.unwrap();
        assert_eq!(result.input.body.as_text(), Some("untouched"));
    }

    // 6. Errors in a matching when's pipeline propagate.
    #[tokio::test]
    async fn test_choice_error_in_when_propagates() {
        let whens = vec![WhenClause {
            predicate: pred_header("a"),
            pipeline: failing(),
        }];
        let mut svc = ChoiceService::new(whens, None);
        let mut ex = Exchange::new(Message::new("x"));
        ex.input.set_header("a", Value::Bool(true));
        let result = svc.ready().await.unwrap().call(ex).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("boom"));
    }

    // 7. Errors in otherwise pipeline propagate.
    #[tokio::test]
    async fn test_choice_error_in_otherwise_propagates() {
        let mut svc = ChoiceService::new(vec![], Some(failing()));
        let ex = Exchange::new(Message::new("x"));
        let result = svc.ready().await.unwrap().call(ex).await;
        assert!(result.is_err());
    }

    // ── Fallible predicate path (language-value-boundary task 1.4) ──

    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};

    use camel_api::FilterPredicate;
    use camel_api::outcome_pipeline::OutcomePipeline as _;
    use camel_api::{ExpressionErrorClass, PipelineOutcome, PredicateSource};

    fn expression_failed() -> CamelError {
        CamelError::ExpressionFailed {
            language: "rhai".to_string(),
            route_id: "r1".to_string(),
            step_id: "step#0".to_string(),
            verb: "choice".to_string(),
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

    struct RecordRun(Arc<AtomicU32>);

    impl camel_api::OutcomePipeline for RecordRun {
        fn clone_box(&self) -> Box<dyn camel_api::OutcomePipeline> {
            Box::new(RecordRun(Arc::clone(&self.0)))
        }
        fn run<'a>(
            &'a mut self,
            exchange: Exchange,
        ) -> Pin<Box<dyn Future<Output = PipelineOutcome> + Send + 'a>> {
            let c = Arc::clone(&self.0);
            Box::pin(async move {
                c.fetch_add(1, Ordering::SeqCst);
                PipelineOutcome::Completed(exchange)
            })
        }
    }

    #[tokio::test]
    async fn choice_when_predicate_error_fails_step() {
        let body_calls = Arc::new(AtomicU32::new(0));
        let otherwise_calls = Arc::new(AtomicU32::new(0));
        let mut seg = ChoiceSegment {
            clauses: vec![WhenClauseSegment {
                predicate: async_err_predicate(expression_failed()),
                body: camel_api::OutcomeSegment::new(Box::new(RecordRun(Arc::clone(&body_calls)))),
            }],
            otherwise: Some(camel_api::OutcomeSegment::new(Box::new(RecordRun(
                Arc::clone(&otherwise_calls),
            )))),
        };
        let outcome = seg.run(Exchange::default()).await;
        match outcome {
            PipelineOutcome::Failed(err) => {
                assert!(
                    matches!(err, CamelError::ExpressionFailed { .. }),
                    "expected ExpressionFailed, got {err:?}"
                );
            }
            other => panic!("expected PipelineOutcome::Failed, got {other:?}"),
        }
        assert_eq!(
            body_calls.load(Ordering::SeqCst),
            0,
            "when body must not run when its predicate errors"
        );
        assert_eq!(
            otherwise_calls.load(Ordering::SeqCst),
            0,
            "otherwise must never execute when a when-predicate errors"
        );
    }
}
