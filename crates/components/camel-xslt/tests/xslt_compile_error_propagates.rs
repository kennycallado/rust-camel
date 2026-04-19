use async_trait::async_trait;
use camel_api::{Exchange, Message, body::Body};
use camel_component_api::{Component, NoOpComponentContext, ProducerContext};
use camel_xslt::{
    BridgeState, StylesheetId, XsltBridgeClient, XsltComponent, XsltComponentConfig, XsltError,
    XsltTransformBackend,
};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::watch;
use tonic::transport::{Channel, Endpoint};
use tower::Service;

#[derive(Debug)]
struct CompileFailBackend;

#[async_trait]
impl XsltTransformBackend for CompileFailBackend {
    async fn compile(
        &self,
        _channel: Channel,
        _stylesheet_id: StylesheetId,
        _stylesheet: Vec<u8>,
    ) -> Result<Option<String>, XsltError> {
        Ok(Some("compile error from mock backend".to_string()))
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

#[tokio::test]
async fn compile_error_surfaces_on_first_transform() {
    let endpoint = Endpoint::from_static("http://127.0.0.1:50051");
    let channel = endpoint.connect_lazy();
    let (state_tx, state_rx) = watch::channel(BridgeState::Ready { channel });

    let client = Arc::new(XsltBridgeClient::with_backend(
        Arc::new(state_rx.clone()),
        Arc::new(CompileFailBackend),
    ));

    let component = XsltComponent::with_client_for_testing(
        XsltComponentConfig::default(),
        state_tx,
        state_rx,
        client,
    );

    let file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(file.path(), b"<xsl:stylesheet version=\"1.0\"/>").unwrap();
    let uri = format!("xslt:{}", file.path().display());

    // create_endpoint now succeeds (lazy init)
    let ep = component
        .create_endpoint(&uri, &NoOpComponentContext)
        .expect("endpoint creation should succeed with lazy init");

    let ctx = ProducerContext::default();
    let mut producer = ep
        .create_producer(&ctx)
        .expect("producer creation should succeed");

    let exchange = Exchange::new(Message::new(Body::Xml("<x/>".to_string())));
    let err = producer
        .call(exchange)
        .await
        .expect_err("compile error should surface on first call");

    assert!(
        err.to_string().to_lowercase().contains("compile"),
        "got: {err}"
    );
}
