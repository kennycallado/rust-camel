//! Exactly-once accounting for the HTTP consumer `consume` operation.
//!
//! `ConsumeOutcome` is the single state cell shared by the request guard and
//! the response body meter. Every transition is a `compare_exchange`; the
//! first writer wins and terminal states never change. Delegation is the
//! handoff point: the dispatch layer moves the cell to `Delegated` before
//! handing the wrapped body to Axum, which disarms the guard.

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::task::{Context, Poll};

use camel_api::ComponentMetrics;
use http_body::Frame;
use http_body::SizeHint;

const UNRECORDED: u8 = 0;
const SUCCEEDED: u8 = 1;
const FAILED: u8 = 2;
const DELEGATED: u8 = 3;

/// Shared, exactly-once outcome cell for one consumed HTTP request.
///
/// States: `Unrecorded=0`, `Succeeded=1`, `Failed=2`, `Delegated=3`.
/// Transitions, all first-writer-wins:
///
/// - guard drop: `Unrecorded -> Failed`;
/// - dispatch failure before delegation: `Unrecorded -> Failed`;
/// - dispatch handoff: `Unrecorded -> Delegated`;
/// - body completion: `Delegated -> Succeeded`;
/// - body error / drop before completion: `Delegated -> Failed`.
#[derive(Clone, Debug, Default)]
pub struct ConsumeOutcome(Arc<AtomicU8>);

impl ConsumeOutcome {
    /// Creates an `Unrecorded` outcome cell.
    pub fn new() -> Self {
        Self::default()
    }

    /// Attempts the dispatch-handoff transition. Returns `true` when this
    /// call performed it and `false` when another writer already won.
    pub fn delegate(&self) -> bool {
        self.transition(UNRECORDED, DELEGATED)
    }

    /// Returns `true` once the outcome is terminal (`Succeeded`/`Failed`).
    ///
    /// Part of the state-machine contract asserted by this module's tests;
    /// production dispatch does not need to poll it.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn is_final(&self) -> bool {
        matches!(self.0.load(Ordering::Acquire), SUCCEEDED | FAILED)
    }

    /// Records a failure before delegation (`Unrecorded -> Failed`) and
    /// emits the `consume` failure exactly once.
    pub fn record_failure_before_delegation(&self, metrics: &ComponentMetrics) {
        if self.transition(UNRECORDED, FAILED) {
            metrics.observe("consume", true);
        }
    }

    /// Records body completion (`Delegated -> Succeeded`) and emits the
    /// `consume` success exactly once.
    pub fn record_success_after_delegation(&self, metrics: &ComponentMetrics) {
        if self.transition(DELEGATED, SUCCEEDED) {
            metrics.observe("consume", false);
        }
    }

    /// Records a post-delegation failure (`Delegated -> Failed`) and emits
    /// the `consume` failure exactly once.
    pub fn record_failure_after_delegation(&self, metrics: &ComponentMetrics) {
        if self.transition(DELEGATED, FAILED) {
            metrics.observe("consume", true);
        }
    }

    fn transition(&self, from: u8, to: u8) -> bool {
        self.0
            .compare_exchange(from, to, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }
}

/// Request-scoped guard: dropping it while still `Unrecorded` records the
/// dispatch failure. Once the dispatch layer delegates, this drop is inert.
pub struct ConsumeGuard {
    outcome: ConsumeOutcome,
    metrics: ComponentMetrics,
}

impl ConsumeGuard {
    /// Arms the guard on the shared outcome cell.
    pub fn new(outcome: ConsumeOutcome, metrics: ComponentMetrics) -> Self {
        Self { outcome, metrics }
    }
}

impl Drop for ConsumeGuard {
    fn drop(&mut self) {
        self.outcome.record_failure_before_delegation(&self.metrics);
    }
}

/// Response body wrapper that records completion or early teardown.
///
/// Exhaustion records success; a body error or a drop before exhaustion
/// records failure. A frame that leaves the inner body end-of-stream
/// records success immediately, so normal teardown needs no extra poll.
pub struct MeterBody<B> {
    inner: B,
    outcome: ConsumeOutcome,
    metrics: ComponentMetrics,
}

impl<B> MeterBody<B> {
    /// Wraps `inner` and arms the meter on the shared outcome cell.
    pub fn new(inner: B, outcome: ConsumeOutcome, metrics: ComponentMetrics) -> Self {
        Self {
            inner,
            outcome,
            metrics,
        }
    }

    /// Returns whether the inner body already reports end-of-stream. The
    /// dispatch layer uses this to record success without sending a body.
    pub fn inner_is_end_stream(&self) -> bool
    where
        B: http_body::Body,
    {
        http_body::Body::is_end_stream(&self.inner)
    }
}

impl<B> http_body::Body for MeterBody<B>
where
    B: http_body::Body + Unpin,
{
    type Data = B::Data;
    type Error = B::Error;

    fn poll_frame(
        self: std::pin::Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let this = self.get_mut();
        let frame = match http_body::Body::poll_frame(std::pin::Pin::new(&mut this.inner), cx) {
            Poll::Ready(frame) => frame,
            Poll::Pending => return Poll::Pending,
        };

        match frame {
            Some(Ok(frame)) => {
                if http_body::Body::is_end_stream(&this.inner) {
                    this.outcome.record_success_after_delegation(&this.metrics);
                }
                Poll::Ready(Some(Ok(frame)))
            }
            Some(Err(error)) => {
                this.outcome.record_failure_after_delegation(&this.metrics);
                Poll::Ready(Some(Err(error)))
            }
            None => {
                this.outcome.record_success_after_delegation(&this.metrics);
                Poll::Ready(None)
            }
        }
    }

    fn is_end_stream(&self) -> bool {
        http_body::Body::is_end_stream(&self.inner)
    }

    fn size_hint(&self) -> SizeHint {
        http_body::Body::size_hint(&self.inner)
    }
}

impl<B> Drop for MeterBody<B> {
    fn drop(&mut self) {
        self.outcome.record_failure_after_delegation(&self.metrics);
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::pin::Pin;
    use std::sync::Arc;
    use std::task::{Context, Poll};

    use bytes::Bytes;
    use camel_api::ComponentMetrics;
    use camel_component_api::RuntimeObservability;
    use camel_component_api::test_support::RecordingRuntimeObservability;
    use http_body::Frame;

    use super::{ConsumeGuard, ConsumeOutcome, MeterBody};

    #[derive(Debug)]
    struct FakeError;

    /// Deterministic body double: frames pop one per poll, and
    /// `is_end_stream` reports true when the queue is empty only if the
    /// fixture asked for it.
    struct FakeBody {
        frames: VecDeque<Result<Frame<Bytes>, FakeError>>,
        end_stream_when_empty: bool,
    }

    impl FakeBody {
        fn new(frames: Vec<Result<Frame<Bytes>, FakeError>>, end_stream_when_empty: bool) -> Self {
            Self {
                frames: frames.into(),
                end_stream_when_empty,
            }
        }
    }

    impl http_body::Body for FakeBody {
        type Data = Bytes;
        type Error = FakeError;

        fn poll_frame(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
        ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
            Poll::Ready(self.get_mut().frames.pop_front())
        }

        fn is_end_stream(&self) -> bool {
            self.end_stream_when_empty && self.frames.is_empty()
        }
    }

    fn frame(data: &'static [u8]) -> Frame<Bytes> {
        Frame::data(Bytes::from_static(data))
    }

    fn recording() -> (Arc<RecordingRuntimeObservability>, ComponentMetrics) {
        let recorder = RecordingRuntimeObservability::new(true);
        let metrics = recorder.component_metrics("http");
        (recorder, metrics)
    }

    fn poll_meter(body: &mut MeterBody<FakeBody>) -> Poll<Option<Result<Frame<Bytes>, FakeError>>> {
        let waker = futures::task::noop_waker_ref();
        let mut cx = Context::from_waker(waker);
        http_body::Body::poll_frame(Pin::new(body), &mut cx)
    }

    #[test]
    fn guard_drop_records_failure_once() {
        let (recorder, metrics) = recording();
        let outcome = ConsumeOutcome::new();
        let guard = ConsumeGuard::new(outcome.clone(), metrics.clone());

        drop(guard);

        assert_eq!(
            recorder.ops(),
            vec![(
                "http".to_string(),
                "consume".to_string(),
                "failure".to_string()
            )],
            "guard drop must record one consume failure"
        );
        assert_eq!(
            recorder.errors(),
            vec![("http".to_string(), "e:http:consume".to_string())],
            "guard drop must forward one error"
        );

        outcome.record_failure_before_delegation(&metrics);

        assert_eq!(recorder.ops().len(), 1, "second transition must be a no-op");
        assert_eq!(recorder.errors().len(), 1, "second error must be a no-op");
        assert!(outcome.is_final(), "failed is terminal");
    }

    #[test]
    fn delegated_body_success_on_exhaustion() {
        let (recorder, metrics) = recording();
        let outcome = ConsumeOutcome::new();
        assert!(outcome.delegate(), "delegation owns the transition");

        let inner = FakeBody::new(vec![Ok(frame(b"one"))], false);
        let mut meter = MeterBody::new(inner, outcome.clone(), metrics);

        assert!(matches!(poll_meter(&mut meter), Poll::Ready(Some(Ok(_)))));
        assert!(recorder.ops().is_empty(), "frame is not completion yet");

        assert!(matches!(poll_meter(&mut meter), Poll::Ready(None)));
        assert_eq!(
            recorder.ops(),
            vec![(
                "http".to_string(),
                "consume".to_string(),
                "success".to_string()
            )],
            "exhaustion records one success"
        );
        assert!(recorder.errors().is_empty());

        drop(meter);
        assert_eq!(recorder.ops().len(), 1, "drop must not double-record");
    }

    #[test]
    fn delegated_body_drop_before_exhaustion_is_failure() {
        let (recorder, metrics) = recording();
        let outcome = ConsumeOutcome::new();
        assert!(outcome.delegate());

        let inner = FakeBody::new(vec![Ok(frame(b"one"))], false);
        let meter = MeterBody::new(inner, outcome.clone(), metrics);

        drop(meter);

        assert_eq!(
            recorder.ops(),
            vec![(
                "http".to_string(),
                "consume".to_string(),
                "failure".to_string()
            )],
            "dropping an unread delegated body records failure"
        );
        assert!(recorder.errors().len() == 1);
        assert!(outcome.is_final());
    }

    #[test]
    fn final_frame_end_stream_is_success_without_another_poll() {
        let (recorder, metrics) = recording();
        let outcome = ConsumeOutcome::new();
        assert!(outcome.delegate());

        let inner = FakeBody::new(vec![Ok(frame(b"only"))], true);
        let mut meter = MeterBody::new(inner, outcome.clone(), metrics);

        assert!(matches!(poll_meter(&mut meter), Poll::Ready(Some(Ok(_)))));
        assert_eq!(
            recorder.ops(),
            vec![(
                "http".to_string(),
                "consume".to_string(),
                "success".to_string()
            )],
            "end-of-stream frame records success immediately"
        );

        drop(meter);
        assert_eq!(recorder.ops().len(), 1);
        assert!(recorder.errors().is_empty());
    }

    #[test]
    fn body_error_is_failure_once() {
        let (recorder, metrics) = recording();
        let outcome = ConsumeOutcome::new();
        assert!(outcome.delegate());

        let inner = FakeBody::new(vec![Err(FakeError)], false);
        let mut meter = MeterBody::new(inner, outcome.clone(), metrics);

        assert!(matches!(poll_meter(&mut meter), Poll::Ready(Some(Err(_)))));
        assert_eq!(
            recorder.ops(),
            vec![(
                "http".to_string(),
                "consume".to_string(),
                "failure".to_string()
            )]
        );
        assert_eq!(recorder.errors().len(), 1);

        drop(meter);
        assert_eq!(recorder.ops().len(), 1, "drop must not double-record");
        assert!(
            !recorder
                .ops()
                .iter()
                .any(|(_, _, outcome)| outcome == "success"),
            "an error frame must never record success"
        );
    }

    #[test]
    fn empty_body_records_success_when_flagged() {
        let (recorder, metrics) = recording();
        let outcome = ConsumeOutcome::new();
        assert!(outcome.delegate());

        let inner = FakeBody::new(Vec::new(), true);
        let meter = MeterBody::new(inner, outcome.clone(), metrics.clone());

        assert!(meter.inner_is_end_stream(), "empty body is end-of-stream");

        outcome.record_success_after_delegation(&metrics);
        drop(meter);

        assert_eq!(
            recorder.ops(),
            vec![(
                "http".to_string(),
                "consume".to_string(),
                "success".to_string()
            )],
            "dispatch-side completion records exactly one success"
        );
        assert!(recorder.errors().is_empty());
    }

    #[test]
    fn success_after_delegation_is_not_doubled_by_drop() {
        let (recorder, metrics) = recording();
        let outcome = ConsumeOutcome::new();
        assert!(outcome.delegate());

        outcome.record_success_after_delegation(&metrics);

        let inner = FakeBody::new(vec![Ok(frame(b"never-read"))], false);
        let meter = MeterBody::new(inner, outcome.clone(), metrics);
        drop(meter);

        assert_eq!(
            recorder.ops(),
            vec![(
                "http".to_string(),
                "consume".to_string(),
                "success".to_string()
            )],
            "drop after success must not record failure"
        );
        assert!(recorder.errors().is_empty());
    }

    #[test]
    fn undelegated_body_cannot_record_success() {
        let (recorder, metrics) = recording();
        let outcome = ConsumeOutcome::new();

        let inner = FakeBody::new(vec![Ok(frame(b"one"))], false);
        let meter = MeterBody::new(inner, outcome.clone(), metrics.clone());

        outcome.record_success_after_delegation(&metrics);

        assert!(recorder.ops().is_empty(), "no success without delegation");
        assert!(recorder.errors().is_empty());
        assert!(!outcome.is_final(), "state remains unrecorded");
        drop(meter);
        assert!(recorder.ops().is_empty(), "drop is inert while unrecorded");
    }
}
