//! Fanout drain-claim integration tests (bd rc-ulmo6, Task 2.1).
//!
//! Each test attaches a real [`InFlightClaim`] to a source exchange, drives a
//! fanout path (wiretap service / multicast segment / split segment), parks
//! the branch copy on a release barrier so it outlives the parent claim scope,
//! then asserts the
//! gauge is held at 1 by the branch copy alone.
//!
//! On the unpatched code path fanout copies carry no claim (plain
//! `Exchange::clone` drops it; split fragments are constructed without
//! one), so the gauge reads 0 at the `== 1`
//! assertion — these tests are the RED witness for the fix and intentionally
//! fail on the unpatched tree.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use camel_api::in_flight::{InFlightClaim, InFlightGauge};
use camel_api::{
    AggregationStrategy, BoxProcessor, CamelError, Exchange, Message, OutcomePipeline,
    OutcomeSegment, PipelineOutcome, SplitSource, split_body_lines,
};
use camel_processor::{MulticastSegment, SplitSegment, WireTapService};
use tokio::sync::{Notify, mpsc};
use tower::{Service, ServiceExt};

/// Bounded receive helper: no test may park forever on a channel (lint-unbounded-wait).
async fn recv_bounded<T>(rx: &mut mpsc::UnboundedReceiver<T>, what: &str) -> T {
    tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .unwrap_or_else(|_| panic!("{what} timed out"))
        .unwrap_or_else(|| panic!("{what} channel closed"))
}

/// Poll the gauge until it reads `want` (2s deadline). Used for every `== 0`
/// assertion — the branch task drops its copy AFTER signaling done, so an
/// immediate `== 0` assert races the drop.
async fn poll_until_total(gauge: &Arc<InFlightGauge>, want: u64) {
    let bounded = tokio::time::timeout(Duration::from_secs(2), async {
        while gauge.total() != want {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    assert!(
        bounded.is_ok(),
        "in-flight gauge did not reach {want} within 2s (now {})",
        gauge.total()
    );
}

/// Gated tap probe: signals entry, parks on the release barrier holding the
/// received exchange, then signals done and returns the exchange (wiretap
/// variant — the branch copy must be returned as-is).
#[derive(Clone)]
struct GatedProbe {
    entry: mpsc::UnboundedSender<()>,
    done: mpsc::UnboundedSender<()>,
    release: Arc<Notify>,
}

impl Service<Exchange> for GatedProbe {
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), CamelError>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, exchange: Exchange) -> Self::Future {
        let entry = self.entry.clone();
        let done = self.done.clone();
        let release = Arc::clone(&self.release);
        Box::pin(async move {
            entry
                .send(())
                .map_err(|_| CamelError::ProcessorError("entry signal dropped".to_string()))?;
            tokio::time::timeout(Duration::from_secs(2), release.notified())
                .await
                .map_err(|_| CamelError::ProcessorError("release barrier timed out".to_string()))?;
            done.send(())
                .map_err(|_| CamelError::ProcessorError("done signal dropped".to_string()))?;
            Ok(exchange)
        })
    }
}

/// OutcomePipeline variant of the forwarding probe: MOVES the received
/// exchange into the channel and completes with a fresh exchange.
#[derive(Clone)]
struct ForwardingOutcomeProbe {
    tx: mpsc::UnboundedSender<Exchange>,
}

impl OutcomePipeline for ForwardingOutcomeProbe {
    fn clone_box(&self) -> Box<dyn OutcomePipeline> {
        Box::new(self.clone())
    }
    fn run<'a>(
        &'a mut self,
        exchange: Exchange,
    ) -> Pin<Box<dyn Future<Output = PipelineOutcome> + Send + 'a>> {
        let tx = self.tx.clone();
        Box::pin(async move {
            if tx.send(exchange).is_err() {
                return PipelineOutcome::Failed(CamelError::ProcessorError(
                    "forward channel closed".to_string(),
                ));
            }
            PipelineOutcome::Completed(Exchange::new(Message::default()))
        })
    }
}

/// OutcomePipeline body that completes immediately with the exchange.
#[derive(Clone)]
struct ImmediateBody;

impl OutcomePipeline for ImmediateBody {
    fn clone_box(&self) -> Box<dyn OutcomePipeline> {
        Box::new(self.clone())
    }
    fn run<'a>(
        &'a mut self,
        exchange: Exchange,
    ) -> Pin<Box<dyn Future<Output = PipelineOutcome> + Send + 'a>> {
        Box::pin(async move { PipelineOutcome::Completed(exchange) })
    }
}

/// Receiver task: awaits the forwarded branch copy (holding it alive), signals
/// entry, parks on the release barrier, signals done, then drops the copy.
fn spawn_forward_receiver(
    mut rx: mpsc::UnboundedReceiver<Exchange>,
    entry: mpsc::UnboundedSender<()>,
    done: mpsc::UnboundedSender<()>,
    release: Arc<Notify>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let held = tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("receiver: forward channel recv timed out")
            .expect("receiver: forward channel closed");
        entry.send(()).expect("receiver: entry signal");
        tokio::time::timeout(Duration::from_secs(2), release.notified())
            .await
            .expect("receiver: release barrier timed out");
        done.send(()).expect("receiver: done signal");
        drop(held);
    })
}

fn claimed_exchange(gauge: &Arc<InFlightGauge>, body: &str) -> Exchange {
    let mut exchange = Exchange::new(Message::new(body));
    exchange.in_flight_claim = Some(InFlightClaim::attach(gauge));
    exchange
}

#[tokio::test]
async fn wiretap_branch_copy_keeps_gauge_nonzero_after_parent_claim_drop() {
    let gauge = Arc::new(InFlightGauge::new());
    let (entry_tx, mut entry_rx) = mpsc::unbounded_channel();
    let (done_tx, mut done_rx) = mpsc::unbounded_channel();
    let release = Arc::new(Notify::new());

    let tap = BoxProcessor::new(GatedProbe {
        entry: entry_tx,
        done: done_tx,
        release: Arc::clone(&release),
    });
    let mut svc = WireTapService::new(tap);

    let exchange = claimed_exchange(&gauge, "main");
    assert_eq!(gauge.total(), 1, "parent claim must count");

    // Act: main exchange returns immediately; tap runs as a detached task.
    let mut main = svc.ready().await.unwrap().call(exchange).await.unwrap();

    // The tap copy is parked inside the probe, holding its claim (if any).
    recv_bounded(&mut entry_rx, "wiretap tap entry").await;

    // Drop the returned main exchange and the parent claim it carries; only
    // the tap branch copy's split claim may keep the gauge non-zero.
    let parent_claim = main.in_flight_claim.take();
    drop(main);
    drop(parent_claim);

    // RED witness on unpatched code: plain `Exchange::clone` drops the claim,
    // so the gauge reads 0 here instead of 1.
    assert_eq!(
        gauge.total(),
        1,
        "wiretap branch copy must keep the gauge at 1 after the parent claim drops"
    );

    release.notify_one();
    poll_until_total(&gauge, 0).await;
    recv_bounded(&mut done_rx, "wiretap tap done").await;
}

#[tokio::test]
async fn multicast_branch_copy_outlives_parent_claim() {
    let gauge = Arc::new(InFlightGauge::new());
    let (entry_tx, mut entry_rx) = mpsc::unbounded_channel();
    let (done_tx, mut done_rx) = mpsc::unbounded_channel();
    let release = Arc::new(Notify::new());
    let (fwd_tx, fwd_rx) = mpsc::unbounded_channel();

    let receiver = spawn_forward_receiver(fwd_rx, entry_tx, done_tx, Arc::clone(&release));

    // Recipient A completes immediately; recipient B moves its branch copy
    // into the channel where the receiver task parks it on the barrier.
    let mut seg = MulticastSegment {
        branches: vec![
            OutcomeSegment::new(Box::new(ImmediateBody)),
            OutcomeSegment::new(Box::new(ForwardingOutcomeProbe { tx: fwd_tx })),
        ],
        parallel: false,
        parallel_limit: None,
        stop_on_exception: true,
        timeout: None,
        aggregator: Arc::new(|exchanges: Vec<Exchange>| {
            exchanges.into_iter().last().unwrap_or_default()
        }),
    };

    let exchange = claimed_exchange(&gauge, "main");

    // Act: all recipients complete (sequential LastWins-equivalent aggregator).
    let mut returned = match OutcomePipeline::run(&mut seg, exchange).await {
        PipelineOutcome::Completed(ex) => ex,
        other => panic!("multicast segment did not complete: {other:?}"),
    };
    recv_bounded(&mut entry_rx, "multicast receiver entry").await;

    // Drop the returned aggregate exchange and the parent claim.
    let returned_claim = returned.in_flight_claim.take();
    drop(returned);
    drop(returned_claim);

    // RED witness on unpatched code: the subscriber copies are uncounted.
    assert_eq!(
        gauge.total(),
        1,
        "multicast recipient branch copy must keep the gauge at 1 after the parent claim drops"
    );

    release.notify_one();
    poll_until_total(&gauge, 0).await;
    recv_bounded(&mut done_rx, "multicast receiver done").await;
    let _ = tokio::time::timeout(Duration::from_secs(2), receiver).await;
}

#[tokio::test]
async fn split_branch_copy_outlives_parent_claim() {
    let gauge = Arc::new(InFlightGauge::new());
    let (entry_tx, mut entry_rx) = mpsc::unbounded_channel();
    let (done_tx, mut done_rx) = mpsc::unbounded_channel();
    let release = Arc::new(Notify::new());
    let (fwd_tx, fwd_rx) = mpsc::unbounded_channel();

    let receiver = spawn_forward_receiver(fwd_rx, entry_tx, done_tx, Arc::clone(&release));

    // The segment recipient moves its fragment into the channel where the
    // receiver task parks it on the barrier.
    let mut seg = SplitSegment {
        splitter: SplitSource::Sync(split_body_lines()),
        body: OutcomeSegment::new(Box::new(ForwardingOutcomeProbe { tx: fwd_tx })),
        parallel: false,
        parallel_limit: None,
        stop_on_exception: true,
        aggregation: AggregationStrategy::LastWins,
    };

    // `split_body_lines` builds fragments through camel-api's
    // `fragment_exchange` seam (the split fix site).
    let exchange = claimed_exchange(&gauge, "only");

    // Act: the single fragment is processed by the segment recipient.
    let mut returned = match OutcomePipeline::run(&mut seg, exchange).await {
        PipelineOutcome::Completed(ex) => ex,
        other => panic!("split segment did not complete: {other:?}"),
    };
    recv_bounded(&mut entry_rx, "split segment entry").await;

    // Drop the returned aggregate exchange and the parent claim.
    let returned_claim = returned.in_flight_claim.take();
    drop(returned);
    drop(returned_claim);

    // RED witness on unpatched code: fragments are constructed without a claim.
    assert_eq!(
        gauge.total(),
        1,
        "split fragment branch copy must keep the gauge at 1 after the parent claim drops"
    );

    release.notify_one();
    poll_until_total(&gauge, 0).await;
    recv_bounded(&mut done_rx, "split segment done").await;
    let _ = tokio::time::timeout(Duration::from_secs(2), receiver).await;
}
