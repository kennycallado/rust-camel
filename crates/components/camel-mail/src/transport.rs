//! Transport seam between the producer and the SMTP library.

use async_trait::async_trait;

use crate::error::MailError;
use crate::message::OutboundMessage;

#[async_trait]
pub(crate) trait MailTransport: Send + Sync {
    async fn send(&self, message: &OutboundMessage) -> Result<(), MailError>;
}

#[cfg(test)]
pub(crate) struct FakeTransport {
    pub(crate) sent: std::sync::Mutex<Vec<OutboundMessage>>,
    pub(crate) fail_with: Option<String>,
}

#[cfg(test)]
impl FakeTransport {
    pub(crate) fn new(fail_with: Option<String>) -> Self {
        Self {
            sent: std::sync::Mutex::new(Vec::new()),
            fail_with,
        }
    }
}

#[cfg(test)]
#[async_trait]
impl MailTransport for FakeTransport {
    async fn send(&self, message: &OutboundMessage) -> Result<(), MailError> {
        self.sent
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(message.clone());
        match &self.fail_with {
            Some(msg) => Err(MailError::Transport(msg.clone())),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::address::MailAddress;
    use crate::config::ContentKind;
    use crate::fence::Recipients;

    fn msg() -> OutboundMessage {
        OutboundMessage {
            from: MailAddress {
                name: None,
                email: "bot@corp.test".to_string(),
            },
            recipients: Recipients {
                to: vec![MailAddress {
                    name: None,
                    email: "a@corp.test".to_string(),
                }],
                cc: Vec::new(),
                bcc: Vec::new(),
            },
            subject: None,
            content: ContentKind::Plain,
            body: "x".to_string(),
            html_alternative: None,
            extra_headers: Vec::new(),
            message_id: "<id@corp.test>".to_string(),
        }
    }

    #[tokio::test]
    async fn fake_records_and_succeeds() {
        let t = FakeTransport::new(None);
        t.send(&msg()).await.expect("ok");
        assert_eq!(t.sent.lock().expect("lock").len(), 1);
    }

    #[tokio::test]
    async fn fake_records_and_fails() {
        let t = FakeTransport::new(Some("boom".to_string()));
        let r = t.send(&msg()).await;
        assert!(matches!(r, Err(MailError::Transport(m)) if m == "boom"));
        assert_eq!(t.sent.lock().expect("lock").len(), 1);
    }
}
