use async_trait::async_trait;
use camel_component_api::{Component, NoOpComponentContext};
use camel_xj::XjComponent;
use camel_xslt::{BridgeState, StylesheetId, XsltBridgeClient, XsltError, XsltTransformBackend};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::watch;
use tonic::transport::{Channel, Endpoint};

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

#[tokio::test(flavor = "multi_thread")]
async fn endpoint_creation_surfaces_compile_error() {
    let channel = Endpoint::from_static("http://127.0.0.1:50051").connect_lazy();
    let (state_tx, state_rx) = watch::channel(BridgeState::Ready { channel });

    let client = Arc::new(XsltBridgeClient::with_backend(
        Arc::new(state_rx.clone()),
        Arc::new(CompileFailBackend),
    ));

    let component = XjComponent::with_client_for_testing(state_tx, state_rx, client);

    let err = match component.create_endpoint(
        "xj:classpath:identity?direction=xml2json",
        &NoOpComponentContext,
    ) {
        Ok(_) => panic!("expected compile error to be propagated"),
        Err(err) => err,
    };

    assert!(
        err.to_string().to_lowercase().contains("compile"),
        "got: {err}"
    );
}
