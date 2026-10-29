use crate::client::{BridgeState, StylesheetId, XsltBridgeClient};
use crate::component::XsltBridgeRuntime;
use camel_api::body::DEFAULT_MATERIALIZE_LIMIT;
use camel_component_api::{Body, CamelError, Exchange};
use camel_language_api::Expression;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use tokio::sync::OnceCell;
use tower::Service;
use tracing::{debug, warn};

/// Safe diagnostic tail for a language error.
///
/// Only trusted metadata is rendered: the typed class and the contractually
/// redacted detail. The raw `Display` of the error is never used because
/// variants such as `EvalError` and `ParseError` can embed exchange or
/// expression text.
pub(crate) fn language_error_tail(err: &camel_language_api::LanguageError) -> String {
    let class_suffix = err
        .class()
        .map(|class| format!(" [{class}]"))
        .unwrap_or_default();
    let detail = err
        .redacted_detail()
        .unwrap_or_else(|| "details withheld".to_string());
    format!("{class_suffix}: {detail}")
}

/// Resolve the XSLT parameters for a single exchange.
///
/// Parameters are resolved in three layers, then collected by the bridge client
/// into a `HashMap` where later entries win:
///
/// 1. Static `param.<name>=<value>` entries, always present.
/// 2. `paramExpr.<name>=<language>:<expression>` entries, evaluated per
///    exchange and appended after the statics.
/// 3. `paramFrom.<name>=<header-key>` entries, appended last.
///
/// Because the bridge map is later-wins, the effective precedence for a given
/// name is `header > paramExpr > static`. A missing or JSON-null value in a
/// higher layer contributes no entry, so resolution falls through to the layer
/// below. Concretely:
///
/// | static | paramExpr   | header    | result   |
/// |--------|-------------|-----------|----------|
/// | `x`    | absent      | absent    | `x`      |
/// | `x`    | `42`        | absent    | `42`     |
/// | `x`    | `42`        | `9`       | `9`      |
/// | `x`    | `null`      | absent    | `x`      |
/// | `x`    | `42`        | `null`    | `42`     |
/// | absent | `42`        | absent    | `42`     |
///
/// Only scalar values (string, number, boolean) can be mapped from either
/// source. Array and object values fail closed: the returned error names the
/// parameter (and, for `paramFrom`, the header key) and the JSON type, but never
/// the value content. An expression evaluation error aborts resolution with a
/// `ProcessorError` that names the parameter and the language error's redacted
/// classification, but never the raw language error text or exchange content.
pub(crate) async fn resolve_params(
    statics: &[(String, String)],
    param_expr: &[(String, Arc<dyn Expression>)],
    param_from: &[(String, String)],
    exchange: &Exchange,
) -> Result<Vec<(String, String)>, CamelError> {
    let mut resolved = statics.to_vec();

    for (name, expression) in param_expr {
        let value = expression.evaluate(exchange).await.map_err(|lang_err| {
            CamelError::ProcessorError(format!(
                "XSLT parameter '{name}' from expression failed{}",
                language_error_tail(&lang_err)
            ))
        })?;
        match value {
            camel_api::value::Value::Null => {}
            camel_api::value::Value::String(s) => resolved.push((name.clone(), s)),
            camel_api::value::Value::Number(n) => resolved.push((name.clone(), n.to_string())),
            camel_api::value::Value::Bool(b) => resolved.push((name.clone(), b.to_string())),
            other @ (camel_api::value::Value::Array(_) | camel_api::value::Value::Object(_)) => {
                return Err(CamelError::ProcessorError(format!(
                    "XSLT parameter '{name}' from expression has unsupported type '{}': only \
                     string, number, and boolean values are supported",
                    camel_api::value::value_type_name(&other)
                )));
            }
        }
    }

    for (name, header_key) in param_from {
        let Some(value) = exchange.input.headers.get(header_key) else {
            continue;
        };
        match value {
            camel_api::value::Value::Null => {}
            camel_api::value::Value::String(s) => resolved.push((name.clone(), s.clone())),
            camel_api::value::Value::Number(n) => resolved.push((name.clone(), n.to_string())),
            camel_api::value::Value::Bool(b) => resolved.push((name.clone(), b.to_string())),
            other @ (camel_api::value::Value::Array(_) | camel_api::value::Value::Object(_)) => {
                return Err(CamelError::ProcessorError(format!(
                    "XSLT parameter '{name}' mapped from header '{header_key}' has unsupported \
                     type '{}': only string, number, and boolean headers are supported",
                    camel_api::value::value_type_name(other)
                )));
            }
        }
    }

    Ok(resolved)
}

/// Tower `Service` that applies an XSLT 3.0 transformation to each exchange.
///
/// # Body Contract
///
/// **Input:** The exchange body MUST contain well-formed XML (as `Body::Xml`,
/// `Body::Bytes`, or any variant that materializes to valid UTF-8 / XML bytes).
/// If `fail_on_null_body` is enabled in the endpoint configuration and the body
/// is empty, the producer returns `CamelError::ProcessorError`.
///
/// **Output:** The exchange body is replaced with the transformation result as
/// `Body::Bytes` containing the raw output (XML, HTML, or plain text depending
/// on the stylesheet's `xsl:output` declaration). Exchange headers are preserved
/// unchanged.
///
/// # TODO(XSLT-012): Output encoding validation
/// The encoding declared in the XSLT stylesheet's `<xsl:output encoding="...">`
/// is not validated against the runtime charset. The transformer always produces
/// bytes according to the stylesheet's `encoding` attribute (defaulting to UTF-8),
/// but this crate does not verify that the declared encoding matches the actual
/// byte content. If a stylesheet declares `encoding="ISO-8859-1"` but the bridge
/// produces UTF-8 bytes (or vice versa), the output may be misinterpreted by
/// downstream consumers. A future enhancement should parse the `xsl:output`
/// declaration and either enforce encoding consistency or annotate the exchange
/// body with the effective charset.
#[derive(Clone)]
pub struct XsltProducer {
    stylesheet_bytes: Vec<u8>,
    compiled: Arc<OnceCell<StylesheetId>>,
    params: Vec<(String, String)>,
    param_from: Vec<(String, String)>,
    // Compiled once at endpoint creation, evaluated per exchange in `call`.
    param_expr: Vec<(String, Arc<dyn Expression>)>,
    output_method: Option<String>,
    fail_on_null_body: bool,
    max_payload_bytes: Option<usize>,
    client: Arc<XsltBridgeClient>,
    runtime: Arc<XsltBridgeRuntime>,
}

impl XsltProducer {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        stylesheet_bytes: Vec<u8>,
        compiled: Arc<OnceCell<StylesheetId>>,
        params: Vec<(String, String)>,
        param_from: Vec<(String, String)>,
        param_expr: Vec<(String, Arc<dyn Expression>)>,
        output_method: Option<String>,
        fail_on_null_body: bool,
        max_payload_bytes: Option<usize>,
        client: Arc<XsltBridgeClient>,
        runtime: Arc<XsltBridgeRuntime>,
    ) -> Self {
        Self {
            stylesheet_bytes,
            compiled,
            params,
            param_from,
            param_expr,
            output_method,
            fail_on_null_body,
            max_payload_bytes,
            client,
            runtime,
        }
    }
}

impl Service<Exchange> for XsltProducer {
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        match &*self.runtime.state_rx().borrow() {
            BridgeState::Ready { .. } => Poll::Ready(Ok(())),
            BridgeState::Starting => {
                // Bridge hasn't been started yet — trigger start now and wake when done.
                // `ensure_started_or_degrade` is idempotent (`start_lock` gates concurrent
                // starts) and transitions to Degraded on failure so we never loop in Starting.
                let waker = cx.waker().clone();
                let runtime = Arc::clone(&self.runtime);
                let client = Arc::clone(&self.client);
                if let Ok(handle) = tokio::runtime::Handle::try_current() {
                    handle.spawn(async move {
                        runtime.ensure_started_or_degrade(client.as_ref()).await;
                        waker.wake();
                    });
                } else {
                    waker.wake_by_ref();
                }
                Poll::Pending
            }
            BridgeState::Restarting { .. } => {
                // Restart already triggered by call() after a transport error.
                // Just wait for the state to change (to Ready or Degraded).
                let waker = cx.waker().clone();
                let state_rx_arc = Arc::clone(self.runtime.state_rx());
                if let Ok(handle) = tokio::runtime::Handle::try_current() {
                    handle.spawn(async move {
                        let mut rx = (*state_rx_arc).clone();
                        let _ = rx.changed().await;
                        waker.wake();
                    });
                } else {
                    waker.wake_by_ref();
                }
                Poll::Pending
            }
            BridgeState::Degraded(reason) => {
                warn!(reason = %reason, "xslt bridge degraded");
                Poll::Ready(Err(CamelError::ProcessorError(format!(
                    "xslt bridge degraded: {reason}"
                ))))
            }
            BridgeState::Stopped => {
                warn!("xslt bridge stopped");
                Poll::Ready(Err(CamelError::ProcessorError(
                    "xslt bridge stopped".to_string(),
                )))
            }
        }
    }

    fn call(&mut self, mut exchange: Exchange) -> Self::Future {
        let stylesheet_bytes = self.stylesheet_bytes.clone();
        let compiled = Arc::clone(&self.compiled);
        let statics = self.params.clone();
        let param_expr = self.param_expr.clone();
        let param_from = self.param_from.clone();
        let output_method = self.output_method.clone();
        let fail_on_null_body = self.fail_on_null_body;
        let max_payload_bytes = self.max_payload_bytes;
        let client = Arc::clone(&self.client);
        let runtime = Arc::clone(&self.runtime);

        Box::pin(async move {
            // Per-exchange resolution happens before the stylesheet init and
            // before the body is taken so a rejected header leaves the
            // exchange untouched.
            let params = match resolve_params(&statics, &param_expr, &param_from, &exchange).await {
                Ok(params) => params,
                Err(err) => {
                    // log-policy: handler-owned
                    warn!(error = %err, "xslt parameter resolution failed");
                    return Err(err);
                }
            };

            // Lazy: start bridge and compile stylesheet on first call
            let stylesheet_id =
                compiled
                    .get_or_try_init(|| async {
                        debug!("xslt bridge starting");
                        runtime
                            .ensure_bridge_started(client.as_ref())
                            .await
                            .map_err(|e: crate::error::XsltError| {
                                CamelError::ProcessorError(e.to_string())
                            })?;
                        debug!("stylesheet compiled");
                        client.compile(stylesheet_bytes).await.map_err(
                            |e: crate::error::XsltError| CamelError::ProcessorError(e.to_string()),
                        )
                    })
                    .await
                    .map_err(|err| {
                        // log-policy: handler-owned
                        warn!(error = %err, "stylesheet compilation failed");
                        err
                    })?;

            let input_body = std::mem::take(&mut exchange.input.body);

            if fail_on_null_body && input_body.is_empty() {
                return Err(CamelError::ProcessorError(
                    "XSLT producer received null/empty body but failOnNullBody is enabled"
                        .to_string(),
                ));
            }

            let effective = max_payload_bytes.unwrap_or(DEFAULT_MATERIALIZE_LIMIT);
            let document = input_body
                .into_bytes(effective)
                .await
                .map_err(|e| CamelError::ProcessorError(format!("XSLT input body error: {e}")))?
                .to_vec();

            let transformed = runtime
                .transform_with_retry(&client, stylesheet_id, document, params, output_method)
                .await
                .map_err(|e| {
                    let err = CamelError::ProcessorError(e.to_string());
                    // log-policy: handler-owned
                    warn!(error = %err, "xslt transform failed");
                    err
                })?;

            exchange.input.body = Body::from(transformed);
            Ok(exchange)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::{StylesheetId, XsltTransformBackend};
    use crate::component::XsltBridgeRuntime;
    use crate::config::XsltComponentConfig;
    use camel_api::Message;
    use camel_api::value::Value;
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::task::{RawWaker, RawWakerVTable, Waker};
    use tokio::sync::{Mutex, watch};
    use tonic::transport::Channel;
    use tower::ServiceExt;

    type CaptureLog = Arc<std::sync::Mutex<Vec<HashMap<String, String>>>>;

    /// Test backend that records the parameters of every transform call and
    /// returns a trivial echo document. Assertions in these tests are made on
    /// the captured maps, not on the echoed bytes.
    #[derive(Debug)]
    struct RecordingBackend {
        captures: CaptureLog,
    }

    impl RecordingBackend {
        fn new() -> Self {
            Self {
                captures: Arc::new(std::sync::Mutex::new(Vec::new())),
            }
        }

        fn captures(&self) -> CaptureLog {
            Arc::clone(&self.captures)
        }
    }

    #[async_trait::async_trait]
    impl XsltTransformBackend for RecordingBackend {
        async fn compile(
            &self,
            _channel: Channel,
            _stylesheet_id: StylesheetId,
            _stylesheet: Vec<u8>,
        ) -> Result<Option<String>, crate::error::XsltError> {
            Ok(None)
        }

        async fn transform(
            &self,
            _channel: Channel,
            _stylesheet_id: StylesheetId,
            _document: Vec<u8>,
            parameters: HashMap<String, String>,
            _output_method: String,
        ) -> Result<(Vec<u8>, Option<String>), crate::error::XsltError> {
            let mut sorted: Vec<_> = parameters.iter().collect();
            sorted.sort_by(|a, b| a.0.cmp(b.0));
            let rendered = sorted
                .iter()
                .map(|(key, value)| format!("<param key=\"{key}\">{value}</param>"))
                .collect::<String>();
            let echo = format!("<out>{rendered}</out>").into_bytes();
            self.captures
                .lock()
                .expect("capture log mutex poisoned")
                .push(parameters);
            Ok((echo, None))
        }

        async fn recompile_all(
            &self,
            _channel: &Channel,
            _stylesheets: Vec<(StylesheetId, Vec<u8>)>,
        ) -> Result<(), crate::error::XsltError> {
            Ok(())
        }
    }

    /// Scriptable value emitted by [`TestExpression`].
    enum TestOutput {
        /// Emit the given value unchanged.
        Emit(Value),
        /// Fail evaluation with a free-form `EvalError` carrying a sentinel.
        Fail,
        /// Fail evaluation with a contractually redacted `EvalFailure` detail.
        RedactedFail,
        /// Await the shared barrier, then echo the named exchange header.
        BarrierEchoHeader {
            key: &'static str,
            barrier: Arc<tokio::sync::Barrier>,
        },
    }

    /// `Expression` fake that drives `paramExpr` resolution without a language
    /// runtime.
    struct TestExpression(TestOutput);

    #[async_trait::async_trait]
    impl Expression for TestExpression {
        async fn evaluate(
            &self,
            exchange: &Exchange,
        ) -> Result<Value, camel_language_api::LanguageError> {
            match &self.0 {
                TestOutput::Emit(value) => Ok(value.clone()),
                TestOutput::Fail => Err(camel_language_api::LanguageError::EvalError(
                    "SECRET-SENTINEL exchange-body-text".to_string(),
                )),
                TestOutput::RedactedFail => Err(camel_language_api::LanguageError::EvalFailure {
                    class: camel_api::ExpressionErrorClass::Runtime,
                    position: None,
                    detail: Some("redacted-ok".to_string()),
                }),
                TestOutput::BarrierEchoHeader { key, barrier } => {
                    barrier.wait().await;
                    Ok(exchange
                        .input
                        .headers
                        .get(*key)
                        .cloned()
                        .unwrap_or(Value::Null))
                }
            }
        }
    }

    fn test_expr(output: TestOutput) -> Arc<dyn Expression> {
        Arc::new(TestExpression(output))
    }

    fn param_expr_set(pairs: Vec<(&str, TestOutput)>) -> Vec<(String, Arc<dyn Expression>)> {
        pairs
            .into_iter()
            .map(|(name, output)| (name.to_string(), test_expr(output)))
            .collect()
    }

    fn make_recording_producer(
        statics: Vec<(String, String)>,
        param_from: Vec<(String, String)>,
    ) -> (XsltProducer, CaptureLog) {
        make_recording_producer_with_expr(statics, Vec::new(), param_from)
    }

    fn make_recording_producer_with_expr(
        statics: Vec<(String, String)>,
        param_expr: Vec<(String, Arc<dyn Expression>)>,
        param_from: Vec<(String, String)>,
    ) -> (XsltProducer, CaptureLog) {
        let channel = tonic::transport::Channel::from_static("http://[::]:0").connect_lazy();
        let (state_tx, state_rx) = watch::channel(BridgeState::Ready { channel });
        let state_rx_arc = Arc::new(state_rx);
        let backend = Arc::new(RecordingBackend::new());
        let captures = backend.captures();
        let client = Arc::new(XsltBridgeClient::with_backend(
            Arc::clone(&state_rx_arc),
            backend,
        ));
        let runtime = Arc::new(XsltBridgeRuntime::new(
            XsltComponentConfig::default(),
            Arc::new(Mutex::new(None)),
            state_tx,
            state_rx_arc,
        ));
        let compiled = Arc::new(OnceCell::new());
        let _ = compiled.set("test-stylesheet".to_string());

        let producer = XsltProducer::new(
            vec![],
            compiled,
            statics,
            param_from,
            param_expr,
            None,
            false,
            None,
            client,
            runtime,
        );

        (producer, captures)
    }

    fn exchange_with_header(key: &str, value: Value) -> Exchange {
        let mut exchange = Exchange::new(Message::new(Body::from("<a/>")));
        exchange.input.headers.insert(key.to_string(), value);
        exchange
    }

    fn exchange_without_header() -> Exchange {
        Exchange::new(Message::new(Body::from("<a/>")))
    }

    async fn call_producer(
        producer: &mut XsltProducer,
        exchange: Exchange,
    ) -> Result<Exchange, CamelError> {
        let svc = &mut *producer;
        svc.ready()
            .await
            .expect("producer should be ready")
            .call(exchange)
            .await
    }

    fn capture_snapshot(captures: &CaptureLog) -> Vec<HashMap<String, String>> {
        captures.lock().expect("capture log mutex poisoned").clone()
    }

    fn param_set(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(name, key)| ((*name).to_string(), (*key).to_string()))
            .collect()
    }

    fn noop_waker() -> Waker {
        const VTABLE: RawWakerVTable = RawWakerVTable::new(
            |_| RawWaker::new(std::ptr::null(), &VTABLE),
            |_| {},
            |_| {},
            |_| {},
        );
        unsafe { Waker::from_raw(RawWaker::new(std::ptr::null(), &VTABLE)) }
    }

    fn make_producer_with_state(
        initial_state: BridgeState,
        max_payload_bytes: Option<usize>,
    ) -> XsltProducer {
        let (state_tx, state_rx) = watch::channel(initial_state);
        let state_rx_arc = Arc::new(state_rx);
        let client = Arc::new(XsltBridgeClient::new(Arc::clone(&state_rx_arc)));
        let runtime = Arc::new(XsltBridgeRuntime::new(
            XsltComponentConfig::default(),
            Arc::new(Mutex::new(None)),
            state_tx,
            state_rx_arc,
        ));
        XsltProducer::new(
            vec![],
            Arc::new(OnceCell::new()),
            vec![],
            vec![],
            vec![],
            None,
            false,
            max_payload_bytes,
            client,
            runtime,
        )
    }

    #[test]
    fn test_poll_ready_bridge_starting() {
        let mut producer = make_producer_with_state(BridgeState::Starting, None);
        let result = producer.poll_ready(&mut Context::from_waker(&noop_waker()));
        assert!(matches!(result, Poll::Pending));
    }

    #[tokio::test]
    async fn test_poll_ready_bridge_ready() {
        let channel = tonic::transport::Channel::from_static("http://[::]:0").connect_lazy();
        let mut producer = make_producer_with_state(BridgeState::Ready { channel }, None);
        let result = producer.poll_ready(&mut Context::from_waker(&noop_waker()));
        assert!(matches!(result, Poll::Ready(Ok(()))));
    }

    #[test]
    fn test_poll_ready_bridge_degraded() {
        let mut producer =
            make_producer_with_state(BridgeState::Degraded("connection lost".to_string()), None);
        let result = producer.poll_ready(&mut Context::from_waker(&noop_waker()));
        assert!(matches!(result, Poll::Ready(Err(_))));
    }

    #[test]
    fn test_poll_ready_bridge_stopped() {
        let mut producer = make_producer_with_state(BridgeState::Stopped, None);
        let result = producer.poll_ready(&mut Context::from_waker(&noop_waker()));
        assert!(matches!(result, Poll::Ready(Err(_))));
    }

    #[tokio::test]
    async fn test_producer_rejects_oversized_payload() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let channel = tonic::transport::Channel::from_static("http://[::]:0").connect_lazy();
        let (state_tx, state_rx) = watch::channel(BridgeState::Ready { channel });
        let state_rx_arc = Arc::new(state_rx);
        let client = Arc::new(XsltBridgeClient::new(Arc::clone(&state_rx_arc)));
        let runtime = Arc::new(XsltBridgeRuntime::new(
            XsltComponentConfig::default(),
            Arc::new(Mutex::new(None)),
            state_tx,
            state_rx_arc,
        ));
        let compiled = Arc::new(OnceCell::new());
        let _ = compiled.set("test-stylesheet".to_string());

        let mut producer = XsltProducer::new(
            vec![],
            compiled,
            vec![],
            vec![],
            vec![],
            None,
            false,
            Some(100),
            client,
            runtime,
        );

        let big_body = "x".repeat(200).into_bytes();
        let exchange = Exchange::new(Message::new(Body::from(big_body)));

        let result = producer.ready().await.unwrap().call(exchange).await;
        let err = result.expect_err("should reject oversized payload");
        let msg = err.to_string();
        assert!(
            msg.contains("Stream size exceeded limit"),
            "expected stream-limit message, got: {msg}"
        );
    }

    #[tokio::test]
    async fn test_producer_allows_within_limit() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let channel = tonic::transport::Channel::from_static("http://[::]:0").connect_lazy();
        let (state_tx, state_rx) = watch::channel(BridgeState::Ready { channel });
        let state_rx_arc = Arc::new(state_rx);
        let client = Arc::new(XsltBridgeClient::new(Arc::clone(&state_rx_arc)));
        let runtime = Arc::new(XsltBridgeRuntime::new(
            XsltComponentConfig::default(),
            Arc::new(Mutex::new(None)),
            state_tx,
            state_rx_arc,
        ));
        let compiled = Arc::new(OnceCell::new());
        let _ = compiled.set("test-stylesheet".to_string());

        let mut producer = XsltProducer::new(
            vec![],
            compiled,
            vec![],
            vec![],
            vec![],
            None,
            false,
            Some(1000),
            client,
            runtime,
        );

        let body = "x".repeat(500).into_bytes();
        let exchange = Exchange::new(Message::new(Body::from(body)));

        let result = producer.ready().await.unwrap().call(exchange).await;
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            !err_msg.contains("Stream size exceeded limit"),
            "should NOT reject within-limit payload, got: {err_msg}"
        );
    }

    #[tokio::test]
    async fn dynamic_header_value_reaches_transform() {
        let (mut producer, captures) =
            make_recording_producer(vec![], param_set(&[("sequenceNumber", "seqHeader")]));

        call_producer(
            &mut producer,
            exchange_with_header("seqHeader", Value::String("42".into())),
        )
        .await
        .expect("transform should succeed");

        let log = capture_snapshot(&captures);
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].get("sequenceNumber").map(String::as_str), Some("42"));
    }

    #[tokio::test]
    async fn missing_header_omits_param() {
        let (mut producer, captures) =
            make_recording_producer(vec![], param_set(&[("sequenceNumber", "seqHeader")]));

        call_producer(&mut producer, exchange_without_header())
            .await
            .expect("transform should succeed");

        let log = capture_snapshot(&captures);
        assert_eq!(log.len(), 1);
        assert!(log[0].is_empty(), "missing header must omit param: {log:?}");
    }

    #[tokio::test]
    async fn null_header_omits_param() {
        let (mut producer, captures) =
            make_recording_producer(vec![], param_set(&[("sequenceNumber", "seqHeader")]));

        call_producer(
            &mut producer,
            exchange_with_header("seqHeader", Value::Null),
        )
        .await
        .expect("transform should succeed");

        let log = capture_snapshot(&captures);
        assert_eq!(log.len(), 1);
        assert!(log[0].is_empty(), "null header must omit param: {log:?}");
    }

    #[tokio::test]
    async fn null_header_falls_back_to_static() {
        let (mut producer, captures) = make_recording_producer(
            vec![("sequenceNumber".to_string(), "0".to_string())],
            param_set(&[("sequenceNumber", "seqHeader")]),
        );

        call_producer(
            &mut producer,
            exchange_with_header("seqHeader", Value::Null),
        )
        .await
        .expect("transform should succeed");

        let log = capture_snapshot(&captures);
        assert_eq!(
            log[0],
            HashMap::from([("sequenceNumber".to_string(), "0".to_string())])
        );
    }

    #[tokio::test]
    async fn present_header_overrides_static_param() {
        let (mut producer, captures) = make_recording_producer(
            vec![("sequenceNumber".to_string(), "0".to_string())],
            param_set(&[("sequenceNumber", "seqHeader")]),
        );

        call_producer(
            &mut producer,
            exchange_with_header("seqHeader", Value::String("9".into())),
        )
        .await
        .expect("transform should succeed");

        let log = capture_snapshot(&captures);
        assert_eq!(
            log[0],
            HashMap::from([("sequenceNumber".to_string(), "9".to_string())])
        );
    }

    #[tokio::test]
    async fn absent_header_falls_back_to_static() {
        let (mut producer, captures) = make_recording_producer(
            vec![("sequenceNumber".to_string(), "0".to_string())],
            param_set(&[("sequenceNumber", "seqHeader")]),
        );

        call_producer(&mut producer, exchange_without_header())
            .await
            .expect("transform should succeed");

        let log = capture_snapshot(&captures);
        assert_eq!(
            log[0],
            HashMap::from([("sequenceNumber".to_string(), "0".to_string())])
        );
    }

    #[tokio::test]
    async fn number_and_bool_headers_convert() {
        let (mut producer, captures) =
            make_recording_producer(vec![], param_set(&[("n", "hn"), ("b", "hb")]));

        let mut exchange = Exchange::new(Message::new(Body::from("<a/>")));
        exchange
            .input
            .headers
            .insert("hn".to_string(), Value::from(7u64));
        exchange
            .input
            .headers
            .insert("hb".to_string(), Value::Bool(true));

        call_producer(&mut producer, exchange)
            .await
            .expect("transform should succeed");

        let log = capture_snapshot(&captures);
        assert_eq!(log[0].get("n").map(String::as_str), Some("7"));
        assert_eq!(log[0].get("b").map(String::as_str), Some("true"));
    }

    #[tokio::test]
    async fn empty_string_header_forwarded_verbatim() {
        let (mut producer, captures) =
            make_recording_producer(vec![], param_set(&[("sequenceNumber", "seqHeader")]));

        call_producer(
            &mut producer,
            exchange_with_header("seqHeader", Value::String(String::new())),
        )
        .await
        .expect("transform should succeed");

        let log = capture_snapshot(&captures);
        assert_eq!(log[0].get("sequenceNumber").map(String::as_str), Some(""));
    }

    #[tokio::test]
    async fn complex_header_fails_closed_without_payload() {
        let (mut producer, captures) = make_recording_producer(vec![], param_set(&[("p", "ph")]));

        let array: Value = "[\"sentinel\"]"
            .parse()
            .expect("literal must be valid JSON array");
        let err = call_producer(&mut producer, exchange_with_header("ph", array))
            .await
            .expect_err("array header must fail");
        let msg = err.to_string();
        assert!(msg.contains("'p'"), "error must name the param: {msg}");
        assert!(
            msg.contains("'ph'"),
            "error must name the header key: {msg}"
        );
        assert!(msg.contains("array"), "error must name the type: {msg}");
        assert!(
            !msg.contains("sentinel"),
            "error leaked header value: {msg}"
        );

        let object: Value = "{\"k\":\"sentinel\"}"
            .parse()
            .expect("literal must be valid JSON object");
        let err = call_producer(&mut producer, exchange_with_header("ph", object))
            .await
            .expect_err("object header must fail");
        let msg = err.to_string();
        assert!(msg.contains("'p'"), "error must name the param: {msg}");
        assert!(
            msg.contains("'ph'"),
            "error must name the header key: {msg}"
        );
        assert!(msg.contains("object"), "error must name the type: {msg}");
        assert!(
            !msg.contains("sentinel"),
            "error leaked header value: {msg}"
        );

        assert!(
            capture_snapshot(&captures).is_empty(),
            "no transform may run when param resolution fails"
        );
    }

    #[tokio::test]
    async fn no_param_leak_between_exchanges() {
        let (mut producer, captures) = make_recording_producer(vec![], param_set(&[("v", "hv")]));

        for value in ["1", "2"] {
            call_producer(
                &mut producer,
                exchange_with_header("hv", Value::String(value.to_string())),
            )
            .await
            .expect("transform should succeed");
        }
        call_producer(&mut producer, exchange_without_header())
            .await
            .expect("transform should succeed");

        let sequential = capture_snapshot(&captures);
        assert_eq!(sequential.len(), 3);
        assert_eq!(sequential[0].get("v").map(String::as_str), Some("1"));
        assert_eq!(sequential[1].get("v").map(String::as_str), Some("2"));
        assert_eq!(sequential[2].get("v"), None);
        assert!(sequential[2].is_empty());

        let mut first = producer.clone();
        let mut second = producer.clone();
        let (first_result, second_result) = tokio::join!(
            call_producer(
                &mut first,
                exchange_with_header("hv", Value::String("1".into())),
            ),
            call_producer(
                &mut second,
                exchange_with_header("hv", Value::String("2".into())),
            ),
        );
        first_result.expect("first concurrent transform should succeed");
        second_result.expect("second concurrent transform should succeed");

        let all = capture_snapshot(&captures);
        assert_eq!(all.len(), 5);
        assert_eq!(all[3], HashMap::from([("v".to_string(), "1".to_string())]));
        assert_eq!(all[4], HashMap::from([("v".to_string(), "2".to_string())]));
    }

    #[tokio::test]
    async fn static_only_unchanged() {
        let (mut producer, captures) =
            make_recording_producer(vec![("mode".to_string(), "debug".to_string())], vec![]);

        call_producer(&mut producer, exchange_without_header())
            .await
            .expect("transform should succeed");

        let log = capture_snapshot(&captures);
        assert_eq!(
            log[0],
            HashMap::from([("mode".to_string(), "debug".to_string())])
        );
    }

    #[tokio::test]
    async fn param_expr_value_reaches_transform() {
        let (mut producer, captures) = make_recording_producer_with_expr(
            vec![],
            param_expr_set(vec![(
                "sequenceNumber",
                TestOutput::Emit(Value::String("42".into())),
            )]),
            vec![],
        );

        call_producer(&mut producer, exchange_without_header())
            .await
            .expect("transform should succeed");

        let log = capture_snapshot(&captures);
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].get("sequenceNumber").map(String::as_str), Some("42"));
    }

    #[tokio::test]
    async fn param_expr_overrides_static_param() {
        let (mut producer, captures) = make_recording_producer_with_expr(
            vec![("sequenceNumber".to_string(), "static".to_string())],
            param_expr_set(vec![(
                "sequenceNumber",
                TestOutput::Emit(Value::String("42".into())),
            )]),
            vec![],
        );

        call_producer(&mut producer, exchange_without_header())
            .await
            .expect("transform should succeed");

        let log = capture_snapshot(&captures);
        assert_eq!(
            log[0],
            HashMap::from([("sequenceNumber".to_string(), "42".to_string())])
        );
    }

    #[tokio::test]
    async fn header_overrides_param_expr() {
        let (mut producer, captures) = make_recording_producer_with_expr(
            vec![("sequenceNumber".to_string(), "static".to_string())],
            param_expr_set(vec![(
                "sequenceNumber",
                TestOutput::Emit(Value::String("42".into())),
            )]),
            param_set(&[("sequenceNumber", "seqHeader")]),
        );

        call_producer(
            &mut producer,
            exchange_with_header("seqHeader", Value::String("9".into())),
        )
        .await
        .expect("transform should succeed");

        let log = capture_snapshot(&captures);
        assert_eq!(
            log[0],
            HashMap::from([("sequenceNumber".to_string(), "9".to_string())])
        );
    }

    #[tokio::test]
    async fn null_param_expr_falls_back_to_static() {
        let (mut producer, captures) = make_recording_producer_with_expr(
            vec![("sequenceNumber".to_string(), "static".to_string())],
            param_expr_set(vec![("sequenceNumber", TestOutput::Emit(Value::Null))]),
            vec![],
        );

        call_producer(&mut producer, exchange_without_header())
            .await
            .expect("transform should succeed");

        let log = capture_snapshot(&captures);
        assert_eq!(
            log[0],
            HashMap::from([("sequenceNumber".to_string(), "static".to_string())])
        );
    }

    #[tokio::test]
    async fn null_header_falls_back_to_param_expr() {
        let (mut producer, captures) = make_recording_producer_with_expr(
            vec![],
            param_expr_set(vec![(
                "sequenceNumber",
                TestOutput::Emit(Value::String("42".into())),
            )]),
            param_set(&[("sequenceNumber", "seqHeader")]),
        );

        call_producer(
            &mut producer,
            exchange_with_header("seqHeader", Value::Null),
        )
        .await
        .expect("transform should succeed");

        let log = capture_snapshot(&captures);
        assert_eq!(
            log[0],
            HashMap::from([("sequenceNumber".to_string(), "42".to_string())])
        );
    }

    #[tokio::test]
    async fn absent_header_still_uses_param_expr() {
        let (mut producer, captures) = make_recording_producer_with_expr(
            vec![],
            param_expr_set(vec![(
                "sequenceNumber",
                TestOutput::Emit(Value::String("42".into())),
            )]),
            param_set(&[("sequenceNumber", "seqHeader")]),
        );

        call_producer(&mut producer, exchange_without_header())
            .await
            .expect("transform should succeed");

        let log = capture_snapshot(&captures);
        assert_eq!(
            log[0],
            HashMap::from([("sequenceNumber".to_string(), "42".to_string())])
        );
    }

    #[tokio::test]
    async fn number_and_bool_param_expr_convert() {
        let (mut producer, captures) = make_recording_producer_with_expr(
            vec![],
            param_expr_set(vec![
                ("n", TestOutput::Emit(Value::from(7u64))),
                ("b", TestOutput::Emit(Value::Bool(true))),
            ]),
            vec![],
        );

        call_producer(&mut producer, exchange_without_header())
            .await
            .expect("transform should succeed");

        let log = capture_snapshot(&captures);
        assert_eq!(log[0].get("n").map(String::as_str), Some("7"));
        assert_eq!(log[0].get("b").map(String::as_str), Some("true"));
    }

    #[tokio::test]
    async fn complex_param_expr_fails_closed_without_payload() {
        let array: Value = "[\"sentinel\"]"
            .parse()
            .expect("literal must be valid JSON array");
        let (mut producer, captures) = make_recording_producer_with_expr(
            vec![],
            param_expr_set(vec![("p", TestOutput::Emit(array))]),
            vec![],
        );

        let err = call_producer(&mut producer, exchange_without_header())
            .await
            .expect_err("array expression result must fail");
        let msg = err.to_string();
        assert!(msg.contains("'p'"), "error must name the param: {msg}");
        assert!(msg.contains("array"), "error must name the type: {msg}");
        assert!(
            !msg.contains("sentinel"),
            "error leaked expression value: {msg}"
        );

        let object: Value = "{\"k\":\"sentinel\"}"
            .parse()
            .expect("literal must be valid JSON object");
        let (mut producer, captures_object) = make_recording_producer_with_expr(
            vec![],
            param_expr_set(vec![("p", TestOutput::Emit(object))]),
            vec![],
        );

        let err = call_producer(&mut producer, exchange_without_header())
            .await
            .expect_err("object expression result must fail");
        let msg = err.to_string();
        assert!(msg.contains("'p'"), "error must name the param: {msg}");
        assert!(msg.contains("object"), "error must name the type: {msg}");
        assert!(
            !msg.contains("sentinel"),
            "error leaked expression value: {msg}"
        );

        assert!(
            capture_snapshot(&captures).is_empty(),
            "no transform may run when param resolution fails"
        );
        assert!(
            capture_snapshot(&captures_object).is_empty(),
            "no transform may run when param resolution fails"
        );
    }

    #[tokio::test]
    async fn param_expr_evaluation_error_propagates() {
        let (mut producer, captures) = make_recording_producer_with_expr(
            vec![],
            param_expr_set(vec![("sequenceNumber", TestOutput::Fail)]),
            vec![],
        );

        let err = call_producer(&mut producer, exchange_without_header())
            .await
            .expect_err("evaluation failure must abort the call");
        let msg = err.to_string();
        assert!(
            msg.contains("'sequenceNumber'"),
            "error must name the param: {msg}"
        );
        assert!(
            msg.contains("details withheld"),
            "error must use a redacted detail marker: {msg}"
        );
        assert!(
            !msg.contains("SECRET-SENTINEL"),
            "error leaked free-form language payload: {msg}"
        );
        assert!(
            !msg.contains("scripted evaluation failure"),
            "error leaked raw language display text: {msg}"
        );

        assert!(
            capture_snapshot(&captures).is_empty(),
            "no transform may run when evaluation fails"
        );
    }

    #[tokio::test]
    async fn param_expr_eval_failure_surfaces_redacted_detail() {
        let (mut producer, captures) = make_recording_producer_with_expr(
            vec![],
            param_expr_set(vec![("p", TestOutput::RedactedFail)]),
            vec![],
        );

        let err = call_producer(&mut producer, exchange_without_header())
            .await
            .expect_err("evaluation failure must abort the call");
        let msg = err.to_string();
        assert!(msg.contains("'p'"), "error must name the param: {msg}");
        assert!(
            msg.contains("redacted-ok"),
            "contractually redacted detail must be surfaced: {msg}"
        );
        assert!(
            msg.contains("[runtime]"),
            "typed class must be surfaced: {msg}"
        );

        assert!(
            capture_snapshot(&captures).is_empty(),
            "no transform may run when evaluation fails"
        );
    }

    #[tokio::test]
    async fn concurrent_exchanges_keep_distinct_param_values() {
        // Both producer clones share the same expression `Arc`, so they share
        // this barrier. Each evaluation parks inside `evaluate` until the other
        // arrives, forcing genuine overlap at the suspension point.
        let barrier = Arc::new(tokio::sync::Barrier::new(2));
        let (producer, captures) = make_recording_producer_with_expr(
            vec![],
            param_expr_set(vec![(
                "v",
                TestOutput::BarrierEchoHeader {
                    key: "exprIn",
                    barrier: Arc::clone(&barrier),
                },
            )]),
            vec![],
        );

        let mut first = producer.clone();
        let mut second = producer.clone();
        let (first_result, second_result) =
            tokio::time::timeout(std::time::Duration::from_secs(5), async {
                tokio::join!(
                    call_producer(
                        &mut first,
                        exchange_with_header("exprIn", Value::String("A".into())),
                    ),
                    call_producer(
                        &mut second,
                        exchange_with_header("exprIn", Value::String("B".into())),
                    ),
                )
            })
            .await
            .expect("concurrent evaluation must finish within 5s");
        first_result.expect("first concurrent transform should succeed");
        second_result.expect("second concurrent transform should succeed");

        let log = capture_snapshot(&captures);
        assert_eq!(log.len(), 2, "expected one transform per exchange: {log:?}");
        let values: Vec<&str> = log
            .iter()
            .filter_map(|params| params.get("v").map(String::as_str))
            .collect();
        assert_eq!(
            values.iter().filter(|value| **value == "A").count(),
            1,
            "exactly one call must carry A: {log:?}"
        );
        assert_eq!(
            values.iter().filter(|value| **value == "B").count(),
            1,
            "exactly one call must carry B: {log:?}"
        );
    }
}
