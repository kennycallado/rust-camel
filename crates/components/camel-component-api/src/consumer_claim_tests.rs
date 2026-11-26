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

// --- Dispatch-cancellation stamping in `send_and_wait` ---

fn dispatch_test_context() -> (ConsumerContext, mpsc::Receiver<ExchangeEnvelope>) {
    let (tx, rx) = mpsc::channel(1);
    let ctx = ConsumerContext::new(tx, CancellationToken::new(), "route".to_string());
    (ctx, rx)
}

#[tokio::test]
async fn send_and_wait_stamps_a_live_dispatch_token() {
    let (ctx, mut rx) = dispatch_test_context();
    let waiter = tokio::spawn({
        let ctx = ctx.clone();
        async move {
            ctx.send_and_wait(Exchange::new(camel_api::Message::new("call")))
                .await
        }
    });
    let mut envelope = timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("envelope must arrive within 2s")
        .expect("envelope channel alive");
    let token = envelope
        .exchange
        .dispatch_cancel()
        .expect("send_and_wait must stamp a dispatch marker");
    assert!(
        !token.is_cancelled(),
        "the dispatch token stays live while the waiter is alive"
    );
    let reply_tx = envelope.reply_tx.take().expect("request-reply envelope");
    reply_tx
        .send(Ok(envelope.exchange))
        .expect("waiter must still be listening");
    let result = timeout(Duration::from_secs(2), waiter)
        .await
        .expect("waiter must finish within 2s")
        .expect("waiter task must not panic");
    assert!(result.is_ok());
}

#[tokio::test]
async fn dropping_send_and_wait_cancels_the_stamped_token() {
    let (ctx, mut rx) = dispatch_test_context();
    let handle = tokio::spawn(async move {
        ctx.send_and_wait(Exchange::new(camel_api::Message::new("call")))
            .await
    });
    let envelope = timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("envelope must arrive within 2s")
        .expect("envelope channel alive");
    let token = envelope
        .exchange
        .dispatch_cancel()
        .expect("send_and_wait must stamp a dispatch marker");
    handle.abort();
    // The bounded join resolves only after the aborted future is dropped, so
    // the dispatch guard has fired when this returns.
    let _ = timeout(Duration::from_secs(2), handle).await;
    assert!(
        token.is_cancelled(),
        "dropping the waiter must cancel the dispatch token"
    );
}

#[tokio::test]
async fn completed_send_and_wait_disarms_the_token() {
    let (ctx, mut rx) = dispatch_test_context();
    let waiter = tokio::spawn({
        let ctx = ctx.clone();
        async move {
            ctx.send_and_wait(Exchange::new(camel_api::Message::new("call")))
                .await
        }
    });
    let mut envelope = timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("envelope must arrive within 2s")
        .expect("envelope channel alive");
    let token = envelope
        .exchange
        .dispatch_cancel()
        .expect("send_and_wait must stamp a dispatch marker");
    let reply_tx = envelope.reply_tx.take().expect("request-reply envelope");
    reply_tx
        .send(Ok(envelope.exchange))
        .expect("waiter must still be listening");
    let result = timeout(Duration::from_secs(2), waiter)
        .await
        .expect("waiter must finish within 2s")
        .expect("waiter task must not panic");
    assert!(result.is_ok());
    assert!(
        !token.is_cancelled(),
        "a completed reply disarms the dispatch signal"
    );
}

#[tokio::test]
async fn reply_carries_no_dispatch_marker() {
    let (ctx, mut rx) = dispatch_test_context();
    // Simulate pipeline entry: the receiver takes the marker, then replies.
    let receiver = tokio::spawn(async move {
        let mut envelope = timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("envelope must arrive within 2s")
            .expect("envelope channel alive");
        let taken = envelope.exchange.take_dispatch_cancel();
        let reply_tx = envelope.reply_tx.take().expect("request-reply envelope");
        reply_tx
            .send(Ok(envelope.exchange))
            .expect("waiter must still be listening");
        (rx, taken)
    });
    let reply = timeout(
        Duration::from_secs(2),
        ctx.send_and_wait(Exchange::new(camel_api::Message::new("call"))),
    )
    .await
    .expect("send_and_wait must finish within 2s")
    .expect("dispatch must succeed");
    assert!(
        reply.dispatch_cancel().is_none(),
        "the pipeline took the marker; the reply carries none"
    );
    let (mut rx, first_token) = timeout(Duration::from_secs(2), receiver)
        .await
        .expect("receiver must finish within 2s")
        .expect("receiver task must not panic");
    let first_token = first_token.expect("pipeline entry must take the marker");

    // A second `send_and_wait` on the returned exchange stamps a fresh token,
    // independent of the first dispatch's token.
    let waiter = tokio::spawn({
        let ctx = ctx.clone();
        async move { ctx.send_and_wait(reply).await }
    });
    let mut envelope = timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("second envelope must arrive within 2s")
        .expect("envelope channel alive");
    let second_token = envelope
        .exchange
        .dispatch_cancel()
        .expect("the second dispatch must stamp a marker");
    assert!(
        !second_token.is_cancelled(),
        "the second dispatch stamps a live token"
    );
    first_token.cancel();
    assert!(
        !second_token.is_cancelled(),
        "the second stamp is independent of the first dispatch token"
    );
    let reply_tx = envelope.reply_tx.take().expect("request-reply envelope");
    reply_tx
        .send(Ok(envelope.exchange))
        .expect("waiter must still be listening");
    let result = timeout(Duration::from_secs(2), waiter)
        .await
        .expect("second waiter must finish within 2s")
        .expect("waiter task must not panic");
    assert!(result.is_ok());
}
