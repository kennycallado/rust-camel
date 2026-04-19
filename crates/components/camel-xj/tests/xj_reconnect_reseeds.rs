use async_trait::async_trait;
use camel_bridge::reconnect::BridgeReconnectHandler;
use camel_xj::XML_TO_JSON_XSLT;
use camel_xslt::{BridgeState, StylesheetId, XsltBridgeClient, XsltError, XsltTransformBackend};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::watch;
use tonic::transport::{Channel, Endpoint};

#[derive(Debug, Default)]
struct MockBackend {
    reseed_calls: Arc<Mutex<Vec<Vec<StylesheetId>>>>,
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
        Ok((Vec::new(), None))
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
async fn reconnect_reseeds_registered_xj_stylesheet() {
    let channel = Endpoint::from_static("http://127.0.0.1:50051").connect_lazy();
    let (_, state_rx) = watch::channel(BridgeState::Ready { channel });

    let backend = Arc::new(MockBackend::default());
    let client = XsltBridgeClient::with_backend(Arc::new(state_rx), backend.clone());

    let _ = client.compile(XML_TO_JSON_XSLT.as_bytes().to_vec()).await;

    client.on_reconnect(9999).unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let calls = backend.reseed_calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].len(), 1);
}
