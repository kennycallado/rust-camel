//! Golden-chain tests for a CUSTOM xml2json stylesheet (mission 468 / bd rc-hro20).
//!
//! Apache Camel Java `xj` parity: a custom stylesheet used with
//! `direction=xml2json` produces XML; the runtime must then convert that XML
//! to JSON through the bundled identity `XML_TO_JSON_XSLT`. The chain is
//! unconditional — a custom xml2json stylesheet MUST emit XML (Java behavior;
//! no output sniffing). The scripted backend stands in for the Saxon bridge:
//! `compile` records the stylesheet bytes, `transform` replays scripted
//! results.

use async_trait::async_trait;
use camel_api::{Exchange, Message};
use camel_component_api::{Body, CamelError, Component, NoOpComponentContext, ProducerContext};
use camel_xj::{XML_TO_JSON_XSLT, XjComponent};
use camel_xslt::{BridgeState, StylesheetId, XsltBridgeClient, XsltError, XsltTransformBackend};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use tokio::sync::watch;
use tonic::transport::{Channel, Endpoint};
use tower::{Service, ServiceExt};

const CUSTOM_XSLT_BYTES: &[u8] = include_bytes!("fixtures/custom-xml2json/custom.xslt");
const INPUT_XML_BYTES: &[u8] = include_bytes!("fixtures/custom-xml2json/input.xml");
const CUSTOM_OUTPUT_XML_BYTES: &[u8] = include_bytes!("fixtures/custom-xml2json/custom-output.xml");
const EXPECTED_JSON_BYTES: &[u8] = include_bytes!("fixtures/custom-xml2json/expected.json");

fn custom_stylesheet_uri() -> String {
    format!(
        "xj:{}/tests/fixtures/custom-xml2json/custom.xslt",
        env!("CARGO_MANIFEST_DIR")
    )
}

#[derive(Debug)]
struct RecordedTransform {
    stylesheet_id: StylesheetId,
    document: Vec<u8>,
    parameters: HashMap<String, String>,
}

/// Records compiles and transforms, replays scripted transform results per
/// stylesheet id. A transform without a scripted response is a bridge error.
#[derive(Debug, Default)]
struct ScriptedBackend {
    compiled: Arc<Mutex<HashMap<StylesheetId, Vec<u8>>>>,
    transforms: Arc<Mutex<Vec<RecordedTransform>>>,
    #[allow(clippy::type_complexity)]
    script: Arc<Mutex<HashMap<StylesheetId, VecDeque<Result<Vec<u8>, XsltError>>>>>,
}

impl ScriptedBackend {
    fn script_response(&self, id: &StylesheetId, result: Result<Vec<u8>, XsltError>) {
        self.script
            .lock()
            .unwrap()
            .entry(id.clone())
            .or_default()
            .push_back(result);
    }
}

#[async_trait]
impl XsltTransformBackend for ScriptedBackend {
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
        _output_method: String,
    ) -> Result<(Vec<u8>, Option<String>), XsltError> {
        self.transforms.lock().unwrap().push(RecordedTransform {
            stylesheet_id: stylesheet_id.clone(),
            document,
            parameters,
        });
        let response = self
            .script
            .lock()
            .unwrap()
            .get_mut(&stylesheet_id)
            .and_then(|queue| queue.pop_front());
        match response {
            Some(Ok(bytes)) => Ok((bytes, None)),
            Some(Err(err)) => Err(err),
            None => Err(XsltError::Bridge(
                "no scripted response for stylesheet".to_string(),
            )),
        }
    }

    async fn recompile_all(
        &self,
        _channel: &Channel,
        _stylesheets: Vec<(StylesheetId, Vec<u8>)>,
    ) -> Result<(), XsltError> {
        Ok(())
    }
}

struct ScriptedHarness {
    component: XjComponent,
    backend: Arc<ScriptedBackend>,
}

impl ScriptedHarness {
    fn new() -> Self {
        let channel = Endpoint::from_static("http://127.0.0.1:50051").connect_lazy();
        let (state_tx, state_rx) = watch::channel(BridgeState::Ready { channel });
        let backend = Arc::new(ScriptedBackend::default());
        let client = Arc::new(XsltBridgeClient::with_backend(
            Arc::new(state_rx.clone()),
            backend.clone(),
        ));
        let component = XjComponent::with_client_for_testing(state_tx, state_rx, client);
        Self { component, backend }
    }

    fn script_response(&self, id: &StylesheetId, result: Result<Vec<u8>, XsltError>) {
        self.backend.script_response(id, result);
    }

    fn compiled_id_for(&self, bytes: &[u8]) -> StylesheetId {
        let compiled = self.backend.compiled.lock().unwrap();
        compiled
            .iter()
            .find(|(_, compiled_bytes)| compiled_bytes.as_slice() == bytes)
            .map(|(id, _)| id.clone())
            .unwrap_or_else(|| panic!("expected compiled stylesheet not found"))
    }
}

fn make_uri(direction: &str) -> String {
    format!("{}?direction={}", custom_stylesheet_uri(), direction)
}

async fn call_producer(
    component: &XjComponent,
    uri: &str,
    body: &[u8],
) -> Result<Exchange, CamelError> {
    let endpoint = component
        .create_endpoint(uri, &NoOpComponentContext)
        .expect("endpoint creation");
    let ctx = ProducerContext::new();
    let rt: Arc<dyn camel_component_api::RuntimeObservability> = Arc::new(NoOpComponentContext);
    let mut producer = endpoint
        .create_producer(rt, &ctx)
        .expect("producer creation");
    let exchange = Exchange::new(Message::new(Body::from(body.to_vec())));
    producer
        .ready()
        .await
        .expect("poll_ready")
        .call(exchange)
        .await
}

fn body_bytes(exchange: Exchange) -> Vec<u8> {
    match exchange.input.body {
        Body::Bytes(bytes) => bytes.to_vec(),
        other => panic!("expected bytes body, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn custom_xml2json_compiles_custom_and_identity_stylesheets() {
    let harness = ScriptedHarness::new();
    let uri = make_uri("xml2json");

    let _ = harness
        .component
        .create_endpoint(&uri, &NoOpComponentContext)
        .expect("endpoint creation");

    let compiled = harness.backend.compiled.lock().unwrap();
    assert_eq!(
        compiled.len(),
        2,
        "custom xml2json must compile the custom stylesheet AND the bundled XML_TO_JSON_XSLT"
    );
    assert!(
        compiled.values().any(|b| b.as_slice() == CUSTOM_XSLT_BYTES),
        "custom stylesheet must be compiled"
    );
    assert!(
        compiled
            .values()
            .any(|b| b.as_slice() == XML_TO_JSON_XSLT.as_bytes()),
        "bundled XML_TO_JSON_XSLT must be compiled for the chain step"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn custom_xml2json_chains_through_identity_xml_to_json() {
    let harness = ScriptedHarness::new();
    let uri = format!(
        "{}?direction=xml2json&param.rootTag=data",
        custom_stylesheet_uri()
    );

    let _ = harness
        .component
        .create_endpoint(&uri, &NoOpComponentContext)
        .expect("endpoint creation");
    let custom_id = harness.compiled_id_for(CUSTOM_XSLT_BYTES);
    let identity_id = harness.compiled_id_for(XML_TO_JSON_XSLT.as_bytes());
    harness.script_response(&custom_id, Ok(CUSTOM_OUTPUT_XML_BYTES.to_vec()));
    harness.script_response(&identity_id, Ok(EXPECTED_JSON_BYTES.to_vec()));

    let result = call_producer(&harness.component, &uri, INPUT_XML_BYTES).await;
    let exchange = result.expect("chained transform should succeed");
    assert_eq!(
        body_bytes(exchange),
        EXPECTED_JSON_BYTES,
        "final body must be the JSON produced by the identity conversion step"
    );

    let transforms = harness.backend.transforms.lock().unwrap();
    assert_eq!(
        transforms.len(),
        2,
        "custom xml2json must run exactly two transforms: custom XSL, then XML_TO_JSON_XSLT"
    );
    assert_eq!(transforms[0].stylesheet_id, custom_id);
    assert_eq!(
        transforms[0].document,
        INPUT_XML_BYTES.to_vec(),
        "custom step receives the original XML"
    );
    assert!(
        !transforms[0].parameters.contains_key("jsonInput"),
        "xml2json must not set jsonInput"
    );
    assert_eq!(
        transforms[0].parameters.get("rootTag").map(String::as_str),
        Some("data"),
        "user params reach the custom step"
    );
    assert_eq!(transforms[1].stylesheet_id, identity_id);
    assert_eq!(
        transforms[1].document,
        CUSTOM_OUTPUT_XML_BYTES.to_vec(),
        "identity step receives the custom stylesheet's XML output"
    );
    assert!(
        transforms[1].parameters.is_empty(),
        "identity conversion step must not receive user params or jsonInput"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn custom_xml2json_xslt_failure_preserves_typed_error() {
    // "Body unchanged on error": the producer consumes the exchange and
    // returns `Err(CamelError)` — no `Ok` exchange carrying a partial or
    // intermediate body can ever surface. The Err return is the guarantee.
    //
    // Real Saxon rejects malformed XML with a typed transform failure; the
    // scripted backend simulates that rejection. The test pins that the
    // producer forwards the raw (malformed) bytes unchanged and preserves the
    // typed error.
    let harness = ScriptedHarness::new();
    let uri = make_uri("xml2json");

    let _ = harness
        .component
        .create_endpoint(&uri, &NoOpComponentContext)
        .expect("endpoint creation");
    let custom_id = harness.compiled_id_for(CUSTOM_XSLT_BYTES);
    harness.script_response(
        &custom_id,
        Err(XsltError::TransformFailed(
            "Saxon: invalid XML document".to_string(),
        )),
    );
    let malformed = b"<order id=\"42\"><item>Widget</item>";

    let result = call_producer(&harness.component, &uri, malformed).await;
    let err = result.expect_err("custom XSLT failure must fail the exchange");
    let CamelError::ProcessorError(payload) = err else {
        panic!("expected ProcessorError, got a different CamelError variant");
    };
    assert!(
        payload.contains("transform failed"),
        "typed XSLT error must be preserved, got: {payload}"
    );
    assert!(
        payload.contains("invalid XML document"),
        "underlying bridge detail must be preserved, got: {payload}"
    );

    let transforms = harness.backend.transforms.lock().unwrap();
    assert_eq!(transforms.len(), 1);
    assert_eq!(
        transforms[0].document,
        malformed.to_vec(),
        "producer must forward the raw (malformed) document to the bridge for rejection"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn custom_xml2json_identity_step_failure_preserves_typed_error() {
    // "Body unchanged on error": the producer consumes the exchange and
    // returns `Err(CamelError)` — no `Ok` exchange carrying a partial or
    // intermediate body can ever surface. The Err return is the guarantee.
    let harness = ScriptedHarness::new();
    let uri = make_uri("xml2json");

    let _ = harness
        .component
        .create_endpoint(&uri, &NoOpComponentContext)
        .expect("endpoint creation");
    let custom_id = harness.compiled_id_for(CUSTOM_XSLT_BYTES);
    harness.script_response(&custom_id, Ok(CUSTOM_OUTPUT_XML_BYTES.to_vec()));
    let identity_id = harness.compiled_id_for(XML_TO_JSON_XSLT.as_bytes());
    harness.script_response(
        &identity_id,
        Err(XsltError::TransformFailed(
            "Saxon: identity conversion failed".to_string(),
        )),
    );

    let result = call_producer(&harness.component, &uri, INPUT_XML_BYTES).await;
    let err = result.expect_err("identity conversion failure must fail the exchange");
    let CamelError::ProcessorError(payload) = err else {
        panic!("expected ProcessorError, got a different CamelError variant");
    };
    assert!(
        payload.contains("transform failed"),
        "typed XSLT error must be preserved, got: {payload}"
    );
    assert!(
        payload.contains("identity conversion failed"),
        "underlying bridge detail must be preserved, got: {payload}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn custom_xml2json_json_emitting_output_still_chained() {
    // Apache Java parity: the chain is unconditional. A custom xml2json
    // stylesheet that already emits JSON is NOT sniffed or skipped — its
    // output is fed to the identity XML->JSON conversion verbatim, and the
    // stylesheet contract (emit XML) is documented. Real Saxon would fail
    // the second step on non-XML input; the scripted backend pins the
    // wiring: two transforms, second receives the first output as-is.
    let harness = ScriptedHarness::new();
    let uri = make_uri("xml2json");

    let _ = harness
        .component
        .create_endpoint(&uri, &NoOpComponentContext)
        .expect("endpoint creation");
    let custom_id = harness.compiled_id_for(CUSTOM_XSLT_BYTES);
    harness.script_response(&custom_id, Ok(br#"{"already":"json"}"#.to_vec()));
    let identity_id = harness.compiled_id_for(XML_TO_JSON_XSLT.as_bytes());
    harness.script_response(&identity_id, Ok(EXPECTED_JSON_BYTES.to_vec()));

    let result = call_producer(&harness.component, &uri, INPUT_XML_BYTES).await;
    let exchange = result.expect("chained transform should succeed");
    assert_eq!(
        body_bytes(exchange),
        EXPECTED_JSON_BYTES,
        "final body must come from the identity conversion step"
    );

    let transforms = harness.backend.transforms.lock().unwrap();
    assert_eq!(transforms.len(), 2, "chain must stay unconditional");
    assert_eq!(
        transforms[1].document,
        br#"{"already":"json"}"#.to_vec(),
        "second step must receive the first step's output verbatim, whatever its shape"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn custom_json2xml_unchanged_single_transform() {
    let harness = ScriptedHarness::new();
    let uri = make_uri("json2xml");

    let _ = harness
        .component
        .create_endpoint(&uri, &NoOpComponentContext)
        .expect("endpoint creation");
    {
        let compiled = harness.backend.compiled.lock().unwrap();
        assert_eq!(
            compiled.len(),
            1,
            "custom json2xml must compile ONLY the custom stylesheet (no identity chain)"
        );
        assert!(
            compiled.values().any(|b| b.as_slice() == CUSTOM_XSLT_BYTES),
            "compiled stylesheet must be the custom one"
        );
    }
    let custom_id = harness.compiled_id_for(CUSTOM_XSLT_BYTES);
    let bridge_xml = b"<result>bridged</result>".to_vec();
    harness.script_response(&custom_id, Ok(bridge_xml.clone()));

    let json = br#"{"name":"test","value":42}"#;
    let result = call_producer(&harness.component, &uri, json).await;
    let exchange = result.expect("custom json2xml should succeed");
    assert_eq!(body_bytes(exchange), bridge_xml);

    let transforms = harness.backend.transforms.lock().unwrap();
    assert_eq!(
        transforms.len(),
        1,
        "custom json2xml runs a single transform"
    );
    assert_eq!(transforms[0].stylesheet_id, custom_id);
    assert_eq!(
        transforms[0].document,
        b"<root/>".to_vec(),
        "json2xml sends <root/> as the document"
    );
    assert_eq!(
        transforms[0]
            .parameters
            .get("jsonInput")
            .map(String::as_str),
        Some(std::str::from_utf8(json).expect("fixture json is utf-8")),
        "jsonInput parameter carries the original JSON"
    );
}
