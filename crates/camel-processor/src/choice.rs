use std::future::Future;
use std::pin::Pin;

use camel_api::{BoxProcessor, PredicateSource};

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
/// The segment operates at the `PipelineOutcome` layer and preserves the
/// exchange at the Stop point including all mutations.
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
    use crate::test_processor_segment::processor_segment;
    use camel_api::{
        Body, BoxProcessorExt, CamelError, Exchange, FilterPredicate, Message, OutcomePipeline,
        PipelineOutcome, Value,
    };

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

    fn clause(predicate: PredicateSource, body: BoxProcessor) -> WhenClauseSegment {
        WhenClauseSegment {
            predicate,
            body: processor_segment(body),
        }
    }

    fn completed_exchange(outcome: PipelineOutcome) -> Exchange {
        match outcome {
            PipelineOutcome::Completed(ex) => ex,
            other => panic!("expected PipelineOutcome::Completed, got {other:?}"),
        }
    }

    // 1. First matching when executes its body.
    #[tokio::test]
    async fn choice_segment_first_when_matches() {
        let mut seg = ChoiceSegment {
            clauses: vec![
                clause(pred_header("a"), append_body("-A")),
                clause(pred_header("b"), append_body("-B")),
            ],
            otherwise: None,
        };
        let mut ex = Exchange::new(Message::new("x"));
        ex.input.set_header("a", Value::Bool(true));
        let result = completed_exchange(seg.run(ex).await);
        assert_eq!(result.input.body.as_text(), Some("x-A"));
    }

    // 2. Second when executes when first does not match.
    #[tokio::test]
    async fn choice_segment_second_when_matches() {
        let mut seg = ChoiceSegment {
            clauses: vec![
                clause(pred_header("a"), append_body("-A")),
                clause(pred_header("b"), append_body("-B")),
            ],
            otherwise: None,
        };
        let mut ex = Exchange::new(Message::new("x"));
        ex.input.set_header("b", Value::Bool(true));
        let result = completed_exchange(seg.run(ex).await);
        assert_eq!(result.input.body.as_text(), Some("x-B"));
    }

    // 3. Only the FIRST matching when fires (short-circuit — both a and b present).
    #[tokio::test]
    async fn choice_segment_short_circuits_at_first_match() {
        let mut seg = ChoiceSegment {
            clauses: vec![
                clause(pred_header("a"), append_body("-A")),
                clause(pred_header("b"), append_body("-B")),
            ],
            otherwise: None,
        };
        let mut ex = Exchange::new(Message::new("x"));
        ex.input.set_header("a", Value::Bool(true));
        ex.input.set_header("b", Value::Bool(true));
        let result = completed_exchange(seg.run(ex).await);
        assert_eq!(result.input.body.as_text(), Some("x-A"));
    }

    // 4. Otherwise executes when no when matches.
    #[tokio::test]
    async fn choice_segment_otherwise_fires_when_no_when_matches() {
        let mut seg = ChoiceSegment {
            clauses: vec![clause(pred_header("a"), append_body("-A"))],
            otherwise: Some(processor_segment(append_body("-else"))),
        };
        let ex = Exchange::new(Message::new("x"));
        let result = completed_exchange(seg.run(ex).await);
        assert_eq!(result.input.body.as_text(), Some("x-else"));
    }

    // 5. No match and no otherwise → exchange passes unchanged.
    #[tokio::test]
    async fn choice_segment_no_match_no_otherwise_passthrough() {
        let mut seg = ChoiceSegment {
            clauses: vec![clause(pred_header("a"), append_body("-A"))],
            otherwise: None,
        };
        let ex = Exchange::new(Message::new("untouched"));
        let result = completed_exchange(seg.run(ex).await);
        assert_eq!(result.input.body.as_text(), Some("untouched"));
    }

    // 6. Errors in a matching when's body surface as Failed.
    #[tokio::test]
    async fn choice_segment_error_in_when_propagates() {
        let mut seg = ChoiceSegment {
            clauses: vec![clause(pred_header("a"), failing())],
            otherwise: None,
        };
        let mut ex = Exchange::new(Message::new("x"));
        ex.input.set_header("a", Value::Bool(true));
        match seg.run(ex).await {
            PipelineOutcome::Failed(err) => {
                assert!(err.to_string().contains("boom"), "unexpected error: {err}");
            }
            other => panic!("expected PipelineOutcome::Failed, got {other:?}"),
        }
    }

    // 7. Errors in otherwise surface as Failed.
    #[tokio::test]
    async fn choice_segment_error_in_otherwise_propagates() {
        let mut seg = ChoiceSegment {
            clauses: vec![],
            otherwise: Some(processor_segment(failing())),
        };
        let ex = Exchange::new(Message::new("x"));
        assert!(matches!(seg.run(ex).await, PipelineOutcome::Failed(_)));
    }

    // ── Fallible predicate path (language-value-boundary task 1.4) ──

    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};

    use camel_api::ExpressionErrorClass;

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
