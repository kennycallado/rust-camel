use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use camel_api::{IdempotentRepository, Value};
use camel_component_api::PollingConsumer;
use tokio::time::Instant;

use super::MailPollingConsumer;
use crate::config::ImapEndpointConfig;
use crate::consumer::tests::TestRepo;
use crate::error::MailError;
use crate::headers::CAMEL_MAIL_IN_UID;
use crate::poll::Mailbox;
use crate::store::fake::{Fail, FakeConnector, FakeMailbox, StoreCall, rfc822};
use crate::store::{MailStore, MailStoreConnector, StoreCapabilities};

fn cfg(suffix: &str) -> Arc<ImapEndpointConfig> {
    Arc::new(
        ImapEndpointConfig::from_uri(&format!("imap://h?allowCleartext=true{suffix}"))
            .expect("config"),
    )
}

fn consumer(
    suffix: &str,
    mailbox: FakeMailbox,
    repo: Option<Arc<dyn IdempotentRepository>>,
) -> (MailPollingConsumer, FakeConnector) {
    let connector = FakeConnector::new(mailbox);
    let c = MailPollingConsumer::new(Mailbox {
        config: cfg(suffix),
        connector: Arc::new(connector.clone()),
        idempotent: repo,
    });
    (c, connector)
}

fn count(conn: &FakeConnector, wanted: &StoreCall) -> usize {
    calls(conn).iter().filter(|c| *c == wanted).count()
}

async fn receive(
    c: &mut MailPollingConsumer,
    timeout: Duration,
) -> Result<Option<camel_api::Exchange>, camel_api::CamelError> {
    tokio::time::timeout(Duration::from_secs(10), c.receive(timeout))
        .await
        .expect("receive timed out")
}

fn calls(conn: &FakeConnector) -> Vec<StoreCall> {
    conn.mailbox
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .calls
        .clone()
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn poll_enrich_receives_one_message() {
    let mb = FakeMailbox::new().with_message(4, rfc822("s", Some("a@x"), "b"));
    let (mut c, conn) = consumer("", mb, None);
    let ex = receive(&mut c, Duration::from_secs(5))
        .await
        .expect("receive")
        .expect("message");
    assert_eq!(ex.input.header(CAMEL_MAIL_IN_UID), Some(&Value::from(4u32)));
    assert!(calls(&conn).contains(&StoreCall::MarkSeen(4)));

    let start = Instant::now();
    let second = receive(&mut c, Duration::from_secs(5))
        .await
        .expect("receive");
    assert!(second.is_none());
    assert!(start.elapsed() >= Duration::from_secs(5));
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn polling_consumer_applies_move() {
    let mut mb = FakeMailbox::new().with_message(4, rfc822("s", None, "b"));
    mb.capabilities = StoreCapabilities {
        uidplus: false,
        move_ext: true,
    };
    let (mut c, conn) = consumer("&moveTo=Done", mb, None);
    let got = receive(&mut c, Duration::from_secs(5))
        .await
        .expect("receive");
    assert!(got.is_some());
    assert!(calls(&conn).contains(&StoreCall::Move(4, "Done".to_string())));
}

/// Fake-only: the fake store answers without awaiting, so one pass fits in
/// a zero deadline. A real server needs network I/O, so `Duration::ZERO`
/// returns none there (see `zero_timeout_returns_none_when_io_must_wait`).
#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn zero_timeout_never_waits_on_an_instant_fake() {
    let (mut c, conn) = consumer("", FakeMailbox::new(), None);
    let start = Instant::now();
    let got = receive(&mut c, Duration::ZERO).await.expect("receive");
    assert!(got.is_none());
    assert_eq!(start.elapsed(), Duration::ZERO);
    assert_eq!(count(&conn, &StoreCall::SearchUnseen), 1);
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn zero_timeout_returns_none_when_io_must_wait() {
    let mut c = MailPollingConsumer::new(Mailbox {
        config: cfg(""),
        connector: Arc::new(PendingConnector),
        idempotent: None,
    });
    let start = Instant::now();
    let got = receive(&mut c, Duration::ZERO).await.expect("receive");
    assert!(got.is_none());
    assert_eq!(start.elapsed(), Duration::ZERO);
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn polling_consumer_fail_closed_without_uidplus() {
    let mb = FakeMailbox::new().with_message(4, rfc822("s", None, "b"));
    let (mut c, _conn) = consumer("&delete=true", mb, None);
    let e = receive(&mut c, Duration::ZERO).await.expect_err("err");
    assert!(e.to_string().contains("UIDPLUS"), "{e}");
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn polling_consumer_contains_error_maps_idempotent() {
    let mb = FakeMailbox::new().with_message(4, rfc822("s", Some("a@x"), "b"));
    let repo = Arc::new(TestRepo {
        fail_contains: true,
        ..TestRepo::default()
    });
    let (mut c, conn) = consumer("&idempotentRepository=r", mb, Some(repo.clone()));
    let e = receive(&mut c, Duration::ZERO).await.expect_err("err");
    assert!(e.to_string().contains("idempotent repository error"), "{e}");
    assert!(!calls(&conn).contains(&StoreCall::MarkSeen(4)));
    assert!(repo.added().is_empty(), "nothing delivered, nothing added");
}

struct PendingConnector;

#[async_trait]
impl MailStoreConnector for PendingConnector {
    async fn connect(&self) -> Result<Box<dyn MailStore>, MailError> {
        std::future::pending().await
    }
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn receive_times_out_on_stalled_store() {
    let mut c = MailPollingConsumer::new(Mailbox {
        config: cfg(""),
        connector: Arc::new(PendingConnector),
        idempotent: None,
    });
    let start = Instant::now();
    let got = receive(&mut c, Duration::from_secs(5))
        .await
        .expect("receive");
    assert!(got.is_none());
    assert!(start.elapsed() >= Duration::from_secs(5));
    assert!(start.elapsed() < Duration::from_secs(6));
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn polling_duplicate_marked_seen_then_next_candidate() {
    let mb = FakeMailbox::new()
        .with_message(1, rfc822("a", Some("A@x"), "b"))
        .with_message(2, rfc822("b", Some("B@x"), "b"));
    let repo = Arc::new(TestRepo::default());
    repo.seen
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert("A@x".to_string());
    let (mut c, conn) = consumer("&idempotentRepository=r", mb, Some(repo));
    let ex = receive(&mut c, Duration::ZERO)
        .await
        .expect("receive")
        .expect("message");
    assert_eq!(ex.input.header(CAMEL_MAIL_IN_UID), Some(&Value::from(2u32)));
    assert!(calls(&conn).contains(&StoreCall::MarkSeen(1)));
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn polling_message_without_id_returned_and_not_added() {
    let mb = FakeMailbox::new().with_message(1, rfc822("a", None, "b"));
    let repo = Arc::new(TestRepo::default());
    let (mut c, _conn) = consumer("&idempotentRepository=r", mb, Some(repo.clone()));
    let got = receive(&mut c, Duration::ZERO).await.expect("receive");
    assert!(got.is_some());
    assert!(repo.added().is_empty());
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn polling_adds_on_hand_over_even_if_disposition_fails() {
    let repo = Arc::new(TestRepo::default());
    let mb = FakeMailbox::new().with_message(1, rfc822("a", Some("A@x"), "b"));
    let (mut c, _conn) = consumer("&idempotentRepository=r", mb, Some(repo.clone()));
    assert!(
        receive(&mut c, Duration::ZERO)
            .await
            .expect("receive")
            .is_some()
    );
    assert_eq!(repo.added(), vec!["A@x".to_string()]);

    let repo = Arc::new(TestRepo::default());
    let mut mb = FakeMailbox::new().with_message(1, rfc822("a", Some("A@x"), "b"));
    mb.fail_mark_seen = Some(Fail::Refused);
    let (mut c, _conn) = consumer("&idempotentRepository=r", mb, Some(repo.clone()));
    assert!(
        receive(&mut c, Duration::ZERO)
            .await
            .expect("receive")
            .is_some()
    );
    assert_eq!(repo.added(), vec!["A@x".to_string()]);
    assert!(
        receive(&mut c, Duration::ZERO)
            .await
            .expect("receive")
            .is_none(),
        "the unseen message is now a known duplicate"
    );
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn polling_reconnects_after_error() {
    let mut mb = FakeMailbox::new().with_message(1, rfc822("a", None, "b"));
    mb.fail_search = Some(Fail::Lost);
    let (mut c, conn) = consumer("", mb, None);
    assert!(receive(&mut c, Duration::ZERO).await.is_err());
    conn.mailbox
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .fail_search = None;
    let got = receive(&mut c, Duration::ZERO).await.expect("receive");
    assert!(got.is_some());
    assert_eq!(count(&conn, &StoreCall::Connect), 2);
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn polling_refused_select_keeps_the_session() {
    let mut mb = FakeMailbox::new().with_message(1, rfc822("a", None, "b"));
    mb.fail_select = Some(Fail::Refused);
    let (mut c, conn) = consumer("", mb, None);
    let e = receive(&mut c, Duration::ZERO).await.expect_err("refused");
    assert!(e.to_string().contains("refused"), "{e}");
    conn.mailbox
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .fail_select = None;
    assert!(
        receive(&mut c, Duration::ZERO)
            .await
            .expect("receive")
            .is_some()
    );
    assert_eq!(count(&conn, &StoreCall::Connect), 1);
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn polling_lost_disposition_still_returns_the_message() {
    let mut mb = FakeMailbox::new()
        .with_message(1, rfc822("a", None, "b"))
        .with_message(2, rfc822("b", None, "b"));
    mb.fail_copy = Some(Fail::Lost);
    let (mut c, conn) = consumer("&copyTo=Archive", mb, None);
    let ex = receive(&mut c, Duration::ZERO)
        .await
        .expect("receive")
        .expect("message despite the failed disposition");
    assert_eq!(ex.input.header(CAMEL_MAIL_IN_UID), Some(&Value::from(1u32)));
    assert!(!calls(&conn).contains(&StoreCall::MarkSeen(1)));
    conn.mailbox
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .fail_copy = None;
    receive(&mut c, Duration::ZERO).await.expect("receive");
    assert_eq!(
        count(&conn, &StoreCall::Connect),
        2,
        "broken session dropped"
    );
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn polling_refused_disposition_redelivery_is_bounded() {
    let mut mb = FakeMailbox::new().with_message(1, rfc822("a", None, "b"));
    mb.fail_copy = Some(Fail::Refused);
    let (mut c, _conn) = consumer("&copyTo=Missing&maxAttempts=2", mb, None);
    for _ in 0..2 {
        assert!(
            receive(&mut c, Duration::ZERO)
                .await
                .expect("receive")
                .is_some()
        );
    }
    assert!(
        receive(&mut c, Duration::ZERO)
            .await
            .expect("receive")
            .is_none(),
        "skipped after maxAttempts"
    );
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn polling_large_listing_is_fetched_in_chunks() {
    let mut mb = FakeMailbox::new();
    for uid in 1..=1200u32 {
        mb = mb.with_message(uid, rfc822("s", None, "b"));
    }
    let (mut c, conn) = consumer("", mb, None);
    let got = receive(&mut c, Duration::from_secs(5))
        .await
        .expect("receive");
    assert!(got.is_some());
    let sizes: Vec<Vec<u32>> = calls(&conn)
        .into_iter()
        .filter_map(|c| match c {
            StoreCall::FetchSizes(u) => Some(u),
            _ => None,
        })
        .collect();
    assert!(sizes.iter().all(|u| u.len() <= 500));
    assert_eq!(sizes.len(), 1);
}

fn set(conn: &FakeConnector, f: impl FnOnce(&mut FakeMailbox)) {
    f(&mut conn
        .mailbox
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner));
}

fn is_seen(conn: &FakeConnector, uid: u32) -> bool {
    conn.mailbox
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .messages
        .get(&uid)
        .is_some_and(|m| m.seen)
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn deadline_during_disposition_still_returns_the_message() {
    let mut mb = FakeMailbox::new().with_message(1, rfc822("a", Some("A@x"), "b"));
    mb.mark_seen_delay = Some(Duration::from_secs(10));
    let repo = Arc::new(TestRepo::default());
    let (mut c, conn) = consumer("&idempotentRepository=r", mb, Some(repo.clone()));
    let start = Instant::now();
    let got = receive(&mut c, Duration::from_secs(1))
        .await
        .expect("receive");
    assert!(got.is_some(), "a loaded message is never dropped");
    assert!(start.elapsed() >= Duration::from_secs(10));
    assert!(is_seen(&conn, 1), "the disposition ran to completion");
    assert_eq!(repo.added(), vec!["A@x".to_string()]);
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn deadline_during_load_leaves_the_message_untouched() {
    let mut mb = FakeMailbox::new().with_message(1, rfc822("a", Some("A@x"), "b"));
    mb.fetch_body_delay = Some(Duration::from_secs(10));
    let repo = Arc::new(TestRepo::default());
    let (mut c, conn) = consumer("&idempotentRepository=r", mb, Some(repo.clone()));
    let start = Instant::now();
    let got = receive(&mut c, Duration::from_secs(1))
        .await
        .expect("receive");
    assert!(got.is_none());
    assert_eq!(start.elapsed(), Duration::from_secs(1));
    assert!(!is_seen(&conn, 1));
    assert!(repo.added().is_empty());
    set(&conn, |mb| mb.fetch_body_delay = None);
    assert!(
        receive(&mut c, Duration::from_secs(1))
            .await
            .expect("receive")
            .is_some(),
        "the next call reconnects and delivers"
    );
    assert_eq!(
        count(&conn, &StoreCall::Connect),
        2,
        "timed-out session dropped"
    );
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn stale_cached_session_is_replaced_once() {
    let mb = FakeMailbox::new()
        .with_message(1, rfc822("a", None, "b"))
        .with_message(2, rfc822("b", None, "b"));
    let (mut c, conn) = consumer("", mb, None);
    assert!(
        receive(&mut c, Duration::ZERO)
            .await
            .expect("first")
            .is_some()
    );
    set(&conn, |mb| mb.drop_selects = 1);
    let got = receive(&mut c, Duration::ZERO).await.expect("second");
    assert!(got.is_some(), "the fresh session delivers within the call");
    assert_eq!(count(&conn, &StoreCall::Connect), 2);
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn stale_cached_session_retry_failing_returns_err() {
    let mb = FakeMailbox::new()
        .with_message(1, rfc822("a", None, "b"))
        .with_message(2, rfc822("b", None, "b"));
    let (mut c, conn) = consumer("", mb, None);
    assert!(
        receive(&mut c, Duration::ZERO)
            .await
            .expect("first")
            .is_some()
    );
    set(&conn, |mb| mb.drop_selects = 2);
    let e = receive(&mut c, Duration::ZERO)
        .await
        .expect_err("both fail");
    assert!(e.to_string().contains("connection lost"), "{e}");
    assert_eq!(count(&conn, &StoreCall::Connect), 2, "exactly one retry");
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn refused_login_errors_without_caching_a_session() {
    let mut mb = FakeMailbox::new().with_message(1, rfc822("a", None, "b"));
    mb.refuse_login = true;
    let (mut c, conn) = consumer("", mb, None);
    let e = receive(&mut c, Duration::from_secs(5))
        .await
        .expect_err("refused");
    assert!(e.to_string().contains("login"), "{e}");
    assert!(c.session_slot().is_none());
    assert_eq!(count(&conn, &StoreCall::Connect), 1);
}
