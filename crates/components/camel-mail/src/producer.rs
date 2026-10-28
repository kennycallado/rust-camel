//! SMTP producer: one exchange, one send, no retry.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use camel_api::{CamelError, Exchange};
use camel_component_api::RuntimeObservability;
use tower::Service;

use crate::config::SmtpEndpointConfig;
use crate::headers::CAMEL_MAIL_MESSAGE_ID;
use crate::message;
use crate::transport::MailTransport;

/// Tower producer sending each exchange as one mail message.
#[derive(Clone)]
pub(crate) struct SmtpProducer {
    config: Arc<SmtpEndpointConfig>,
    transport: Arc<dyn MailTransport>,
    rt: Arc<dyn RuntimeObservability>,
}

impl SmtpProducer {
    pub(crate) fn new(
        config: Arc<SmtpEndpointConfig>,
        transport: Arc<dyn MailTransport>,
        rt: Arc<dyn RuntimeObservability>,
    ) -> Self {
        Self {
            config,
            transport,
            rt,
        }
    }
}

/// Emit one `component="mail", operation="send"` observation.
pub(crate) fn record_send(rt: &dyn RuntimeObservability, failed: bool) {
    rt.component_metrics().observe("mail", "send", failed);
}

impl Service<Exchange> for SmtpProducer {
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, mut exchange: Exchange) -> Self::Future {
        let config = Arc::clone(&self.config);
        let transport = Arc::clone(&self.transport);
        let rt = Arc::clone(&self.rt);
        Box::pin(async move {
            let msg = message::build_outbound(&config, &exchange).map_err(CamelError::from)?;
            let result = transport.send(&msg).await;
            record_send(&*rt, result.is_err());
            match result {
                Ok(()) => {
                    exchange
                        .input
                        .set_header(CAMEL_MAIL_MESSAGE_ID, msg.message_id.clone());
                    Ok(exchange)
                }
                Err(e) => Err(CamelError::from(e)),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::FakeTransport;
    use camel_api::Message;
    use camel_component_api::test_support::RecordingRuntimeObservability;

    fn producer(
        uri: &str,
        fake: &Arc<FakeTransport>,
        rt: &Arc<RecordingRuntimeObservability>,
    ) -> SmtpProducer {
        let cfg = Arc::new(SmtpEndpointConfig::from_uri(uri).expect("uri"));
        let transport: Arc<dyn MailTransport> = fake.clone();
        let rt: Arc<dyn RuntimeObservability> = rt.clone();
        SmtpProducer::new(cfg, transport, rt)
    }

    const URI: &str = "smtp://h:2525?from=a@x.test&to=b@x.test&allowCleartext=true";

    fn exchange() -> Exchange {
        Exchange::new(Message::new("hello"))
    }

    #[tokio::test]
    async fn send_outcome_counted() {
        let rt = RecordingRuntimeObservability::new(true);
        let ok = Arc::new(FakeTransport::new(None));
        producer(URI, &ok, &rt)
            .call(exchange())
            .await
            .expect("sent");
        let bad = Arc::new(FakeTransport::new(Some("boom".into())));
        assert!(producer(URI, &bad, &rt).call(exchange()).await.is_err());
        assert_eq!(
            rt.ops(),
            vec![
                ("mail".into(), "send".into(), "success".into()),
                ("mail".into(), "send".into(), "failure".into())
            ]
        );
    }

    #[tokio::test]
    async fn message_id_header_set_after_accept() {
        let rt = RecordingRuntimeObservability::new(true);
        let fake = Arc::new(FakeTransport::new(None));
        let out = producer(URI, &fake, &rt)
            .call(exchange())
            .await
            .expect("sent");
        let id = out
            .input
            .header(CAMEL_MAIL_MESSAGE_ID)
            .and_then(|v| v.as_str().map(str::to_string))
            .expect("id header");
        assert!(!id.is_empty());
        let sent = fake.sent.lock().expect("lock");
        assert_eq!(sent[0].message_id, id);
    }

    #[tokio::test]
    async fn no_recipient_sends_nothing() {
        let rt = RecordingRuntimeObservability::new(true);
        let fake = Arc::new(FakeTransport::new(None));
        let uri = "smtp://h:2525?from=a@x.test&allowCleartext=true";
        assert!(producer(uri, &fake, &rt).call(exchange()).await.is_err());
        assert!(fake.sent.lock().expect("lock").is_empty());
        assert!(rt.ops().is_empty());
    }

    #[tokio::test]
    async fn failed_send_is_not_retried() {
        let rt = RecordingRuntimeObservability::new(true);
        let fake = Arc::new(FakeTransport::new(Some("421".into())));
        assert!(producer(URI, &fake, &rt).call(exchange()).await.is_err());
        assert_eq!(fake.sent.lock().expect("lock").len(), 1);
    }
}
