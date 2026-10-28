//! Lifecycle of the push consumer: initial delay, cancellation, reconnect
//! backoff, idle drops, refused polls and logins, health and fail-closed
//! capabilities.

use std::sync::{Arc, PoisonError};
use std::time::Duration;

use camel_api::CamelError;
use camel_component_api::test_support::RecordingRuntimeObservability;
use camel_component_api::{Consumer, NetworkRetryPolicy};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use super::{BOUND, calls, is_seen, mailbox, one, test_ctx};
use crate::consumer::MailConsumer;
use crate::health::MailHealthState;
use crate::store::fake::{Fail, FakeConnector, FakeMailbox, StoreCall};

fn consumer_for(uri_suffix: &str, conn: &FakeConnector, retry: NetworkRetryPolicy) -> MailConsumer {
    consumer_with(
        uri_suffix,
        conn,
        retry,
        MailHealthState::new(),
        RecordingRuntimeObservability::new(false),
    )
}

fn consumer_with(
    uri_suffix: &str,
    conn: &FakeConnector,
    retry: NetworkRetryPolicy,
    health: Arc<MailHealthState>,
    rt: Arc<RecordingRuntimeObservability>,
) -> MailConsumer {
    MailConsumer::new(mailbox(uri_suffix, conn, None), retry, health, rt)
}

fn count_calls(conn: &FakeConnector, wanted: &StoreCall) -> usize {
    calls(conn).iter().filter(|c| *c == wanted).count()
}

async fn wait_polled(conn: &FakeConnector) {
    tokio::time::timeout(BOUND, conn.polled.notified())
        .await
        .expect("poll did not happen in time");
}

async fn join(handle: JoinHandle<Result<(), CamelError>>) -> Result<(), CamelError> {
    tokio::time::timeout(BOUND, handle)
        .await
        .expect("consumer did not stop in time")
        .expect("consumer task panicked")
}

fn spawn_consumer(
    mut consumer: MailConsumer,
) -> (CancellationToken, JoinHandle<Result<(), CamelError>>) {
    let (ctx, _rx) = test_ctx();
    let token = ctx.cancel_token();
    let handle = tokio::spawn(async move {
        let _rx = _rx;
        consumer.start(ctx).await
    });
    (token, handle)
}

fn connect_times(conn: &FakeConnector) -> Vec<tokio::time::Instant> {
    conn.mailbox
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .connect_times
        .clone()
}

fn set_fail_search(conn: &FakeConnector, fail: Option<Fail>) {
    conn.mailbox
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .fail_search = fail;
}

fn backoff(initial_secs: u64, multiplier: f64) -> NetworkRetryPolicy {
    NetworkRetryPolicy {
        max_attempts: 3,
        initial_delay: Duration::from_secs(initial_secs),
        multiplier,
        max_delay: Duration::from_secs(60),
        jitter_factor: 0.0,
        ..Default::default()
    }
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn cancellation_stops_polling() {
    let conn = FakeConnector::new(FakeMailbox::new());
    let consumer = consumer_for(
        "&initialDelay=0&delay=60000",
        &conn,
        NetworkRetryPolicy::default(),
    );
    let (token, handle) = spawn_consumer(consumer);
    wait_polled(&conn).await;
    token.cancel();
    assert!(join(handle).await.is_ok());
    assert_eq!(count_calls(&conn, &StoreCall::SearchUnseen), 1);
    assert_eq!(calls(&conn).last(), Some(&StoreCall::Logout));
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn cancellation_during_initial_delay() {
    let conn = FakeConnector::new(FakeMailbox::new());
    let mut consumer = consumer_for("&initialDelay=30000", &conn, NetworkRetryPolicy::default());
    let (ctx, _rx) = test_ctx();
    ctx.cancel_token().cancel();
    let res = tokio::time::timeout(BOUND, consumer.start(ctx))
        .await
        .expect("start timed out");
    assert!(res.is_ok());
    assert_eq!(count_calls(&conn, &StoreCall::Connect), 0);
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn reconnect_after_connection_loss() {
    let mut mb = FakeMailbox::new();
    mb.connect_failures = 1;
    let conn = FakeConnector::new(mb);
    let consumer = consumer_for("&initialDelay=0&delay=60000", &conn, backoff(2, 1.0));
    let (token, handle) = spawn_consumer(consumer);
    wait_polled(&conn).await;
    let t = connect_times(&conn);
    assert_eq!(t.len(), 2);
    assert!(t[1] - t[0] >= Duration::from_secs(2));
    assert!(count_calls(&conn, &StoreCall::Select("INBOX".to_string())) >= 1);
    token.cancel();
    assert!(join(handle).await.is_ok());
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn connect_exhaustion_returns_error() {
    let mut mb = FakeMailbox::new();
    mb.connect_failures = 10;
    let conn = FakeConnector::new(mb);
    let retry = NetworkRetryPolicy {
        max_attempts: 2,
        initial_delay: Duration::from_millis(10),
        jitter_factor: 0.0,
        ..Default::default()
    };
    let mut consumer = consumer_for("&initialDelay=0", &conn, retry);
    let (ctx, _rx) = test_ctx();
    let res = tokio::time::timeout(BOUND, consumer.start(ctx))
        .await
        .expect("start timed out");
    assert!(res.is_err());
    assert_eq!(count_calls(&conn, &StoreCall::Connect), 2);
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn poll_error_reconnects() {
    let mut mb = FakeMailbox::new();
    mb.fail_search = Some(Fail::Lost);
    let conn = FakeConnector::new(mb);
    let consumer = consumer_for("&initialDelay=0&delay=60000", &conn, backoff(2, 2.0));
    let (token, handle) = spawn_consumer(consumer);
    wait_polled(&conn).await;
    set_fail_search(&conn, None);
    wait_polled(&conn).await;
    let t = connect_times(&conn);
    assert!(t.len() >= 2);
    assert!(t[1] - t[0] >= Duration::from_secs(2));
    token.cancel();
    assert!(join(handle).await.is_ok());
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn poll_failures_back_off_escalating() {
    let mut mb = FakeMailbox::new();
    mb.fail_search = Some(Fail::Lost);
    let conn = FakeConnector::new(mb);
    let consumer = consumer_for("&initialDelay=0&delay=60000", &conn, backoff(2, 2.0));
    let (token, handle) = spawn_consumer(consumer);
    while connect_times(&conn).len() < 4 {
        tokio::time::timeout(Duration::from_secs(30), conn.polled.notified())
            .await
            .expect("poll did not happen in time");
    }
    token.cancel();
    assert!(join(handle).await.is_ok());
    let t = connect_times(&conn);
    assert!(t[1] - t[0] >= Duration::from_secs(2));
    assert!(t[2] - t[1] >= Duration::from_secs(4));
    assert!(t[3] - t[2] >= Duration::from_secs(8));
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn refused_poll_keeps_the_session() {
    let mut mb = FakeMailbox::new();
    mb.fail_select = Some(Fail::Refused);
    let conn = FakeConnector::new(mb);
    let health = MailHealthState::new();
    let consumer = consumer_with(
        "&initialDelay=0&delay=1000",
        &conn,
        backoff(30, 1.0),
        Arc::clone(&health),
        RecordingRuntimeObservability::new(false),
    );
    let (token, handle) = spawn_consumer(consumer);
    for _ in 0..3 {
        wait_polled(&conn).await;
    }
    token.cancel();
    assert!(join(handle).await.is_ok());
    assert_eq!(count_calls(&conn, &StoreCall::Connect), 1, "no reconnect");
    let snap = health.snapshot();
    assert!(
        snap.last_failure
            .is_some_and(|(_, reason)| reason.contains("refused")),
        "health must report the refusal"
    );
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn consumer_updates_health_state() {
    let conn = FakeConnector::new(FakeMailbox::new());
    let state = MailHealthState::new();
    let consumer = consumer_with(
        "&initialDelay=0&delay=60000",
        &conn,
        NetworkRetryPolicy::default(),
        Arc::clone(&state),
        RecordingRuntimeObservability::new(false),
    );
    let (token, handle) = spawn_consumer(consumer);
    wait_polled(&conn).await;
    token.cancel();
    assert!(join(handle).await.is_ok());
    assert!(state.snapshot().last_ok.is_some());
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn poll_outcome_counted() {
    let conn = FakeConnector::new(FakeMailbox::new());
    let rt = RecordingRuntimeObservability::new(true);
    let consumer = consumer_with(
        "&initialDelay=0&delay=1000",
        &conn,
        NetworkRetryPolicy::default(),
        MailHealthState::new(),
        rt.clone(),
    );
    let (token, handle) = spawn_consumer(consumer);
    wait_polled(&conn).await;
    set_fail_search(&conn, Some(Fail::Lost));
    wait_polled(&conn).await;
    token.cancel();
    assert!(join(handle).await.is_ok());
    let ops = rt.ops();
    let has = |outcome: &str| {
        ops.iter()
            .any(|(c, o, r)| c == "mail" && o == "poll" && r == outcome)
    };
    assert!(has("success"), "{ops:?}");
    assert!(has("failure"), "{ops:?}");
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn transient_capabilities_error_reconnects() {
    let mut mb = FakeMailbox::new();
    mb.fail_capabilities = 1;
    let conn = FakeConnector::new(mb);
    let consumer = consumer_for("&initialDelay=0&delay=60000", &conn, backoff(2, 1.0));
    let (token, handle) = spawn_consumer(consumer);
    wait_polled(&conn).await;
    let t = connect_times(&conn);
    assert_eq!(t.len(), 2);
    assert!(t[1] - t[0] >= Duration::from_secs(2));
    token.cancel();
    assert!(join(handle).await.is_ok());
}

async fn start_fail_closed(uri_suffix: &str, mb: FakeMailbox) -> (FakeConnector, CamelError) {
    let conn = FakeConnector::new(mb);
    let mut consumer = consumer_for(uri_suffix, &conn, NetworkRetryPolicy::default());
    let (ctx, _rx) = test_ctx();
    let res = tokio::time::timeout(BOUND, consumer.start(ctx))
        .await
        .expect("start timed out");
    (conn, res.expect_err("must fail closed"))
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn delete_without_uidplus_fails_closed() {
    let (conn, err) = start_fail_closed("&initialDelay=0&delete=true", one(7)).await;
    assert!(err.to_string().contains("UIDPLUS"), "{err}");
    let c = calls(&conn);
    assert!(
        !c.iter().any(|x| matches!(
            x,
            StoreCall::Select(_) | StoreCall::FetchBody(_) | StoreCall::Delete(_)
        )),
        "{c:?}"
    );
    assert_eq!(
        count_calls(&conn, &StoreCall::Connect),
        1,
        "permanent: no retry"
    );
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn move_without_move_or_uidplus_fails_closed() {
    let (conn, err) = start_fail_closed("&initialDelay=0&moveTo=Done", one(7)).await;
    assert!(err.to_string().contains("MOVE or UIDPLUS"), "{err}");
    let c = calls(&conn);
    let i = c
        .iter()
        .position(|x| *x == StoreCall::Capabilities)
        .expect("capabilities call");
    assert!(
        !c[i + 1..].iter().any(|x| matches!(
            x,
            StoreCall::Select(_)
                | StoreCall::FetchSizes(_)
                | StoreCall::FetchBody(_)
                | StoreCall::MarkSeen(_)
                | StoreCall::Copy(..)
                | StoreCall::Move(..)
                | StoreCall::Delete(_)
        )),
        "{c:?}"
    );
    assert!(!is_seen(&conn, 7));
    assert!(
        conn.mailbox
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .messages
            .contains_key(&7)
    );
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn refused_login_stops_without_reconnect() {
    let mut mb = FakeMailbox::new();
    mb.refuse_login = true;
    let conn = FakeConnector::new(mb);
    let health = MailHealthState::new();
    let mut consumer = consumer_with(
        "&initialDelay=0",
        &conn,
        NetworkRetryPolicy::default(),
        Arc::clone(&health),
        RecordingRuntimeObservability::new(false),
    );
    let (ctx, _rx) = test_ctx();
    let res = tokio::time::timeout(BOUND, consumer.start(ctx))
        .await
        .expect("start timed out");
    let err = res.expect_err("a refused login is permanent");
    assert!(err.to_string().contains("login"), "{err}");
    assert_eq!(
        count_calls(&conn, &StoreCall::Connect),
        1,
        "no reconnect loop"
    );
    assert!(
        health.snapshot().stopped.is_some(),
        "health must keep the stop"
    );
}

fn drop_next_selects(conn: &FakeConnector, n: u32) {
    conn.mailbox
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .drop_selects = n;
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn idle_drop_reconnects_at_once() {
    let conn = FakeConnector::new(FakeMailbox::new());
    let health = MailHealthState::new();
    let consumer = consumer_with(
        "&initialDelay=0&delay=1000",
        &conn,
        backoff(30, 1.0),
        Arc::clone(&health),
        RecordingRuntimeObservability::new(false),
    );
    let (token, handle) = spawn_consumer(consumer);
    wait_polled(&conn).await;
    drop_next_selects(&conn, 1);
    while connect_times(&conn).len() < 2 {
        wait_polled(&conn).await;
    }
    wait_polled(&conn).await;
    token.cancel();
    assert!(join(handle).await.is_ok());
    let t = connect_times(&conn);
    assert_eq!(
        t[1] - t[0],
        Duration::from_secs(1),
        "no backoff step: {t:?}"
    );
    assert!(
        health.snapshot().last_failure.is_none(),
        "an idle drop is not unhealthy"
    );
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn idle_drop_retry_is_once_then_backoff() {
    let conn = FakeConnector::new(FakeMailbox::new());
    let consumer = consumer_for("&initialDelay=0&delay=1000", &conn, backoff(30, 1.0));
    let (token, handle) = spawn_consumer(consumer);
    wait_polled(&conn).await;
    drop_next_selects(&conn, 2);
    while connect_times(&conn).len() < 3 {
        tokio::time::timeout(Duration::from_secs(60), conn.polled.notified())
            .await
            .expect("poll did not happen in time");
    }
    token.cancel();
    assert!(join(handle).await.is_ok());
    let t = connect_times(&conn);
    assert_eq!(t[1] - t[0], Duration::from_secs(1), "{t:?}");
    assert!(
        t[2] - t[1] >= Duration::from_secs(30),
        "backoff after the retry: {t:?}"
    );
}
