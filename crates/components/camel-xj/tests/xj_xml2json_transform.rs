use async_trait::async_trait;
use camel_component_api::{Component, NoOpComponentContext};
use camel_xj::{XML_TO_JSON_XSLT, XjComponent};
use camel_xslt::{BridgeState, StylesheetId, XsltBridgeClient, XsltError, XsltTransformBackend};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::watch;
use tonic::transport::{Channel, Endpoint};

#[derive(Debug, Default)]
struct MockBackend {
    compiled: Arc<Mutex<HashMap<StylesheetId, Vec<u8>>>>,
}

#[async_trait]
impl XsltTransformBackend for MockBackend {
    async fn compile(
        &self,
        _channel: Channel,
        stylesheet_id: StylesheetId,
        stylesheet: Vec<u8>,
    ) -> Result<Option<String>, XsltError> {
        self.compiled
            .lock()
            .unwrap()
            .insert(stylesheet_id, stylesheet);
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
        _stylesheets: Vec<(StylesheetId, Vec<u8>)>,
    ) -> Result<(), XsltError> {
        Ok(())
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn xml2json_uses_xml_to_json_identity_stylesheet() {
    let channel = Endpoint::from_static("http://127.0.0.1:50051").connect_lazy();
    let (state_tx, state_rx) = watch::channel(BridgeState::Ready { channel });
    let backend = Arc::new(MockBackend::default());
    let client = Arc::new(XsltBridgeClient::with_backend(
        Arc::new(state_rx.clone()),
        backend.clone(),
    ));
    let component = XjComponent::with_client_for_testing(state_tx, state_rx, client);

    let _ = component
        .create_endpoint(
            "xj:classpath:identity?direction=xml2json",
            &NoOpComponentContext,
        )
        .expect("endpoint should compile identity stylesheet");

    let compiled = backend.compiled.lock().unwrap();
    assert_eq!(compiled.len(), 1);
    let stylesheet = compiled.values().next().unwrap();
    assert_eq!(stylesheet.as_slice(), XML_TO_JSON_XSLT.as_bytes());
}
