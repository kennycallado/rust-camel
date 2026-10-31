use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use camel_api::{BoxProcessor, CamelError, PredicateSource};

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

// ── ChoiceGroupSegment (grouped when clauses) ─────────────────────────────

/// Outcome-aware structural EIP segment for grouped `when` clauses that share
/// one language.
///
/// Grouped evaluation of same-language `when` clauses — one group evaluation
/// per message replaces per-clause `PredicateSource::matches`. Clause order,
/// first-match, and error semantics are identical to [`ChoiceSegment`].
pub struct ChoiceGroupSegment {
    /// The shared-language predicate group evaluated once per message.
    pub group: Arc<dyn camel_language_api::PredicateGroup>,
    /// Per-clause evaluation metadata, parallel to `bodies`.
    pub metas: Vec<camel_language_api::EvalMeta>,
    /// Sub-pipeline for each clause, parallel to `metas`.
    pub bodies: Vec<camel_api::OutcomeSegment>,
    /// Pipeline to run when no clause matches.
    pub otherwise: Option<camel_api::OutcomeSegment>,
}

impl Clone for ChoiceGroupSegment {
    fn clone(&self) -> Self {
        Self {
            group: Arc::clone(&self.group),
            metas: self.metas.clone(),
            bodies: self.bodies.clone(),
            otherwise: self.otherwise.clone(),
        }
    }
}

impl ChoiceGroupSegment {
    /// Build a grouped choice segment.
    ///
    /// `metas` and `bodies` are parallel: `metas[i]` attributes an error from
    /// clause `i` and `bodies[i]` is the pipeline to run when clause `i` is the
    /// first match. Empty groups are rejected because the compiler falls back
    /// to the sequential path for 0/1 clauses.
    pub fn new(
        group: Arc<dyn camel_language_api::PredicateGroup>,
        metas: Vec<camel_language_api::EvalMeta>,
        bodies: Vec<camel_api::OutcomeSegment>,
        otherwise: Option<camel_api::OutcomeSegment>,
    ) -> Result<Self, CamelError> {
        if bodies.is_empty() {
            return Err(CamelError::Config(
                "ChoiceGroupSegment requires at least one clause".to_string(),
            ));
        }
        if metas.len() != bodies.len() {
            return Err(CamelError::Config(format!(
                "ChoiceGroupSegment metas ({}) and bodies ({}) must have equal length",
                metas.len(),
                bodies.len()
            )));
        }
        Ok(Self {
            group,
            metas,
            bodies,
            otherwise,
        })
    }
}

impl camel_api::OutcomePipeline for ChoiceGroupSegment {
    fn clone_box(&self) -> Box<dyn camel_api::OutcomePipeline> {
        Box::new(self.clone())
    }

    fn run<'a>(
        &'a mut self,
        exchange: camel_api::Exchange,
    ) -> Pin<Box<dyn Future<Output = camel_api::PipelineOutcome> + Send + 'a>> {
        Box::pin(async move {
            match self.group.first_match(&exchange).await {
                Ok(Some(index)) => self.bodies[index].run(exchange).await,
                Ok(None) => {
                    if let Some(otherwise) = self.otherwise.as_mut() {
                        otherwise.run(exchange).await
                    } else {
                        camel_api::PipelineOutcome::Completed(exchange)
                    }
                }
                Err(indexed) => camel_api::PipelineOutcome::Failed(
                    camel_language_api::eval::to_expression_failed(
                        indexed.error,
                        &self.metas[indexed.index],
                    ),
                ),
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

    // ── ChoiceGroupSegment (grouped when clauses) ────────────────────────

    use camel_language_api::{EvalMeta, IndexedPredicateError, LanguageError, PredicateGroup};

    enum GroupOutcome {
        Match(usize),
        NoMatch,
        Error { index: usize },
    }

    struct MockGroup {
        outcome: GroupOutcome,
        calls: Arc<AtomicU32>,
    }

    impl MockGroup {
        fn mock(outcome: GroupOutcome, calls: Arc<AtomicU32>) -> Arc<dyn PredicateGroup> {
            Arc::new(Self { outcome, calls })
        }
    }

    #[camel_language_api::async_trait]
    impl PredicateGroup for MockGroup {
        async fn first_match(
            &self,
            _exchange: &Exchange,
        ) -> Result<Option<usize>, IndexedPredicateError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            match &self.outcome {
                GroupOutcome::Match(i) => Ok(Some(*i)),
                GroupOutcome::NoMatch => Ok(None),
                GroupOutcome::Error { index } => Err(IndexedPredicateError {
                    index: *index,
                    error: LanguageError::NotSupported {
                        feature: "predicate_group".to_string(),
                        language: "rhai".to_string(),
                    },
                }),
            }
        }
    }

    fn group_meta(index: usize) -> EvalMeta {
        EvalMeta {
            language: "rhai".to_string(),
            route_id: "r1".to_string(),
            step_id: format!("step#{index}"),
            verb: format!("choice/when[{index}]"),
            target: None,
        }
    }

    fn group_segment(
        group: Arc<dyn PredicateGroup>,
        metas: Vec<EvalMeta>,
        bodies: Vec<camel_api::OutcomeSegment>,
        otherwise: Option<camel_api::OutcomeSegment>,
    ) -> ChoiceGroupSegment {
        ChoiceGroupSegment::new(group, metas, bodies, otherwise).expect("valid group segment")
    }

    fn body_segments(suffixes: &[&'static str]) -> Vec<camel_api::OutcomeSegment> {
        suffixes
            .iter()
            .map(|s| processor_segment(append_body(s)))
            .collect()
    }

    // 1. First matching clause runs its body.
    #[tokio::test]
    async fn choice_group_first_when_matches() {
        let calls = Arc::new(AtomicU32::new(0));
        let group = MockGroup::mock(GroupOutcome::Match(0), Arc::clone(&calls));
        let mut seg = group_segment(
            group,
            vec![group_meta(0), group_meta(1)],
            body_segments(&["-A", "-B"]),
            None,
        );
        let ex = Exchange::new(Message::new("x"));
        let result = completed_exchange(seg.run(ex).await);
        assert_eq!(result.input.body.as_text(), Some("x-A"));
        assert_eq!(calls.load(Ordering::SeqCst), 1, "exactly one first_match");
    }

    // 2. No match runs otherwise.
    #[tokio::test]
    async fn choice_group_no_match_runs_otherwise() {
        let calls = Arc::new(AtomicU32::new(0));
        let group = MockGroup::mock(GroupOutcome::NoMatch, Arc::clone(&calls));
        let mut seg = group_segment(
            group,
            vec![group_meta(0)],
            body_segments(&["-A"]),
            Some(processor_segment(append_body("-else"))),
        );
        let ex = Exchange::new(Message::new("x"));
        let result = completed_exchange(seg.run(ex).await);
        assert_eq!(result.input.body.as_text(), Some("x-else"));
    }

    // 3. No match and no otherwise → passthrough.
    #[tokio::test]
    async fn choice_group_no_match_passthrough_without_otherwise() {
        let calls = Arc::new(AtomicU32::new(0));
        let group = MockGroup::mock(GroupOutcome::NoMatch, Arc::clone(&calls));
        let mut seg = group_segment(group, vec![group_meta(0)], body_segments(&["-A"]), None);
        let ex = Exchange::new(Message::new("untouched"));
        let result = completed_exchange(seg.run(ex).await);
        assert_eq!(result.input.body.as_text(), Some("untouched"));
    }

    // 4. Group errors are attributed to the offending clause's metadata.
    #[tokio::test]
    async fn choice_group_error_maps_through_offending_clause_meta() {
        let calls = Arc::new(AtomicU32::new(0));
        let group = MockGroup::mock(GroupOutcome::Error { index: 2 }, Arc::clone(&calls));
        let mut seg = group_segment(
            group,
            vec![group_meta(0), group_meta(1), group_meta(2)],
            body_segments(&["-A", "-B", "-C"]),
            None,
        );
        match seg.run(Exchange::default()).await {
            PipelineOutcome::Failed(err) => match err {
                CamelError::ExpressionFailed { step_id, verb, .. } => {
                    assert_eq!(verb, "choice/when[2]", "clause attribution");
                    assert_eq!(step_id, "step#2");
                }
                other => panic!("expected ExpressionFailed, got {other:?}"),
            },
            other => panic!("expected PipelineOutcome::Failed, got {other:?}"),
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1, "exactly one first_match");
    }

    // 5. Only the selected clause's body runs.
    #[tokio::test]
    async fn choice_group_runs_body_of_selected_clause_only() {
        let calls = Arc::new(AtomicU32::new(0));
        let group = MockGroup::mock(GroupOutcome::Match(1), Arc::clone(&calls));
        let mut seg = group_segment(
            group,
            vec![group_meta(0), group_meta(1), group_meta(2)],
            body_segments(&["-A", "-B", "-C"]),
            None,
        );
        let ex = Exchange::new(Message::new("x"));
        let result = completed_exchange(seg.run(ex).await);
        assert_eq!(result.input.body.as_text(), Some("x-B"));
    }

    // 6. Constructor rejects mismatched metas/bodies and empty groups.
    #[test]
    fn choice_group_new_rejects_metas_body_mismatch() {
        let calls = Arc::new(AtomicU32::new(0));
        let group = MockGroup::mock(GroupOutcome::NoMatch, Arc::clone(&calls));
        assert!(
            ChoiceGroupSegment::new(
                Arc::clone(&group),
                vec![group_meta(0), group_meta(1)],
                body_segments(&["-A"]),
                None,
            )
            .is_err(),
            "metas.len() != bodies.len() must be rejected"
        );
        assert!(
            ChoiceGroupSegment::new(group, vec![], vec![], None).is_err(),
            "empty groups must be rejected"
        );
    }
}
