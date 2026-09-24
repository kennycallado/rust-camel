use super::*;
use std::time::Duration;
use tokio::time::timeout;

// --- InFlightClaim / drainclaim tests ---

#[test]
fn claim_attach_increments_and_drop_decrements() {
    let counter = Arc::new(InFlightGauge::new());
    let claim = InFlightClaim::attach(&counter);
    assert_eq!(counter.total(), 1);
    drop(claim);
    assert_eq!(counter.total(), 0);
}

#[test]
fn claim_split_adds_one_sibling() {
    let counter = Arc::new(InFlightGauge::new());
    let original = InFlightClaim::attach(&counter);
    assert_eq!(counter.total(), 1);
    let sibling = original.split();
    assert_eq!(counter.total(), 2);
    drop(sibling);
    assert_eq!(counter.total(), 1);
    drop(original);
    assert_eq!(counter.total(), 0);
}

#[tokio::test]
async fn send_attaches_claim_when_counter_installed() {
    let counter = Arc::new(InFlightGauge::new());
    let (tx, mut rx) = mpsc::channel(1);
    let ctx = ConsumerContext::new(tx, CancellationToken::new(), "route".to_string())
        .with_in_flight_counter(Arc::clone(&counter));
    ctx.send(Exchange::new(camel_api::Message::new("payload")))
        .await
        .expect("send must succeed");
    let envelope = timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("envelope must arrive within 2s")
        .expect("envelope channel alive");
    assert!(envelope.in_flight_claim.is_some());
    assert_eq!(counter.total(), 1);
    drop(envelope);
    assert_eq!(counter.total(), 0);
}

#[tokio::test]
async fn send_without_counter_carries_none() {
    let (tx, mut rx) = mpsc::channel(1);
    let ctx = ConsumerContext::new(tx, CancellationToken::new(), "route".to_string());
    ctx.send(Exchange::new(camel_api::Message::new("payload")))
        .await
        .expect("send must succeed");
    let envelope = timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("envelope must arrive within 2s")
        .expect("envelope channel alive");
    assert!(envelope.in_flight_claim.is_none());
}

#[tokio::test]
async fn push_failure_rolls_claim_back() {
    let counter = Arc::new(InFlightGauge::new());
    // Pre-seed the counter with one live claim (baseline 1): a failed send
    // that never attached would leave the counter at 1, so only a real
    // rollback returns it to the prior value.
    let _baseline = InFlightClaim::attach(&counter);
    assert_eq!(counter.total(), 1);
    let (tx, rx) = mpsc::channel(1);
    drop(rx); // receiver gone — the push must fail
    let ctx = ConsumerContext::new(tx, CancellationToken::new(), "route".to_string())
        .with_in_flight_counter(Arc::clone(&counter));
    let err = ctx
        .send(Exchange::new(camel_api::Message::new("payload")))
        .await
        .expect_err("push into a closed channel must fail");
    assert!(matches!(err, CamelError::ChannelClosed));
    // The rejected envelope dropped its claim — counter rolled back to the
    // pre-existing baseline (1), not to 0.
    assert_eq!(counter.total(), 1);
}

#[tokio::test]
async fn raw_sender_path_stays_uncounted() {
    let counter = Arc::new(InFlightGauge::new());
    let (tx, mut rx) = mpsc::channel(1);
    let ctx = ConsumerContext::new(tx, CancellationToken::new(), "route".to_string())
        .with_in_flight_counter(Arc::clone(&counter));
    let sender = ctx.sender();
    sender
        .send(ExchangeEnvelope {
            exchange: Exchange::new(camel_api::Message::new("payload")),
            reply_tx: None,
            in_flight_claim: None,
        })
        .await
        .expect("raw push must succeed");
    let envelope = timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("envelope must arrive within 2s")
        .expect("envelope channel alive");
    assert!(envelope.in_flight_claim.is_none());
    assert_eq!(counter.total(), 0);
}

// rc-nftni: raw-sender components capture the counter through the getter and
// mint at their own acceptance points — the getter must return the SAME Arc
// the context mints with, not a copy of the current value.
#[test]
fn in_flight_counter_getter_returns_the_installed_arc() {
    let counter = Arc::new(InFlightGauge::new());
    let (tx, _rx) = mpsc::channel(1);
    let ctx = ConsumerContext::new(tx, CancellationToken::new(), "route".to_string());
    assert!(
        ctx.in_flight_counter().is_none(),
        "contexts without a counter must return None"
    );
    let ctx = ctx.with_in_flight_counter(Arc::clone(&counter));
    let returned = ctx
        .in_flight_counter()
        .expect("counter must be returned once installed");
    // Attach through the RETURNED handle: only the same Arc moves the
    // original counter.
    let claim = InFlightClaim::attach(&returned);
    assert_eq!(counter.total(), 1);
    drop(claim);
    assert_eq!(counter.total(), 0);
}
