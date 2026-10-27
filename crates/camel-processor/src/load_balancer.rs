use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

// ── LoadBalanceSegment (ADR-0025 OutcomePipeline) ────────────────────────

/// Outcome-aware LoadBalance segment. Holds N destinations + a strategy.
/// On each call: strategy picks ONE destination (round-robin / failover /
/// random / weighted), runs it. If chosen destination returns Completed,
/// return Completed. If Stopped: return Stopped immediately (no failover —
/// Stop is successful control flow). If Failed: strategy decides (failover
/// retries next dest, others return Failed).
///
/// This differs from Multicast (which runs all branches) — LoadBalance picks
/// exactly one. The parallel cancellation logic from T13/T15 does NOT apply.
#[derive(Clone)]
pub struct LoadBalanceSegment {
    pub destinations: Vec<camel_api::OutcomeSegment>,
    pub strategy: camel_api::LoadBalanceStrategy,
    /// Shared round-robin index for interior mutability across cloned segments.
    pub round_robin_index: Arc<AtomicUsize>,
}

impl camel_api::OutcomePipeline for LoadBalanceSegment {
    fn clone_box(&self) -> Box<dyn camel_api::OutcomePipeline> {
        Box::new(self.clone())
    }

    fn run<'a>(
        &'a mut self,
        exchange: camel_api::Exchange,
    ) -> Pin<Box<dyn Future<Output = camel_api::PipelineOutcome> + Send + 'a>> {
        Box::pin(async move {
            let len = self.destinations.len();
            if len == 0 {
                return camel_api::PipelineOutcome::Completed(exchange);
            }

            let start_idx = match &self.strategy {
                camel_api::LoadBalanceStrategy::Random => rand::random_range(0..len),
                camel_api::LoadBalanceStrategy::Weighted(weights) => pick_weighted(weights, len),
                camel_api::LoadBalanceStrategy::Failover => 0,
                // RoundRobin and any future variant default to round-robin indexing.
                _ => self.round_robin_index.fetch_add(1, Ordering::SeqCst) % len,
            };

            let mut idx = start_idx;
            let mut last_err: Option<camel_api::CamelError> = None;
            loop {
                if idx >= len {
                    return camel_api::PipelineOutcome::Failed(last_err.unwrap_or_else(|| {
                        camel_api::CamelError::ProcessorError(
                            "load_balance: all destinations exhausted".to_string(),
                        )
                    }));
                }
                match self.destinations[idx].run(exchange.branch_clone()).await {
                    camel_api::PipelineOutcome::Completed(ex) => {
                        return camel_api::PipelineOutcome::Completed(ex);
                    }
                    camel_api::PipelineOutcome::Stopped(ex) => {
                        return camel_api::PipelineOutcome::Stopped(ex);
                    }
                    camel_api::PipelineOutcome::Failed(err) => match self.strategy {
                        camel_api::LoadBalanceStrategy::Failover => {
                            last_err = Some(err);
                            idx += 1;
                            continue;
                        }
                        _ => return camel_api::PipelineOutcome::Failed(err),
                    },
                }
            }
        })
    }
}

/// Pick a destination index using weighted random selection.
fn pick_weighted(weights: &[(String, u32)], len: usize) -> usize {
    if weights.is_empty() || len == 0 {
        return 0;
    }
    let numeric_weights: Vec<u64> = weights.iter().map(|(_, w)| *w as u64).collect();
    let Some(total) = numeric_weights
        .iter()
        .try_fold(0u64, |acc, w| acc.checked_add(*w))
    else {
        return 0;
    };
    if total == 0 {
        return 0;
    }
    let mut r = rand::random::<u64>() % total;
    for (i, w) in numeric_weights.iter().enumerate() {
        if r < *w {
            return i.min(len - 1);
        }
        r -= w;
    }
    len - 1
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_api::{CamelError, Exchange, Message};
    use std::sync::Mutex;

    // ── LoadBalanceSegment tests (ADR-0025 OutcomePipeline parity) ───

    /// OutcomePipeline body that mutates exchange body to "lb-stopped" then returns Stopped.
    struct StoppingBody;
    impl camel_api::OutcomePipeline for StoppingBody {
        fn clone_box(&self) -> Box<dyn camel_api::OutcomePipeline> {
            Box::new(StoppingBody)
        }
        fn run<'a>(
            &'a mut self,
            mut ex: Exchange,
        ) -> Pin<Box<dyn Future<Output = camel_api::PipelineOutcome> + Send + 'a>> {
            Box::pin(async move {
                ex.input.body = camel_api::Body::Text("lb-stopped".to_string());
                camel_api::PipelineOutcome::Stopped(ex)
            })
        }
    }

    /// OutcomePipeline body that records invocation count via shared counter.
    struct RecordingBody(Arc<AtomicUsize>);
    impl camel_api::OutcomePipeline for RecordingBody {
        fn clone_box(&self) -> Box<dyn camel_api::OutcomePipeline> {
            Box::new(RecordingBody(Arc::clone(&self.0)))
        }
        fn run<'a>(
            &'a mut self,
            ex: Exchange,
        ) -> Pin<Box<dyn Future<Output = camel_api::PipelineOutcome> + Send + 'a>> {
            let count = Arc::clone(&self.0);
            Box::pin(async move {
                count.fetch_add(1, Ordering::SeqCst);
                camel_api::PipelineOutcome::Completed(ex)
            })
        }
    }

    /// OutcomePipeline body that always fails with ProcessorError.
    struct FailingBody;
    impl camel_api::OutcomePipeline for FailingBody {
        fn clone_box(&self) -> Box<dyn camel_api::OutcomePipeline> {
            Box::new(FailingBody)
        }
        fn run<'a>(
            &'a mut self,
            _ex: Exchange,
        ) -> Pin<Box<dyn Future<Output = camel_api::PipelineOutcome> + Send + 'a>> {
            Box::pin(async {
                camel_api::PipelineOutcome::Failed(CamelError::ProcessorError(
                    "intentional fail".to_string(),
                ))
            })
        }
    }

    /// OutcomePipeline body that mutates body to "recovered" then completes.
    struct RecoveringBody;
    impl camel_api::OutcomePipeline for RecoveringBody {
        fn clone_box(&self) -> Box<dyn camel_api::OutcomePipeline> {
            Box::new(RecoveringBody)
        }
        fn run<'a>(
            &'a mut self,
            mut ex: Exchange,
        ) -> Pin<Box<dyn Future<Output = camel_api::PipelineOutcome> + Send + 'a>> {
            Box::pin(async move {
                ex.input.body = camel_api::Body::Text("recovered".to_string());
                camel_api::PipelineOutcome::Completed(ex)
            })
        }
    }

    /// Test 1: Stop inside a destination propagates immediately (no failover).
    /// First destination mutates + Stops; second destination is NOT tried.
    #[tokio::test]
    async fn load_balance_child_stop_propagates() {
        let count = Arc::new(AtomicUsize::new(0));
        let mut seg = LoadBalanceSegment {
            destinations: vec![
                camel_api::OutcomeSegment::new(Box::new(StoppingBody)),
                camel_api::OutcomeSegment::new(Box::new(RecordingBody(count.clone()))),
            ],
            strategy: camel_api::LoadBalanceStrategy::RoundRobin,
            round_robin_index: Arc::new(AtomicUsize::new(0)),
        };

        let ex = Exchange::new(Message::new("trigger"));
        let result = camel_api::OutcomePipeline::run(&mut seg, ex).await;

        match result {
            camel_api::PipelineOutcome::Stopped(ex) => {
                assert_eq!(
                    ex.input.body.as_text(),
                    Some("lb-stopped"),
                    "Stopped exchange must preserve mutation"
                );
            }
            other => panic!("expected PipelineOutcome::Stopped, got {other:?}"),
        }
        assert_eq!(
            count.load(Ordering::SeqCst),
            0,
            "second destination must NOT be tried when first is Stopped"
        );
    }

    /// Test 2: Failover strategy retries on failure. First destination fails,
    /// second destination succeeds.
    #[tokio::test]
    async fn load_balance_child_failure_retries_whole_step() {
        let mut seg = LoadBalanceSegment {
            destinations: vec![
                camel_api::OutcomeSegment::new(Box::new(FailingBody)),
                camel_api::OutcomeSegment::new(Box::new(RecoveringBody)),
            ],
            strategy: camel_api::LoadBalanceStrategy::Failover,
            round_robin_index: Arc::new(AtomicUsize::new(0)),
        };

        let ex = Exchange::new(Message::new("trigger"));
        let result = camel_api::OutcomePipeline::run(&mut seg, ex).await;

        match result {
            camel_api::PipelineOutcome::Completed(ex) => {
                assert_eq!(
                    ex.input.body.as_text(),
                    Some("recovered"),
                    "failover must produce the second destination's output"
                );
            }
            other => panic!("expected PipelineOutcome::Completed, got {other:?}"),
        }
    }

    /// Test 3: Round-robin strategy distributes across destinations.
    /// 3 sequential calls hit each destination once.
    #[tokio::test]
    async fn load_balance_strategy_selection_preserved() {
        let c1 = Arc::new(AtomicUsize::new(0));
        let c2 = Arc::new(AtomicUsize::new(0));
        let c3 = Arc::new(AtomicUsize::new(0));

        let mut seg = LoadBalanceSegment {
            destinations: vec![
                camel_api::OutcomeSegment::new(Box::new(RecordingBody(c1.clone()))),
                camel_api::OutcomeSegment::new(Box::new(RecordingBody(c2.clone()))),
                camel_api::OutcomeSegment::new(Box::new(RecordingBody(c3.clone()))),
            ],
            strategy: camel_api::LoadBalanceStrategy::RoundRobin,
            round_robin_index: Arc::new(AtomicUsize::new(0)),
        };

        for _ in 0..3 {
            let ex = Exchange::new(Message::new("test"));
            let _result = camel_api::OutcomePipeline::run(&mut seg, ex).await;
        }

        assert_eq!(
            c1.load(Ordering::SeqCst),
            1,
            "round-robin: dest 0 call count"
        );
        assert_eq!(
            c2.load(Ordering::SeqCst),
            1,
            "round-robin: dest 1 call count"
        );
        assert_eq!(
            c3.load(Ordering::SeqCst),
            1,
            "round-robin: dest 2 call count"
        );
    }

    /// Test: failover exhaustion preserves the LAST destination's error,
    /// NOT a generic "all destinations exhausted" message.
    #[tokio::test]
    async fn load_balance_segment_failover_exhaustion_preserves_last_error() {
        let err1 = CamelError::ProcessorError("first-dest-failed".to_string());
        let err2 = CamelError::ProcessorError("second-dest-failed".to_string());

        struct FailWith(CamelError);
        impl camel_api::OutcomePipeline for FailWith {
            fn clone_box(&self) -> Box<dyn camel_api::OutcomePipeline> {
                Box::new(FailWith(self.0.clone()))
            }
            fn run<'a>(
                &'a mut self,
                _ex: Exchange,
            ) -> Pin<Box<dyn Future<Output = camel_api::PipelineOutcome> + Send + 'a>> {
                let e = self.0.clone();
                Box::pin(async move { camel_api::PipelineOutcome::Failed(e) })
            }
        }

        let mut seg = LoadBalanceSegment {
            destinations: vec![
                camel_api::OutcomeSegment::new(Box::new(FailWith(err1))),
                camel_api::OutcomeSegment::new(Box::new(FailWith(err2.clone()))),
            ],
            strategy: camel_api::LoadBalanceStrategy::Failover,
            round_robin_index: Arc::new(AtomicUsize::new(0)),
        };

        let ex = Exchange::new(Message::new("test"));
        let result = camel_api::OutcomePipeline::run(&mut seg, ex).await;

        match result {
            camel_api::PipelineOutcome::Failed(err) => {
                assert_eq!(
                    err.to_string(),
                    err2.to_string(),
                    "failover exhaustion must return the LAST destination error, not a generic message"
                );
            }
            other => panic!(
                "expected PipelineOutcome::Failed(last error), got {:?}",
                other
            ),
        }
    }

    // ── Ported from the removed Tower load-balancer tests ──────────────

    #[tokio::test]
    async fn load_balance_round_robin_distribution_six_calls() {
        let c1 = Arc::new(AtomicUsize::new(0));
        let c2 = Arc::new(AtomicUsize::new(0));
        let c3 = Arc::new(AtomicUsize::new(0));

        let mut seg = LoadBalanceSegment {
            destinations: vec![
                camel_api::OutcomeSegment::new(Box::new(RecordingBody(c1.clone()))),
                camel_api::OutcomeSegment::new(Box::new(RecordingBody(c2.clone()))),
                camel_api::OutcomeSegment::new(Box::new(RecordingBody(c3.clone()))),
            ],
            strategy: camel_api::LoadBalanceStrategy::RoundRobin,
            round_robin_index: Arc::new(AtomicUsize::new(0)),
        };

        for _ in 0..6 {
            let ex = Exchange::new(Message::new("test"));
            let result = camel_api::OutcomePipeline::run(&mut seg, ex).await;
            assert!(
                matches!(result, camel_api::PipelineOutcome::Completed(_)),
                "round-robin dispatch must complete; got {result:?}"
            );
        }

        assert_eq!(c1.load(Ordering::SeqCst), 2);
        assert_eq!(c2.load(Ordering::SeqCst), 2);
        assert_eq!(c3.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn load_balance_random_distribution() {
        let c1 = Arc::new(AtomicUsize::new(0));
        let c2 = Arc::new(AtomicUsize::new(0));

        let mut seg = LoadBalanceSegment {
            destinations: vec![
                camel_api::OutcomeSegment::new(Box::new(RecordingBody(c1.clone()))),
                camel_api::OutcomeSegment::new(Box::new(RecordingBody(c2.clone()))),
            ],
            strategy: camel_api::LoadBalanceStrategy::Random,
            round_robin_index: Arc::new(AtomicUsize::new(0)),
        };

        for _ in 0..100 {
            let ex = Exchange::new(Message::new("test"));
            let result = camel_api::OutcomePipeline::run(&mut seg, ex).await;
            assert!(
                matches!(result, camel_api::PipelineOutcome::Completed(_)),
                "random dispatch must complete; got {result:?}"
            );
        }

        let total = c1.load(Ordering::SeqCst) + c2.load(Ordering::SeqCst);
        assert_eq!(total, 100);
        assert!(c1.load(Ordering::SeqCst) > 20);
        assert!(c2.load(Ordering::SeqCst) > 20);
    }

    #[tokio::test]
    async fn load_balance_failover_on_error() {
        let count = Arc::new(AtomicUsize::new(0));

        let mut seg = LoadBalanceSegment {
            destinations: vec![
                camel_api::OutcomeSegment::new(Box::new(FailingBody)),
                camel_api::OutcomeSegment::new(Box::new(RecordingBody(count.clone()))),
            ],
            strategy: camel_api::LoadBalanceStrategy::Failover,
            round_robin_index: Arc::new(AtomicUsize::new(0)),
        };

        let ex = Exchange::new(Message::new("test"));
        let result = camel_api::OutcomePipeline::run(&mut seg, ex).await;
        assert!(matches!(result, camel_api::PipelineOutcome::Completed(_)));
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn load_balance_failover_preserves_original_exchange() {
        // Failure retry must receive a branch clone of the original exchange.
        let seen_body: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let seen_body_clone = Arc::clone(&seen_body);

        struct CaptureBody(Arc<Mutex<Option<String>>>);
        impl camel_api::OutcomePipeline for CaptureBody {
            fn clone_box(&self) -> Box<dyn camel_api::OutcomePipeline> {
                Box::new(CaptureBody(Arc::clone(&self.0)))
            }
            fn run<'a>(
                &'a mut self,
                ex: Exchange,
            ) -> Pin<Box<dyn Future<Output = camel_api::PipelineOutcome> + Send + 'a>> {
                let seen = Arc::clone(&self.0);
                Box::pin(async move {
                    if let Some(text) = ex.input.body.as_text() {
                        *seen.lock().unwrap() = Some(text.to_string());
                    }
                    camel_api::PipelineOutcome::Completed(ex)
                })
            }
        }

        let mut seg = LoadBalanceSegment {
            destinations: vec![
                camel_api::OutcomeSegment::new(Box::new(FailingBody)),
                camel_api::OutcomeSegment::new(Box::new(CaptureBody(seen_body_clone))),
            ],
            strategy: camel_api::LoadBalanceStrategy::Failover,
            round_robin_index: Arc::new(AtomicUsize::new(0)),
        };

        let ex = Exchange::new(Message::new("original body"));
        let _result = camel_api::OutcomePipeline::run(&mut seg, ex).await;

        assert_eq!(
            seen_body.lock().unwrap().as_deref(),
            Some("original body"),
            "retry destination must receive the original exchange body, not a blank one"
        );
    }

    #[tokio::test]
    async fn load_balance_empty_destinations_completes() {
        let mut seg = LoadBalanceSegment {
            destinations: vec![],
            strategy: camel_api::LoadBalanceStrategy::RoundRobin,
            round_robin_index: Arc::new(AtomicUsize::new(0)),
        };

        let ex = Exchange::new(Message::new("test"));
        let result = camel_api::OutcomePipeline::run(&mut seg, ex).await;

        match result {
            camel_api::PipelineOutcome::Completed(ex) => {
                assert_eq!(ex.input.body.as_text(), Some("test"));
            }
            other => panic!("expected Completed(original), got {other:?}"),
        }
    }

    #[tokio::test]
    async fn load_balance_weighted_sum_does_not_overflow_u32() {
        // u32::MAX + 1 overflows a u32 sum; the segment's u64 selection must
        // not panic and must still dispatch to a destination.
        let c1 = Arc::new(AtomicUsize::new(0));
        let c2 = Arc::new(AtomicUsize::new(0));

        let mut seg = LoadBalanceSegment {
            destinations: vec![
                camel_api::OutcomeSegment::new(Box::new(RecordingBody(c1.clone()))),
                camel_api::OutcomeSegment::new(Box::new(RecordingBody(c2.clone()))),
            ],
            strategy: camel_api::LoadBalanceStrategy::Weighted(vec![
                ("a".to_string(), u32::MAX),
                ("b".to_string(), 1),
            ]),
            round_robin_index: Arc::new(AtomicUsize::new(0)),
        };

        let ex = Exchange::new(Message::new("test"));
        let result = camel_api::OutcomePipeline::run(&mut seg, ex).await;
        assert!(
            matches!(result, camel_api::PipelineOutcome::Completed(_)),
            "u32::MAX + 1 must not overflow with u64 sum: {result:?}"
        );
        assert_eq!(c1.load(Ordering::SeqCst) + c2.load(Ordering::SeqCst), 1);
    }
}
