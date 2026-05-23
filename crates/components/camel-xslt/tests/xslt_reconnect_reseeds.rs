use async_trait::async_trait;
use camel_api::{Exchange, Message, body::Body};
use camel_bridge::reconnect::BridgeReconnectHandler;
use camel_component_api::{Component, NoOpComponentContext, ProducerContext};
use camel_xslt::{
    BridgeState, StylesheetId, XsltBridgeClient, XsltComponent, XsltComponentConfig, XsltError,
    XsltTransformBackend,
};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::watch;
use tonic::Code;
use tonic::transport::{Channel, Endpoint};
use tower::Service;

#[derive(Debug, Default)]
struct MockBackend {
    reseed_calls: Arc<Mutex<Vec<Vec<StylesheetId>>>>,
    transform_calls: Arc<AtomicU32>,
}

#[async_trait]
impl XsltTransformBackend for MockBackend {
    async fn compile(
        &self,
        _channel: Channel,
        _stylesheet_id: StylesheetId,
        _stylesheet: Vec<u8>,
    ) -> Result<Option<String>, XsltError> {
        Ok(None)
    }

    async fn transform(
        &self,
        _channel: Channel,
        _stylesheet_id: StylesheetId,
        _document: Vec<u8>,
        _parameters: HashMap<String, String>,
        _output_method: String,
    ) -> Result<(Vec<u8>, Option<String>), XsltError> {
        let call = self.transform_calls.fetch_add(1, Ordering::SeqCst) + 1;
        if call <= 3 {
            return Err(XsltError::BridgeTransport {
                code: Code::Unavailable,
                message: "transport down".to_string(),
            });
        }
        Ok((b"<ok/>".to_vec(), None))
    }

    async fn recompile_all(
        &self,
        _port: u16,
        stylesheets: Vec<(StylesheetId, Vec<u8>)>,
    ) -> Result<(), XsltError> {
        let mut guard = self.reseed_calls.lock().unwrap();
        guard.push(stylesheets.into_iter().map(|(id, _)| id).collect());
        Ok(())
    }
}

#[tokio::test]
async fn reconnect_reseeds_registered_stylesheets() {
    let channel = Endpoint::from_static("http://127.0.0.1:50051").connect_lazy();
    let (state_tx, state_rx) = watch::channel(BridgeState::Ready { channel });

    let backend = Arc::new(MockBackend::default());
    let client = Arc::new(XsltBridgeClient::with_backend(
        Arc::new(state_rx.clone()),
        backend.clone(),
    ));

    let mut cfg = XsltComponentConfig::default();
    cfg.max_retries = 3;
    let component = XsltComponent::with_client_for_testing(cfg, state_tx, state_rx, client.clone());

    let file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(file.path(), b"<xsl:stylesheet version=\"1.0\"/>").unwrap();
    let uri = format!("xslt:{}", file.path().display());
    let ep = component
        .create_endpoint(&uri, &NoOpComponentContext)
        .unwrap();
    let ctx = ProducerContext::default();
    let mut producer = ep.create_producer(&ctx).unwrap();
    let exchange = Exchange::new(Message::new(Body::Xml("<x/>".to_string())));
    let out = producer
        .call(exchange)
        .await
        .expect("should succeed after retries");
    let body = out.input.body.materialize().await.unwrap();
    assert_eq!(body.as_ref(), b"<ok/>");
    assert_eq!(backend.transform_calls.load(Ordering::SeqCst), 4);

    let _ = client.compile(b"<xsl:stylesheet id='one'/>".to_vec()).await;
    let _ = client.compile(b"<xsl:stylesheet id='two'/>".to_vec()).await;

    client.on_reconnect(9999).unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let calls = backend.reseed_calls.lock().unwrap();
    // 3 retry restarts + 1 explicit on_reconnect = 4 total.
    assert_eq!(calls.len(), 4);
    // The producer compiles its own stylesheet before the first transform attempt,
    // so each retry reconnect sees exactly 1 registered stylesheet.
    for call in calls[..3].iter() {
        assert_eq!(call.len(), 1);
    }
    // The explicit on_reconnect fires after compile("one") and compile("two"),
    // so it sees the producer stylesheet + one + two = 3 stylesheets.
    assert_eq!(calls[3].len(), 3);
}
