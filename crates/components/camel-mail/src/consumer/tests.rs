use std::collections::HashSet;
use std::io;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use camel_api::{CamelError, Exchange, IdempotentRepository};
use camel_component_api::test_support::RecordingRuntimeObservability;
use camel_component_api::{ConsumerContext, ExchangeEnvelope};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use super::{Polled, poll_once};
use crate::config::ImapEndpointConfig;
use crate::error::MailError;
use crate::headers::CAMEL_MAIL_IN_UID;
use crate::poll::{Mailbox, PollState, Session};
use crate::store::StoreCapabilities;
use crate::store::fake::{Fail, FakeConnector, FakeMailbox, FakeMessage, StoreCall, rfc822};

mod lifecycle;

const BOUND: Duration = Duration::from_secs(5);

fn cfg(uri_suffix: &str) -> ImapEndpointConfig {
    ImapEndpointConfig::from_uri(&format!("imap://h?allowCleartext=true{uri_suffix}"))
        .expect("config")
}

fn mailbox(
    uri_suffix: &str,
    conn: &FakeConnector,
    repo: Option<Arc<dyn IdempotentRepository>>,
) -> Mailbox {
    Mailbox {
        config: Arc::new(cfg(uri_suffix)),
        connector: Arc::new(conn.clone()),
        idempotent: repo,
    }
}

fn test_ctx() -> (ConsumerContext, mpsc::Receiver<ExchangeEnvelope>) {
    let (tx, rx) = mpsc::channel(16);
    let ctx = ConsumerContext::new(tx, CancellationToken::new(), "route-1".to_string());
    (ctx, rx)
}

type Received = Arc<Mutex<Vec<Exchange>>>;

/// What the scripted route does with each exchange.
#[derive(Clone, Copy)]
enum Route {
    Succeed,
    Fail,
    /// Succeed after setting would-be class C headers.
    Retarget,
}

fn spawn_route(
    mut rx: mpsc::Receiver<ExchangeEnvelope>,
    route: Route,
) -> (JoinHandle<()>, Received) {
    let received: Received = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&received);
    let handle = tokio::spawn(async move {
        while let Some(env) = rx.recv().await {
            seen.lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(env.exchange.clone());
            let Some(tx) = env.reply_tx else { continue };
            let mut ex = env.exchange;
            let reply = match route {
                Route::Succeed => Ok(ex),
                Route::Fail => Err(CamelError::ProcessorError("route failed".into())),
                Route::Retarget => {
                    ex.input
                        .set_header("CamelMailMoveTo", serde_json::json!("Evil"));
                    ex.input
                        .set_header("CamelMailDelete", serde_json::json!(true));
                    Ok(ex)
                }
            };
            let _ = tx.send(reply);
        }
    });
    (handle, received)
}

#[derive(Clone)]
struct BufWriter(Arc<Mutex<Vec<u8>>>);

impl io::Write for BufWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for BufWriter {
    type Writer = BufWriter;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

fn capture_warnings() -> (tracing::subscriber::DefaultGuard, Arc<Mutex<Vec<u8>>>) {
    let buf = Arc::new(Mutex::new(Vec::new()));
    let sub = tracing_subscriber::fmt()
        .with_ansi(false)
        .with_max_level(tracing::Level::DEBUG)
        .with_writer(BufWriter(Arc::clone(&buf)))
        .finish();
    (tracing::subscriber::set_default(sub), buf)
}

fn log_text(buf: &Arc<Mutex<Vec<u8>>>) -> String {
    String::from_utf8_lossy(&buf.lock().unwrap_or_else(PoisonError::into_inner)).into_owned()
}

/// In-memory repository double shared with the PollingConsumer tests.
#[derive(Debug, Default)]
pub(crate) struct TestRepo {
    pub(crate) seen: Mutex<HashSet<String>>,
    pub(crate) fail_contains: bool,
    pub(crate) fail_add: bool,
    pub(crate) adds: Mutex<Vec<String>>,
}

impl TestRepo {
    pub(crate) fn added(&self) -> Vec<String> {
        self.adds
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

#[async_trait::async_trait]
impl IdempotentRepository for TestRepo {
    fn name(&self) -> &str {
        "test-repo"
    }

    async fn contains(&self, key: &str) -> Result<bool, CamelError> {
        if self.fail_contains {
            return Err(CamelError::ProcessorError("repo read failed".into()));
        }
        Ok(self
            .seen
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(key))
    }

    async fn add(&self, key: &str) -> Result<bool, CamelError> {
        if self.fail_add {
            return Err(CamelError::ProcessorError("repo write failed".into()));
        }
        self.adds
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(key.to_string());
        Ok(self
            .seen
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(key.to_string()))
    }

    async fn remove(&self, key: &str) -> Result<(), CamelError> {
        self.seen
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(key);
        Ok(())
    }

    async fn clear(&self) -> Result<(), CamelError> {
        self.seen
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
        Ok(())
    }
}

/// One open session on a fake mailbox, a scripted route and poll memory.
struct Harness {
    conn: FakeConnector,
    mailbox: Mailbox,
    session: Session,
    state: PollState,
    ctx: ConsumerContext,
    rt: Arc<RecordingRuntimeObservability>,
    received: Received,
    _route: JoinHandle<()>,
}

impl Harness {
    async fn new(mb: FakeMailbox, uri_suffix: &str, route: Route) -> Self {
        Self::with_repo(mb, uri_suffix, route, None).await
    }

    async fn with_repo(
        mb: FakeMailbox,
        uri_suffix: &str,
        route: Route,
        repo: Option<Arc<dyn IdempotentRepository>>,
    ) -> Self {
        let conn = FakeConnector::new(mb);
        let mailbox = mailbox(uri_suffix, &conn, repo);
        let session = tokio::time::timeout(BOUND, mailbox.open())
            .await
            .expect("open timed out")
            .expect("open");
        let (ctx, rx) = test_ctx();
        let (route, received) = spawn_route(rx, route);
        Self {
            conn,
            mailbox,
            session,
            state: PollState::default(),
            ctx,
            rt: RecordingRuntimeObservability::new(true),
            received,
            _route: route,
        }
    }

    async fn try_poll(&mut self) -> Result<Polled, MailError> {
        tokio::time::timeout(
            BOUND,
            poll_once(
                &mut self.session,
                &self.mailbox,
                &mut self.state,
                &self.ctx,
                self.rt.as_ref(),
            ),
        )
        .await
        .expect("poll timed out")
    }

    async fn poll(&mut self) -> Polled {
        self.try_poll().await.expect("poll failed")
    }

    fn calls(&self) -> Vec<StoreCall> {
        calls(&self.conn)
    }

    /// Store calls after the body fetch of `uid`.
    fn after_fetch(&self, uid: u32) -> Vec<StoreCall> {
        let c = self.calls();
        let i = c
            .iter()
            .position(|x| *x == StoreCall::FetchBody(uid))
            .expect("fetch body call");
        c[i + 1..].to_vec()
    }

    fn received(&self) -> Vec<Exchange> {
        self.received
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn counted(&self, label: &str) -> bool {
        self.rt
            .errors()
            .contains(&("route-1".to_string(), label.to_string()))
    }

    fn is_seen(&self, uid: u32) -> bool {
        is_seen(&self.conn, uid)
    }

    fn holds(&self, uid: u32) -> bool {
        self.conn
            .mailbox
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .messages
            .contains_key(&uid)
    }

    /// Replace the folder content under a new UIDVALIDITY.
    fn replace_messages(&self, validity: u32, uids: &[u32]) {
        let mut mb = self
            .conn
            .mailbox
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        mb.uid_validity = validity;
        mb.messages.clear();
        for uid in uids {
            mb.messages.insert(
                *uid,
                FakeMessage {
                    raw: rfc822("new", Some("n@x"), "b2"),
                    seen: false,
                    reported_size: None,
                },
            );
        }
    }
}

fn calls(conn: &FakeConnector) -> Vec<StoreCall> {
    conn.mailbox
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .calls
        .clone()
}

fn is_seen(conn: &FakeConnector, uid: u32) -> bool {
    conn.mailbox
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .messages
        .get(&uid)
        .is_some_and(|m| m.seen)
}

fn one(uid: u32) -> FakeMailbox {
    FakeMailbox::new().with_message(uid, rfc822("s", Some("m@x"), "b"))
}

fn with_caps(mut mb: FakeMailbox, uidplus: bool, move_ext: bool) -> FakeMailbox {
    mb.capabilities = StoreCapabilities { uidplus, move_ext };
    mb
}

fn uid_of(ex: &Exchange) -> String {
    ex.input
        .header(CAMEL_MAIL_IN_UID)
        .map(|v| v.to_string())
        .unwrap_or_default()
}

// --- Message-ID idempotency -------------------------------------------------

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn duplicate_message_id_skipped_and_seen() {
    let mb = FakeMailbox::new()
        .with_message(1, rfc822("a", Some("dup@corp.test"), "x"))
        .with_message(2, rfc822("a", Some("dup@corp.test"), "x"));
    let repo = Arc::new(TestRepo::default());
    let mut h = Harness::with_repo(mb, "", Route::Succeed, Some(repo.clone())).await;
    h.poll().await;
    assert_eq!(h.received().len(), 1, "route must see exactly one exchange");
    let c = h.calls();
    assert!(c.contains(&StoreCall::MarkSeen(1)), "calls: {c:?}");
    assert!(c.contains(&StoreCall::MarkSeen(2)), "calls: {c:?}");
    assert_eq!(repo.added(), vec!["dup@corp.test".to_string()]);
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn repository_read_failure_is_not_a_duplicate_verdict() {
    let mb = FakeMailbox::new().with_message(1, rfc822("a", Some("m@corp.test"), "x"));
    let repo = Arc::new(TestRepo {
        fail_contains: true,
        ..TestRepo::default()
    });
    let mut h = Harness::with_repo(mb, "", Route::Succeed, Some(repo.clone())).await;
    h.poll().await;
    assert!(h.received().is_empty(), "route must receive nothing");
    assert!(repo.added().is_empty(), "no add without a delivery");
    assert!(!h.calls().contains(&StoreCall::MarkSeen(1)));
    assert!(h.counted("b-prime:mail:idempotent"));
    assert!(
        h.state.attempts.is_empty(),
        "attempts: {:?}",
        h.state.attempts
    );
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn message_without_message_id_processed() {
    let mb = FakeMailbox::new().with_message(1, rfc822("a", None, "x"));
    let repo = Arc::new(TestRepo::default());
    let mut h = Harness::with_repo(mb, "", Route::Succeed, Some(repo.clone())).await;
    h.poll().await;
    assert_eq!(h.received().len(), 1);
    assert!(repo.added().is_empty());
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn add_after_route_success_even_if_disposition_fails() {
    let mut mb = FakeMailbox::new().with_message(1, rfc822("a", Some("m@corp.test"), "x"));
    mb.fail_mark_seen = Some(Fail::Refused);
    let repo = Arc::new(TestRepo::default());
    let mut h = Harness::with_repo(mb, "", Route::Succeed, Some(repo.clone())).await;
    h.poll().await;
    assert_eq!(repo.added(), vec!["m@corp.test".to_string()]);
    assert!(h.counted("b-prime:mail:dispose"));
    assert!(!h.is_seen(1));
    // The unseen message comes back; the repository now suppresses it.
    h.poll().await;
    assert_eq!(h.received().len(), 1, "no redelivery past the repository");
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn failed_route_does_not_add() {
    let mb = FakeMailbox::new().with_message(1, rfc822("a", Some("m@corp.test"), "x"));
    let repo = Arc::new(TestRepo::default());
    let mut h = Harness::with_repo(mb, "", Route::Fail, Some(repo.clone())).await;
    h.poll().await;
    assert_eq!(h.received().len(), 1);
    assert!(repo.added().is_empty(), "{:?}", repo.added());
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn add_failure_counted_and_disposition_runs() {
    let mb = FakeMailbox::new().with_message(1, rfc822("a", Some("m@corp.test"), "x"));
    let repo = Arc::new(TestRepo {
        fail_add: true,
        ..TestRepo::default()
    });
    let mut h = Harness::with_repo(mb, "", Route::Succeed, Some(repo)).await;
    h.poll().await;
    assert!(h.counted("b-prime:mail:idempotent"));
    assert!(h.is_seen(1), "a failed add must not stop the disposition");
}

// --- Receive and dispose-after-route ----------------------------------------

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn success_sets_seen() {
    let mut h = Harness::new(one(7), "", Route::Succeed).await;
    h.poll().await;
    assert_eq!(h.received().len(), 1);
    let c = h.calls();
    let fetch = c.iter().position(|x| *x == StoreCall::FetchBody(7));
    let seen = c.iter().position(|x| *x == StoreCall::MarkSeen(7));
    assert!(fetch.is_some() && seen.is_some() && fetch < seen, "{c:?}");
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn no_duplicate_after_success() {
    let mut h = Harness::new(one(7), "", Route::Succeed).await;
    h.poll().await;
    let before = h.calls().len();
    h.poll().await;
    assert_eq!(h.received().len(), 1);
    let after = h.calls();
    assert!(
        !after[before..]
            .iter()
            .any(|x| matches!(x, StoreCall::FetchBody(_))),
        "{after:?}"
    );
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn failed_route_leaves_message_untouched() {
    let mut h = Harness::new(one(7), "", Route::Fail).await;
    h.poll().await;
    assert_eq!(h.state.attempts.get(&(1, 7)), Some(&1));
    assert!(!h.calls().contains(&StoreCall::MarkSeen(7)));
    assert!(!h.is_seen(7));
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn poison_message_bounded() {
    let (_guard, buf) = capture_warnings();
    let mut h = Harness::new(one(7), "&maxAttempts=2", Route::Fail).await;
    for _ in 0..4 {
        h.poll().await;
    }
    assert_eq!(h.received().len(), 2);
    assert!(log_text(&buf).contains("skipped for the process lifetime"));
    assert!(h.counted("b-prime:mail:skip"));
    assert!(!h.calls().contains(&StoreCall::MarkSeen(7)));
    assert!(!h.is_seen(7));
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn oversize_skipped_before_body_fetch() {
    let (_guard, buf) = capture_warnings();
    let big = rfc822("big", Some("b@x"), &"x".repeat(4096));
    let mb = FakeMailbox::new().with_message(3, big);
    let mut h = Harness::new(mb, "&maxMessageSize=1024", Route::Succeed).await;
    h.poll().await;
    assert!(!h.calls().contains(&StoreCall::FetchBody(3)));
    assert!(h.received().is_empty());
    assert!(log_text(&buf).contains("maxMessageSize"));
    assert!(h.counted("b-prime:mail:skip"));
    assert!(!h.is_seen(3));
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn skipped_messages_do_not_starve() {
    let big = rfc822("big", Some("b@x"), &"x".repeat(4096));
    let mb = FakeMailbox::new()
        .with_message(1, big)
        .with_message(2, rfc822("ok", Some("o@x"), "b"));
    let mut h = Harness::new(
        mb,
        "&maxMessagesPerPoll=1&maxMessageSize=1024",
        Route::Succeed,
    )
    .await;
    h.poll().await;
    let got = h.received();
    assert_eq!(got.len(), 1);
    assert_eq!(uid_of(&got[0]).trim_matches('"'), "2");
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn disposition_failure_counted() {
    let mut mb = one(7);
    mb.fail_mark_seen = Some(Fail::Refused);
    let mut h = Harness::new(mb, "", Route::Succeed).await;
    h.poll().await;
    assert_eq!(h.received().len(), 1);
    assert!(h.counted("b-prime:mail:dispose"));
    assert!(!h.is_seen(7));
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn refused_disposition_redelivery_is_bounded() {
    // copyTo into a folder the server refuses: the message stays unseen and
    // would be routed again on every poll without the attempt bound.
    let mut mb = one(7);
    mb.fail_copy = Some(Fail::Refused);
    let mut h = Harness::new(mb, "&copyTo=Missing&maxAttempts=2", Route::Succeed).await;
    for _ in 0..4 {
        h.poll().await;
    }
    assert_eq!(h.received().len(), 2);
    assert!(h.counted("b-prime:mail:dispose"));
    assert!(h.counted("b-prime:mail:skip"));
    assert!(!h.is_seen(7));
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn lost_connection_during_disposition_breaks_the_session() {
    let mut mb = one(7);
    mb.fail_mark_seen = Some(Fail::Lost);
    let mut h = Harness::new(mb, "", Route::Succeed).await;
    let err = h.try_poll().await.expect_err("session must be abandoned");
    assert!(err.is_retryable(), "{err}");
    assert!(h.session.is_broken());
    assert_eq!(h.received().len(), 1);
    assert!(h.counted("b-prime:mail:dispose"));
    assert_eq!(h.state.attempts.get(&(1, 7)), Some(&1));
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn vanished_message_is_not_an_attempt() {
    let mut mb = one(7).with_message(8, rfc822("s", Some("m8@x"), "b"));
    mb.vanish.insert(7);
    let mut h = Harness::new(mb, "", Route::Succeed).await;
    h.poll().await;
    assert_eq!(h.received().len(), 1);
    assert_eq!(uid_of(&h.received()[0]).trim_matches('"'), "8");
    assert!(h.state.attempts.is_empty(), "{:?}", h.state.attempts);
    assert!(h.state.skipped.is_empty(), "{:?}", h.state.skipped);
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn route_channel_closed_stops_poll() {
    let conn = FakeConnector::new(one(7));
    let mailbox = mailbox("", &conn, None);
    let mut session = mailbox.open().await.expect("open");
    let (ctx, rx) = test_ctx();
    drop(rx);
    let rt = RecordingRuntimeObservability::new(true);
    let mut state = PollState::default();
    let polled = tokio::time::timeout(
        BOUND,
        poll_once(&mut session, &mailbox, &mut state, &ctx, rt.as_ref()),
    )
    .await
    .expect("poll timed out")
    .expect("poll");
    assert_eq!(polled, Polled::RouteClosed);
    assert!(!calls(&conn).contains(&StoreCall::MarkSeen(7)));
    assert!(state.attempts.is_empty());
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn body_fetch_failure_is_bounded() {
    let mut mb = FakeMailbox::new()
        .with_message(1, rfc822("bad", Some("a@x"), "b"))
        .with_message(2, rfc822("ok", Some("o@x"), "b"));
    mb.fail_fetch_body.insert(1);
    let mut h = Harness::new(mb, "&maxAttempts=2", Route::Succeed).await;
    for _ in 0..2 {
        assert!(h.try_poll().await.is_err());
    }
    assert!(h.state.skipped.contains(&(1, 1)));
    assert!(h.state.attempts.is_empty());
    h.poll().await;
    assert_eq!(h.received().len(), 1);
}

#[test]
fn inbound_headers_cannot_retarget() {
    const RETARGET_EML: &[u8] = b"From: Alice <alice@corp.test>\r\n\
To: bob@corp.test\r\n\
Cc: carol@corp.test\r\n\
Bcc: dave@corp.test\r\n\
CamelMailTo: evil@attacker.test\r\n\
CamelMailMoveTo: Trash\r\n\
Subject: greetings\r\n\
Message-ID: <m1@corp.test>\r\n\
\r\n\
hello\r\n";
    let m = crate::adapter::inbound::parse_inbound(RETARGET_EML, 1, "INBOX").expect("parse");
    for h in [
        "CamelMailTo",
        "CamelMailCc",
        "CamelMailBcc",
        "CamelMailMoveTo",
    ] {
        assert!(m.exchange.input.header(h).is_none(), "{h} present");
    }
    let cfg =
        crate::config::SmtpEndpointConfig::from_uri("smtp://h?from=bot@corp.test&to=ops@corp.test")
            .expect("cfg");
    let out = crate::message::build_outbound(&cfg, &m.exchange).expect("outbound");
    let emails: Vec<&str> = out
        .recipients
        .envelope()
        .into_iter()
        .map(|a| a.email.as_str())
        .collect();
    assert_eq!(emails, ["ops@corp.test"]);
}

// --- Disposition options ----------------------------------------------------

async fn run_dispose(suffix: &str, mb: FakeMailbox, route: Route) -> Vec<StoreCall> {
    let mut h = Harness::new(mb, suffix, route).await;
    h.poll().await;
    h.after_fetch(7)
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn delete_with_uidplus_expunges_uid() {
    let after = run_dispose(
        "&delete=true",
        with_caps(one(7), true, false),
        Route::Succeed,
    )
    .await;
    assert_eq!(after, [StoreCall::MarkSeen(7), StoreCall::Delete(7)]);
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn move_with_move_capability() {
    let after = run_dispose(
        "&moveTo=Done",
        with_caps(one(7), false, true),
        Route::Succeed,
    )
    .await;
    assert_eq!(
        after,
        [
            StoreCall::MarkSeen(7),
            StoreCall::Move(7, "Done".to_string())
        ]
    );
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn move_without_move_capability_falls_back() {
    let after = run_dispose(
        "&moveTo=Done",
        with_caps(one(7), true, false),
        Route::Succeed,
    )
    .await;
    assert_eq!(
        after,
        [
            StoreCall::MarkSeen(7),
            StoreCall::Copy(7, "Done".to_string()),
            StoreCall::Delete(7)
        ]
    );
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn copy_to_copies_first() {
    let after = run_dispose(
        "&copyTo=Archive&moveTo=Done",
        with_caps(one(7), false, true),
        Route::Succeed,
    )
    .await;
    assert_eq!(
        after,
        [
            StoreCall::Copy(7, "Archive".to_string()),
            StoreCall::MarkSeen(7),
            StoreCall::Move(7, "Done".to_string())
        ]
    );
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn copy_only_still_marks_seen() {
    let after = run_dispose("&copyTo=Archive", one(7), Route::Succeed).await;
    assert_eq!(
        after,
        [
            StoreCall::Copy(7, "Archive".to_string()),
            StoreCall::MarkSeen(7)
        ]
    );
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn header_cannot_retarget_disposition() {
    let mut h = Harness::new(with_caps(one(7), true, true), "", Route::Retarget).await;
    h.poll().await;
    assert_eq!(h.after_fetch(7), [StoreCall::MarkSeen(7)]);
    assert!(!format!("{:?}", h.calls()).contains("Evil"));
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn failed_route_skips_all_disposition() {
    let after = run_dispose(
        "&copyTo=Archive&delete=true",
        with_caps(one(7), true, false),
        Route::Fail,
    )
    .await;
    assert!(after.is_empty(), "{after:?}");
}

async fn failing_copy(suffix: &str, uidplus: bool, move_ext: bool) -> Harness {
    let mut mb = with_caps(one(7), uidplus, move_ext);
    mb.fail_copy = Some(Fail::Refused);
    let mut h = Harness::new(mb, suffix, Route::Succeed).await;
    h.poll().await;
    assert_eq!(h.received().len(), 1);
    assert!(h.counted("b-prime:mail:dispose"));
    h
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn copy_failure_never_deletes() {
    let h = failing_copy("&moveTo=Done", true, false).await;
    assert!(
        !h.after_fetch(7)
            .iter()
            .any(|x| matches!(x, StoreCall::Delete(_))),
        "{:?}",
        h.calls()
    );
    assert!(h.holds(7));
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn copy_to_failure_leaves_message_unseen() {
    let h = failing_copy("&copyTo=Archive&moveTo=Done", true, true).await;
    let after = h.after_fetch(7);
    assert!(
        !after.iter().any(|x| matches!(
            x,
            StoreCall::MarkSeen(_) | StoreCall::Move(..) | StoreCall::Delete(_)
        )),
        "{after:?}"
    );
    assert!(!h.is_seen(7));
}

// --- UIDVALIDITY and listing ------------------------------------------------

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn uid_validity_change_discards_state() {
    let mut h = Harness::new(one(5), "&maxAttempts=1", Route::Fail).await;
    h.poll().await;
    assert_eq!(h.received().len(), 1);
    assert!(h.state.skipped.contains(&(1, 5)));

    h.replace_messages(2, &[5]);
    let (ctx, rx) = test_ctx();
    let (route, received) = spawn_route(rx, Route::Succeed);
    h.ctx = ctx;
    h.received = received;
    h._route = route;
    h.poll().await;
    assert_eq!(h.received().len(), 1);
    assert!(h.state.skipped.is_empty());
    assert!(h.state.attempts.is_empty());
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn uid_validity_change_issues_no_stale_command() {
    let mb = one(5).with_message(6, rfc822("s", Some("m2@x"), "b"));
    let mut h = Harness::new(mb, "&maxAttempts=1", Route::Fail).await;
    h.poll().await;

    h.replace_messages(2, &[9]);
    h.poll().await;

    let all = h.calls();
    let selects: Vec<usize> = all
        .iter()
        .enumerate()
        .filter(|(_, c)| matches!(c, StoreCall::Select(_)))
        .map(|(i, _)| i)
        .collect();
    assert_eq!(selects.len(), 2);
    let after = &all[selects[1]..];
    assert!(after.contains(&StoreCall::FetchBody(9)));
    for call in after {
        match call {
            StoreCall::FetchSizes(uids) => assert_eq!(uids, &vec![9]),
            StoreCall::FetchBody(u) | StoreCall::MarkSeen(u) => assert_eq!(*u, 9),
            _ => {}
        }
    }
}

fn fetch_size_calls(h: &Harness) -> Vec<Vec<u32>> {
    h.calls()
        .into_iter()
        .filter_map(|c| match c {
            StoreCall::FetchSizes(u) => Some(u),
            _ => None,
        })
        .collect()
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn large_listing_is_fetched_in_chunks() {
    let mut mb = FakeMailbox::new();
    for uid in 1..=1200u32 {
        mb = mb.with_message(uid, rfc822("s", None, "b"));
    }
    let mut h = Harness::new(mb, "&maxMessagesPerPoll=3", Route::Succeed).await;
    h.poll().await;
    let sizes = fetch_size_calls(&h);
    assert!(sizes.iter().all(|u| u.len() <= 500), "chunk too large");
    assert_eq!(sizes.len(), 1, "only the first chunk is requested");
    assert_eq!(h.received().len(), 3);
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn oversize_run_larger_than_chunk_does_not_starve() {
    let mut mb = FakeMailbox::new();
    for uid in 1..=601u32 {
        mb = mb.with_message(uid, rfc822("s", None, "b"));
        if uid <= 600 {
            mb.messages.get_mut(&uid).expect("message").reported_size = Some(1_000_000);
        }
    }
    let mut h = Harness::new(
        mb,
        "&maxMessagesPerPoll=1&maxMessageSize=1024",
        Route::Succeed,
    )
    .await;
    h.poll().await;
    assert_eq!(fetch_size_calls(&h).len(), 2);
    assert!(h.calls().contains(&StoreCall::FetchBody(601)));
    assert_eq!(h.received().len(), 1);
    assert_eq!(h.state.skipped.len(), 500 + 100);
}
