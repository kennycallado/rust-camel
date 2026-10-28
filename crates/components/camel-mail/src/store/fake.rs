//! In-memory `MailStore` for consumer tests.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use async_trait::async_trait;

use super::{MailStore, MailStoreConnector, MessageSize, SelectedFolder, StoreCapabilities};
use crate::error::MailError;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum StoreCall {
    Connect,
    Capabilities,
    Select(String),
    SearchUnseen,
    FetchSizes(Vec<u32>),
    FetchBody(u32),
    MarkSeen(u32),
    Copy(u32, String),
    Move(u32, String),
    Delete(u32),
    Logout,
}

/// How a scripted fake failure manifests.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Fail {
    /// A tagged `NO`: the session stays usable.
    Refused,
    /// A broken connection: the session must be dropped.
    Lost,
}

impl Fail {
    fn error(self, what: &str) -> MailError {
        match self {
            Fail::Refused => MailError::Refused(format!("fake {what} refused")),
            Fail::Lost => MailError::Store(format!("fake {what} connection lost")),
        }
    }
}

pub(crate) struct FakeMessage {
    pub(crate) raw: Vec<u8>,
    pub(crate) seen: bool,
    pub(crate) reported_size: Option<u64>,
}

pub(crate) struct FakeMailbox {
    pub(crate) uid_validity: u32,
    pub(crate) messages: BTreeMap<u32, FakeMessage>,
    pub(crate) capabilities: StoreCapabilities,
    pub(crate) calls: Vec<StoreCall>,
    pub(crate) connect_times: Vec<tokio::time::Instant>,
    pub(crate) connect_failures: u32,
    /// Every connect fails with a refused login.
    pub(crate) refuse_login: bool,
    pub(crate) fail_mark_seen: Option<Fail>,
    pub(crate) fail_select: Option<Fail>,
    /// The next this-many SELECTs fail with a lost connection (an idle
    /// session the server dropped).
    pub(crate) drop_selects: u32,
    pub(crate) fail_search: Option<Fail>,
    pub(crate) fail_capabilities: u32,
    pub(crate) fail_copy: Option<Fail>,
    /// UIDs whose body download fails with a lost connection.
    pub(crate) fail_fetch_body: BTreeSet<u32>,
    /// UIDs listed by the search but expunged before the body fetch.
    pub(crate) vanish: BTreeSet<u32>,
    /// Simulated server latency of a body download.
    pub(crate) fetch_body_delay: Option<Duration>,
    /// Simulated server latency of the `\Seen` STORE.
    pub(crate) mark_seen_delay: Option<Duration>,
}

impl FakeMailbox {
    pub(crate) fn new() -> Self {
        Self {
            uid_validity: 1,
            messages: BTreeMap::new(),
            capabilities: StoreCapabilities::default(),
            calls: Vec::new(),
            connect_times: Vec::new(),
            connect_failures: 0,
            refuse_login: false,
            fail_mark_seen: None,
            fail_select: None,
            drop_selects: 0,
            fail_search: None,
            fail_capabilities: 0,
            fail_copy: None,
            fail_fetch_body: BTreeSet::new(),
            vanish: BTreeSet::new(),
            fetch_body_delay: None,
            mark_seen_delay: None,
        }
    }

    pub(crate) fn with_message(mut self, uid: u32, raw: Vec<u8>) -> Self {
        self.messages.insert(
            uid,
            FakeMessage {
                raw,
                seen: false,
                reported_size: None,
            },
        );
        self
    }
}

#[derive(Clone)]
pub(crate) struct FakeConnector {
    pub(crate) mailbox: Arc<Mutex<FakeMailbox>>,
    pub(crate) polled: Arc<tokio::sync::Notify>,
}

impl FakeConnector {
    pub(crate) fn new(mailbox: FakeMailbox) -> Self {
        Self {
            mailbox: Arc::new(Mutex::new(mailbox)),
            polled: Arc::new(tokio::sync::Notify::new()),
        }
    }
}

fn lock(m: &Mutex<FakeMailbox>) -> std::sync::MutexGuard<'_, FakeMailbox> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

#[async_trait]
impl MailStoreConnector for FakeConnector {
    async fn connect(&self) -> Result<Box<dyn MailStore>, MailError> {
        let mut mb = lock(&self.mailbox);
        mb.calls.push(StoreCall::Connect);
        mb.connect_times.push(tokio::time::Instant::now());
        if mb.refuse_login {
            return Err(MailError::LoginRefused);
        }
        if mb.connect_failures > 0 {
            mb.connect_failures -= 1;
            return Err(MailError::Connect("fake connect failure".into()));
        }
        drop(mb);
        Ok(Box::new(FakeStore {
            mailbox: Arc::clone(&self.mailbox),
            polled: Arc::clone(&self.polled),
        }))
    }
}

struct FakeStore {
    mailbox: Arc<Mutex<FakeMailbox>>,
    polled: Arc<tokio::sync::Notify>,
}

#[async_trait]
impl MailStore for FakeStore {
    async fn capabilities(&mut self) -> Result<StoreCapabilities, MailError> {
        let mut mb = lock(&self.mailbox);
        mb.calls.push(StoreCall::Capabilities);
        if mb.fail_capabilities > 0 {
            mb.fail_capabilities -= 1;
            return Err(MailError::Store("fake capabilities failure".into()));
        }
        Ok(mb.capabilities)
    }

    async fn select(&mut self, folder: &str) -> Result<SelectedFolder, MailError> {
        let mut mb = lock(&self.mailbox);
        mb.calls.push(StoreCall::Select(folder.to_string()));
        self.polled.notify_one();
        if mb.drop_selects > 0 {
            mb.drop_selects -= 1;
            return Err(Fail::Lost.error("select"));
        }
        if let Some(fail) = mb.fail_select {
            return Err(fail.error("select"));
        }
        Ok(SelectedFolder {
            uid_validity: mb.uid_validity,
        })
    }

    async fn search_unseen(&mut self) -> Result<Vec<u32>, MailError> {
        let mut mb = lock(&self.mailbox);
        mb.calls.push(StoreCall::SearchUnseen);
        if let Some(fail) = mb.fail_search {
            return Err(fail.error("search"));
        }
        Ok(mb
            .messages
            .iter()
            .filter(|(_, m)| !m.seen)
            .map(|(uid, _)| *uid)
            .collect())
    }

    async fn fetch_sizes(&mut self, uids: &[u32]) -> Result<Vec<MessageSize>, MailError> {
        let mut mb = lock(&self.mailbox);
        mb.calls.push(StoreCall::FetchSizes(uids.to_vec()));
        Ok(uids
            .iter()
            .filter_map(|uid| {
                mb.messages.get(uid).map(|m| MessageSize {
                    uid: *uid,
                    size: m.reported_size.unwrap_or(m.raw.len() as u64),
                })
            })
            .collect())
    }

    async fn fetch_body(&mut self, uid: u32) -> Result<Option<Vec<u8>>, MailError> {
        let delay = {
            let mut mb = lock(&self.mailbox);
            mb.calls.push(StoreCall::FetchBody(uid));
            mb.fetch_body_delay
        };
        if let Some(delay) = delay {
            tokio::time::sleep(delay).await;
        }
        let mb = lock(&self.mailbox);
        if mb.fail_fetch_body.contains(&uid) {
            return Err(Fail::Lost.error("fetch-body"));
        }
        if mb.vanish.contains(&uid) {
            return Ok(None);
        }
        Ok(mb.messages.get(&uid).map(|m| m.raw.clone()))
    }

    async fn mark_seen(&mut self, uid: u32) -> Result<(), MailError> {
        let delay = {
            let mut mb = lock(&self.mailbox);
            mb.calls.push(StoreCall::MarkSeen(uid));
            mb.mark_seen_delay
        };
        if let Some(delay) = delay {
            tokio::time::sleep(delay).await;
        }
        let mut mb = lock(&self.mailbox);
        if let Some(fail) = mb.fail_mark_seen {
            return Err(fail.error("mark-seen"));
        }
        match mb.messages.get_mut(&uid) {
            Some(m) => {
                m.seen = true;
                Ok(())
            }
            None => Err(MailError::Store(format!("no message with UID {uid}"))),
        }
    }

    async fn copy(&mut self, uid: u32, folder: &str) -> Result<(), MailError> {
        let mut mb = lock(&self.mailbox);
        mb.calls.push(StoreCall::Copy(uid, folder.to_string()));
        if let Some(fail) = mb.fail_copy {
            return Err(fail.error("copy"));
        }
        if mb.messages.contains_key(&uid) {
            Ok(())
        } else {
            Err(MailError::Store(format!("no message with UID {uid}")))
        }
    }

    async fn move_to(&mut self, uid: u32, folder: &str) -> Result<(), MailError> {
        let mut mb = lock(&self.mailbox);
        mb.calls.push(StoreCall::Move(uid, folder.to_string()));
        match mb.messages.remove(&uid) {
            Some(_) => Ok(()),
            None => Err(MailError::Store(format!("no message with UID {uid}"))),
        }
    }

    async fn delete_uid(&mut self, uid: u32) -> Result<(), MailError> {
        let mut mb = lock(&self.mailbox);
        mb.calls.push(StoreCall::Delete(uid));
        match mb.messages.remove(&uid) {
            Some(_) => Ok(()),
            None => Err(MailError::Store(format!("no message with UID {uid}"))),
        }
    }

    async fn logout(&mut self) -> Result<(), MailError> {
        lock(&self.mailbox).calls.push(StoreCall::Logout);
        Ok(())
    }
}

/// Build a minimal RFC 5322 message with CRLF line endings.
pub(crate) fn rfc822(subject: &str, message_id: Option<&str>, body: &str) -> Vec<u8> {
    let mut out = String::new();
    out.push_str("From: sender@corp.test\r\n");
    out.push_str("To: inbox@corp.test\r\n");
    out.push_str(&format!("Subject: {subject}\r\n"));
    if let Some(id) = message_id {
        out.push_str(&format!("Message-ID: <{id}>\r\n"));
    }
    out.push_str("Date: Mon, 19 Oct 2026 10:00:00 +0000\r\n");
    out.push_str("Content-Type: text/plain; charset=utf-8\r\n");
    out.push_str("\r\n");
    out.push_str(body);
    out.push_str("\r\n");
    out.into_bytes()
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[tokio::test]
    async fn fake_connect_failures_then_success() {
        let mut mb = FakeMailbox::new();
        mb.connect_failures = 1;
        let c = FakeConnector::new(mb);
        assert!(matches!(c.connect().await, Err(MailError::Connect(_))));
        assert!(c.connect().await.is_ok());
        assert_eq!(
            lock(&c.mailbox).calls,
            vec![StoreCall::Connect, StoreCall::Connect]
        );
    }

    #[tokio::test]
    async fn fake_search_returns_ascending_unseen() {
        let mut mb = FakeMailbox::new()
            .with_message(5, rfc822("a", None, "x"))
            .with_message(3, rfc822("b", None, "x"))
            .with_message(9, rfc822("c", None, "x"));
        if let Some(m) = mb.messages.get_mut(&5) {
            m.seen = true;
        }
        let c = FakeConnector::new(mb);
        let mut s = tokio::time::timeout(Duration::from_secs(5), c.connect())
            .await
            .expect("connect timed out")
            .expect("connect");
        assert_eq!(s.search_unseen().await.expect("search"), vec![3, 9]);
    }

    #[tokio::test]
    async fn fake_store_round_trip() {
        let raw = rfc822("s", Some("m@x"), "b");
        let mut mb = FakeMailbox::new().with_message(3, raw.clone());
        mb.uid_validity = 9;
        mb.capabilities = StoreCapabilities {
            uidplus: true,
            move_ext: false,
        };
        let c = FakeConnector::new(mb);
        let mut s = tokio::time::timeout(Duration::from_secs(5), c.connect())
            .await
            .expect("connect timed out")
            .expect("connect");
        let caps = s.capabilities().await.expect("caps");
        assert!(caps.uidplus);
        assert!(!caps.move_ext);
        assert_eq!(s.select("INBOX").await.expect("select").uid_validity, 9);
        assert_eq!(s.search_unseen().await.expect("search"), vec![3]);
        let sizes = s.fetch_sizes(&[3]).await.expect("sizes");
        assert_eq!(sizes.len(), 1);
        assert_eq!(sizes[0].uid, 3);
        assert_eq!(sizes[0].size, raw.len() as u64);
        assert_eq!(s.fetch_body(3).await.expect("body"), Some(raw));
        s.mark_seen(3).await.expect("seen");
        s.logout().await.expect("logout");
        let mb = lock(&c.mailbox);
        assert_eq!(
            mb.calls,
            vec![
                StoreCall::Connect,
                StoreCall::Capabilities,
                StoreCall::Select("INBOX".to_string()),
                StoreCall::SearchUnseen,
                StoreCall::FetchSizes(vec![3]),
                StoreCall::FetchBody(3),
                StoreCall::MarkSeen(3),
                StoreCall::Logout,
            ]
        );
        assert!(mb.messages.get(&3).is_some_and(|m| m.seen));
    }

    #[tokio::test]
    async fn fake_move_removes_message() {
        let mb = FakeMailbox::new().with_message(3, rfc822("a", None, "x"));
        let c = FakeConnector::new(mb);
        let mut s = tokio::time::timeout(Duration::from_secs(5), c.connect())
            .await
            .expect("connect timed out")
            .expect("connect");
        s.move_to(3, "Done").await.expect("move");
        let mb = lock(&c.mailbox);
        assert_eq!(
            mb.calls.last(),
            Some(&StoreCall::Move(3, "Done".to_string()))
        );
        assert!(!mb.messages.contains_key(&3));
    }

    #[tokio::test]
    async fn fake_delete_records_uid() {
        let mb = FakeMailbox::new().with_message(3, rfc822("a", None, "x"));
        let c = FakeConnector::new(mb);
        let mut s = tokio::time::timeout(Duration::from_secs(5), c.connect())
            .await
            .expect("connect timed out")
            .expect("connect");
        s.delete_uid(3).await.expect("delete");
        let mb = lock(&c.mailbox);
        assert!(mb.calls.contains(&StoreCall::Delete(3)));
    }
}
