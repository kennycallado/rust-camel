use async_trait::async_trait;
use camel_xj::XjComponent;
use camel_xslt::{
    BridgeState, StylesheetId, XsltBridgeClient, XsltError, XsltTransformBackend,
};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::watch;
use tonic::transport::{Channel, Endpoint};

#[derive(Debug, Default)]
pub struct RecordedTransform {
    pub stylesheet_id: StylesheetId,
    pub document: Vec<u8>,
    pub parameters: HashMap<String, String>,
    pub output_method: String,
}

#[derive(Debug, Default)]
pub struct TransformRecorderBackend {
    compiled: Arc<Mutex<HashMap<StylesheetId, Vec<u8>>>>,
    transforms: Arc<Mutex<Vec<RecordedTransform>>>,
}

impl TransformRecorderBackend {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn compiled_stylesheets(&self) -> Arc<Mutex<HashMap<StylesheetId, Vec<u8>>>> {
        Arc::clone(&self.compiled)
    }

    pub fn recorded_transforms(&self) -> Arc<Mutex<Vec<RecordedTransform>>> {
        Arc::clone(&self.transforms)
    }
}

#[async_trait]
impl XsltTransformBackend for TransformRecorderBackend {
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
        stylesheet_id: StylesheetId,
        document: Vec<u8>,
        parameters: HashMap<String, String>,
        output_method: String,
    ) -> Result<(Vec<u8>, Option<String>), XsltError> {
        self.transforms.lock().unwrap().push(RecordedTransform {
            stylesheet_id,
            document,
            parameters,
            output_method,
        });
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

pub struct XjTestHarness {
    pub component: XjComponent,
    pub backend: Arc<TransformRecorderBackend>,
}

impl XjTestHarness {
    pub fn new() -> Self {
        let channel = Endpoint::from_static("http://127.0.0.1:50051").connect_lazy();
        let (state_tx, state_rx) = watch::channel(BridgeState::Ready { channel });
        let backend = Arc::new(TransformRecorderBackend::new());
        let client = Arc::new(XsltBridgeClient::with_backend(
            Arc::new(state_rx.clone()),
            backend.clone(),
        ));
        let component = XjComponent::with_client_for_testing(state_tx, state_rx, client);
        Self { component, backend }
    }
}
