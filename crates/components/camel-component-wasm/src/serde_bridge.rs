use bytes::Bytes;
use camel_api::{Body, CamelError, Exchange, ExchangePattern, Message, Value};
use std::collections::HashMap;

use crate::bindings::camel::plugin::types::{
    WasmBody, WasmError, WasmExchange, WasmMessage, WasmPattern,
};

use crate::bean_bindings::camel::plugin::types::{
    WasmBody as BeanWasmBody, WasmExchange as BeanWasmExchange, WasmMessage as BeanWasmMessage,
    WasmPattern as BeanWasmPattern,
};

use crate::security_policy_bindings::camel::plugin::types::{
    WasmBody as SpWasmBody, WasmExchange as SpWasmExchange, WasmMessage as SpWasmMessage,
    WasmPattern as SpWasmPattern,
};

impl From<WasmExchange> for BeanWasmExchange {
    fn from(v: WasmExchange) -> Self {
        BeanWasmExchange {
            input: v.input.into(),
            output: v.output.map(Into::into),
            properties: v.properties,
            pattern: v.pattern.into(),
            correlation_id: v.correlation_id,
            route_id: v.route_id,
            message_id: v.message_id,
        }
    }
}

impl From<BeanWasmExchange> for WasmExchange {
    fn from(v: BeanWasmExchange) -> Self {
        WasmExchange {
            input: v.input.into(),
            output: v.output.map(Into::into),
            properties: v.properties,
            pattern: v.pattern.into(),
            correlation_id: v.correlation_id,
            route_id: v.route_id,
            message_id: v.message_id,
        }
    }
}

impl From<WasmMessage> for BeanWasmMessage {
    fn from(v: WasmMessage) -> Self {
        BeanWasmMessage {
            headers: v.headers,
            body: v.body.into(),
        }
    }
}

impl From<BeanWasmMessage> for WasmMessage {
    fn from(v: BeanWasmMessage) -> Self {
        WasmMessage {
            headers: v.headers,
            body: v.body.into(),
        }
    }
}

/// Cross-binding `From` impl for structurally identical WasmBody types.
///
/// Maps each variant by discriminant name. The `Stream` arm is unreachable in
/// Phase 1 (guest→host streaming body support is Phase 2; `body_to_wasm`
/// rejects `Body::Stream`). This arm exists for exhaustiveness and triggers a
/// debug assertion if ever reached.
///
/// Both type names must be single identifiers imported at the call site
/// (e.g. `WasmBody`, `BeanWasmBody`, `SpWasmBody`).
macro_rules! impl_from_wasm_body {
    ($from:ident => $to:ident) => {
        impl From<$from> for $to {
            fn from(v: $from) -> Self {
                match v {
                    $from::Empty => $to::Empty,
                    $from::Text(s) => $to::Text(s),
                    $from::Bytes(b) => $to::Bytes(b),
                    $from::Json(s) => $to::Json(s),
                    $from::Xml(s) => $to::Xml(s),
                    $from::Stream(_) => {
                        debug_assert!(
                            false,
                            "WasmBody::Stream should not reach cross-binding conversion"
                        );
                        tracing::warn!("stream body unsupported in cross-binding — data dropped");
                        $to::Empty
                    }
                }
            }
        }
    };
}

impl_from_wasm_body!(WasmBody => BeanWasmBody);
impl_from_wasm_body!(BeanWasmBody => WasmBody);

impl From<WasmPattern> for BeanWasmPattern {
    fn from(v: WasmPattern) -> Self {
        match v {
            WasmPattern::InOnly => BeanWasmPattern::InOnly,
            WasmPattern::InOut => BeanWasmPattern::InOut,
        }
    }
}

impl From<BeanWasmPattern> for WasmPattern {
    fn from(v: BeanWasmPattern) -> Self {
        match v {
            BeanWasmPattern::InOnly => WasmPattern::InOnly,
            BeanWasmPattern::InOut => WasmPattern::InOut,
        }
    }
}

impl From<WasmExchange> for SpWasmExchange {
    fn from(v: WasmExchange) -> Self {
        SpWasmExchange {
            input: v.input.into(),
            output: v.output.map(Into::into),
            properties: v.properties,
            pattern: v.pattern.into(),
            correlation_id: v.correlation_id,
            route_id: v.route_id,
            message_id: v.message_id,
        }
    }
}

impl From<WasmMessage> for SpWasmMessage {
    fn from(v: WasmMessage) -> Self {
        SpWasmMessage {
            headers: v.headers,
            body: v.body.into(),
        }
    }
}

impl_from_wasm_body!(WasmBody => SpWasmBody);

impl From<WasmPattern> for SpWasmPattern {
    fn from(v: WasmPattern) -> Self {
        match v {
            WasmPattern::InOnly => SpWasmPattern::InOnly,
            WasmPattern::InOut => SpWasmPattern::InOut,
        }
    }
}

pub fn exchange_to_wasm(exchange: &Exchange) -> Result<WasmExchange, CamelError> {
    let input_body = body_to_wasm(&exchange.input.body)?;
    exchange_to_wasm_with_body(exchange, input_body)
}

/// Build a [`WasmExchange`] using a pre-assembled input [`WasmBody`].
///
/// Streaming-aware variant of [`exchange_to_wasm`]: callers that have already
/// built the input body inside `Store::run_concurrent` (e.g. via
/// [`crate::stream_bridge::assemble_stream_body`]) pass it here instead of
/// letting [`exchange_to_wasm`] call [`body_to_wasm`] internally —
/// `body_to_wasm` cannot ship a `Body::Stream` (the `stream<u8>` handle needs
/// the `&Accessor` only available under `run_concurrent`).
///
/// The input headers, output message (still routed through
/// [`message_to_wasm`]), properties, pattern, and correlation-id are taken
/// from `exchange` unchanged.
pub fn exchange_to_wasm_with_body(
    exchange: &Exchange,
    input_body: WasmBody,
) -> Result<WasmExchange, CamelError> {
    let input = WasmMessage {
        headers: values_to_kv_list(&exchange.input.headers),
        body: input_body,
    };
    let output = exchange.output.as_ref().map(message_to_wasm).transpose()?;
    let properties = values_to_kv_list(&exchange.properties);
    let pattern = match exchange.pattern {
        ExchangePattern::InOnly => WasmPattern::InOnly,
        ExchangePattern::InOut => WasmPattern::InOut,
    };
    Ok(WasmExchange {
        input,
        output,
        properties,
        pattern,
        correlation_id: exchange.correlation_id.clone(),
        route_id: None,
        message_id: None,
    })
}

pub fn wasm_to_exchange(wasm: WasmExchange, original: &mut Exchange) {
    original.input = wasm_to_message(wasm.input);
    original.output = wasm.output.map(wasm_to_message);
    let guest_props = kv_list_to_values(wasm.properties);
    for (key, value) in guest_props {
        original.properties.insert(key, value);
    }
    original.pattern = match wasm.pattern {
        WasmPattern::InOnly => ExchangePattern::InOnly,
        WasmPattern::InOut => ExchangePattern::InOut,
    };
}

pub fn camel_error_to_wasm_error(error: CamelError) -> WasmError {
    match error {
        CamelError::TypeConversionFailed(msg) => WasmError::TypeConversion(msg),
        CamelError::AlreadyConsumed => {
            WasmError::TypeConversion("Body stream has already been consumed".into())
        }
        CamelError::StreamLimitExceeded(limit) => {
            WasmError::TypeConversion(format!("Stream size exceeded limit: {limit}"))
        }
        CamelError::Io(msg) => WasmError::Io(msg),
        CamelError::ComponentNotFound(msg)
        | CamelError::EndpointCreationFailed(msg)
        | CamelError::ProcessorError(msg)
        | CamelError::InvalidUri(msg)
        | CamelError::RouteError(msg)
        | CamelError::DeadLetterChannelFailed(msg)
        | CamelError::CircuitOpen(msg)
        | CamelError::Config(msg) => WasmError::ProcessorError(msg),
        CamelError::HttpOperationFailed {
            method,
            url,
            status_code,
            status_text,
            response_body,
        } => WasmError::ProcessorError(format!(
            "HTTP {method} {url} failed: {status_code} {status_text}{}",
            response_body
                .map(|body| format!(" body={body}"))
                .unwrap_or_default()
        )),
        // Post-ADR-0024: Stop EIP no longer reaches WASM (pipeline translates
        // PipelineOutcome::Stopped → Ok(ex) before WASM producers run). The only
        // CamelError variants WASM might see from the framework are producer
        // shutdown signals — map ConsumerStopping explicitly; Stopped is legacy.
        CamelError::ConsumerStopping => WasmError::ProcessorError("Consumer stopping".into()),
        CamelError::ChannelClosed => WasmError::Io("Channel closed".into()),
        _ => WasmError::ProcessorError(error.to_string()),
    }
}

fn message_to_wasm(message: &Message) -> Result<WasmMessage, CamelError> {
    let headers = values_to_kv_list(&message.headers);
    let body = body_to_wasm(&message.body)?;
    Ok(WasmMessage { headers, body })
}

fn wasm_to_message(message: WasmMessage) -> Message {
    Message {
        headers: kv_list_to_values(message.headers),
        body: wasm_to_body(message.body),
    }
}

fn wasm_to_body(body: WasmBody) -> Body {
    match body {
        WasmBody::Empty => Body::Empty,
        WasmBody::Text(s) => Body::Text(s),
        WasmBody::Bytes(v) => Body::Bytes(Bytes::from(v)),
        WasmBody::Json(s) => serde_json::from_str::<Value>(&s)
            .map(Body::Json)
            .unwrap_or(Body::Text(s)),
        WasmBody::Xml(s) => Body::Xml(s),
        // Unreachable after producer unification: all plugin paths now go
        // through `process_streaming_exchange`, which extracts the stream via
        // `take_stream` before `wasm_to_exchange` is called. Kept as a
        // defensive guard (warn + Empty) for release safety — if somehow
        // reached, it drops instead of panics.
        WasmBody::Stream(_) => {
            tracing::warn!("stream body on undrained sync plugin-return path — data dropped");
            Body::Empty
        }
    }
}

fn body_to_wasm(body: &Body) -> Result<WasmBody, CamelError> {
    match body {
        Body::Empty => Ok(WasmBody::Empty),
        Body::Text(s) => Ok(WasmBody::Text(s.clone())),
        Body::Bytes(b) => Ok(WasmBody::Bytes(b.to_vec())),
        Body::Json(v) => Ok(WasmBody::Json(serde_json::to_string(v).unwrap_or_default())),
        Body::Xml(s) => Ok(WasmBody::Xml(s.clone())),
        // `Body::Stream` cannot be lowered here: the WASM `stream<u8>` handle
        // can only be created inside `Store::run_concurrent` (it needs the
        // `&Accessor`). Callers that need to ship a streaming body across the
        // WASM boundary must use [`crate::stream_bridge::assemble_stream_body`]
        // from within a `run_concurrent` closure.
        Body::Stream(_) => Err(CamelError::ProcessorError(
            "wasm: streaming body requires assemble_stream_body inside run_concurrent".into(),
        )),
    }
}

fn values_to_kv_list(values: &HashMap<String, Value>) -> Vec<(String, String)> {
    values
        .iter()
        .map(|(k, v)| {
            let value = match v {
                Value::String(s) => s.clone(),
                Value::Number(n) => n.to_string(),
                Value::Bool(b) => b.to_string(),
                Value::Object(_) | Value::Array(_) | Value::Null => {
                    serde_json::to_string(v).unwrap_or_default()
                }
            };
            (k.clone(), value)
        })
        .collect()
}

fn kv_list_to_values(values: Vec<(String, String)>) -> HashMap<String, Value> {
    values
        .into_iter()
        .map(|(k, v)| {
            let parsed = serde_json::from_str::<Value>(&v).unwrap_or(Value::String(v));
            (k, parsed)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_api::{Exchange, Message, StreamBody, StreamMetadata};
    use serde_json::json;
    use std::sync::Arc;
    use tokio::sync::Mutex;

    #[test]
    fn test_exchange_to_wasm_basic() {
        let mut msg = Message::new("hello");
        msg.set_header("content-type", Value::String("text/plain".into()));

        let mut exchange = Exchange::new(msg);
        exchange.set_property("enabled", Value::Bool(true));

        let wasm = exchange_to_wasm(&exchange).expect("exchange_to_wasm should succeed");

        assert!(matches!(wasm.input.body, WasmBody::Text(ref s) if s == "hello"));
        assert_eq!(
            wasm.input
                .headers
                .iter()
                .find(|(k, _)| k == "content-type")
                .map(|(_, v)| v.as_str()),
            Some("text/plain")
        );
        assert_eq!(
            wasm.properties
                .iter()
                .find(|(k, _)| k == "enabled")
                .map(|(_, v)| v.as_str()),
            Some("true")
        );

        let mut original = Exchange::new(Message::default());
        wasm_to_exchange(wasm, &mut original);
        assert_eq!(original.input.body.as_text(), Some("hello"));
        assert_eq!(original.properties.get("enabled"), Some(&Value::Bool(true)));
    }

    #[test]
    fn test_exchange_to_wasm_with_body_uses_passed_body() {
        // The pre-built body must be used verbatim — exchange_to_wasm_with_body
        // must NOT call body_to_wasm on the input (that path rejects streams).
        let mut msg = Message::new("will-be-ignored");
        msg.set_header("h", Value::String("v".into()));
        let exchange = Exchange::new(msg);

        let wasm = exchange_to_wasm_with_body(&exchange, WasmBody::Bytes(b"override".to_vec()))
            .expect("with_body should succeed");

        // Input body is the passed-in Bytes, not the original Text.
        assert!(matches!(wasm.input.body, WasmBody::Bytes(ref b) if b == b"override"));
        // Headers still flow through from the exchange.
        assert_eq!(
            wasm.input
                .headers
                .iter()
                .find(|(k, _)| k == "h")
                .map(|(_, v)| v.as_str()),
            Some("v")
        );
        // route_id / message_id unset (host-side conversion never sets them).
        assert!(wasm.route_id.is_none());
        assert!(wasm.message_id.is_none());
    }

    #[test]
    fn test_exchange_to_wasm_with_body_preserves_pattern_props_output() {
        let mut msg = Message::new("in");
        msg.set_header("ct", Value::String("text/plain".into()));
        let mut exchange = Exchange::new(msg);
        exchange.pattern = ExchangePattern::InOut;
        exchange.set_property("flag", Value::Bool(true));
        exchange.correlation_id = "corr-1".to_string();
        // Output message still routes through message_to_wasm (body_to_wasm).
        exchange.output = Some(Message::new("out-payload"));

        let wasm = exchange_to_wasm_with_body(&exchange, WasmBody::Empty)
            .expect("with_body should succeed");

        assert!(matches!(wasm.pattern, WasmPattern::InOut));
        assert_eq!(
            wasm.properties
                .iter()
                .find(|(k, _)| k == "flag")
                .map(|(_, v)| v.as_str()),
            Some("true")
        );
        assert_eq!(wasm.correlation_id, "corr-1");
        // Output converted via message_to_wasm — text body survives.
        let out = wasm.output.expect("output present");
        assert!(matches!(out.body, WasmBody::Text(ref s) if s == "out-payload"));
    }

    #[test]
    fn test_exchange_to_wasm_with_body_matches_exchange_to_wasm() {
        // exchange_to_wasm must delegate to exchange_to_wasm_with_body, so for a
        // non-stream body the two paths produce identical WasmExchange metadata.
        let mut msg = Message::new("x");
        msg.set_header("k", json!(1));
        let exchange = Exchange::new(msg);

        let direct = exchange_to_wasm(&exchange).expect("direct");
        let via_with_body =
            exchange_to_wasm_with_body(&exchange, body_to_wasm(&Body::Text("x".into())).unwrap())
                .expect("with_body");
        assert_eq!(direct.input.headers, via_with_body.input.headers);
        assert_eq!(direct.properties, via_with_body.properties);
        assert_eq!(direct.pattern, via_with_body.pattern);
    }

    #[test]
    fn test_body_mapping_all_variants() {
        let json_v = json!({"k":"v"});
        let variants = vec![
            Body::Empty,
            Body::Text("text".into()),
            Body::Bytes(Bytes::from_static(b"bytes")),
            Body::Json(json_v.clone()),
            Body::Xml("<x/>".into()),
        ];

        for body in variants {
            let wasm = body_to_wasm(&body).expect("non-stream body should convert");
            let back = wasm_to_body(wasm);
            match (body, back) {
                (Body::Empty, Body::Empty)
                | (Body::Text(_), Body::Text(_))
                | (Body::Bytes(_), Body::Bytes(_))
                | (Body::Json(_), Body::Json(_))
                | (Body::Xml(_), Body::Xml(_)) => {}
                _ => panic!("variant mapping mismatch"),
            }
        }
    }

    #[test]
    fn test_header_value_types() {
        let mut msg = Message::new("x");
        msg.set_header("s", Value::String("abc".into()));
        msg.set_header("n", json!(42));
        msg.set_header("b", Value::Bool(true));
        msg.set_header("o", json!({"k":"v"}));
        msg.set_header("a", json!([1, 2, 3]));

        let exchange = Exchange::new(msg);
        let wasm = exchange_to_wasm(&exchange).expect("exchange_to_wasm should succeed");

        let mut out = Exchange::new(Message::default());
        wasm_to_exchange(wasm, &mut out);

        assert_eq!(
            out.input.headers.get("s"),
            Some(&Value::String("abc".into()))
        );
        assert_eq!(out.input.headers.get("n"), Some(&json!(42)));
        assert_eq!(out.input.headers.get("b"), Some(&Value::Bool(true)));
        assert_eq!(out.input.headers.get("o"), Some(&json!({"k":"v"})));
        assert_eq!(out.input.headers.get("a"), Some(&json!([1, 2, 3])));
    }

    #[test]
    fn test_stream_body_returns_error() {
        // body_to_wasm cannot ship a stream (needs run_concurrent); it must
        // reject Body::Stream so callers route through
        // crate::stream_bridge::assemble_stream_body.
        let body = Body::Stream(StreamBody {
            stream: Arc::new(Mutex::new(None)),
            metadata: StreamMetadata::default(),
        });

        let err = body_to_wasm(&body).expect_err("stream must return error");
        assert!(err.to_string().contains("assemble_stream_body"));
    }

    #[test]
    fn test_wasm_to_exchange_preserves_correlation_id() {
        let exchange = Exchange::new(Message::new("in"));
        let original_id = exchange.correlation_id.clone();

        let mut wasm = exchange_to_wasm(&exchange).expect("exchange_to_wasm should succeed");
        wasm.correlation_id = "guest-changed".into();

        let mut target = exchange.clone();
        wasm_to_exchange(wasm, &mut target);

        assert_eq!(target.correlation_id, original_id);
    }

    #[test]
    fn test_wasm_to_exchange_merges_properties_guest_does_not_echo_host_props() {
        let mut exchange = Exchange::new(Message::new("in"));
        exchange.set_property("hasMore", Value::String("true".into()));
        exchange.set_property("keep", Value::Number(1.into()));

        let mut wasm = exchange_to_wasm(&exchange).expect("exchange_to_wasm should succeed");
        wasm.properties = vec![("new".into(), "2".into())];

        let mut target = exchange.clone();
        wasm_to_exchange(wasm, &mut target);

        assert_eq!(
            target.properties.get("hasMore"),
            Some(&Value::String("true".into())),
            "host property not touched by guest must survive"
        );
        assert_eq!(
            target.properties.get("keep"),
            Some(&Value::Number(1.into())),
            "host property not touched by guest must survive"
        );
        assert_eq!(
            target.properties.get("new"),
            Some(&Value::Number(2.into())),
            "guest property must be added"
        );
    }

    #[test]
    fn test_wasm_to_exchange_guest_overrides_host_property() {
        let mut exchange = Exchange::new(Message::new("in"));
        exchange.set_property("hasMore", Value::Bool(true));

        let mut wasm = exchange_to_wasm(&exchange).expect("exchange_to_wasm should succeed");
        wasm.properties = vec![("hasMore".into(), "false".into())];

        let mut target = exchange.clone();
        wasm_to_exchange(wasm, &mut target);

        assert_eq!(
            target.properties.get("hasMore"),
            Some(&Value::Bool(false)),
            "guest property must override host on conflict"
        );
        assert_eq!(target.properties.len(), 1);
    }

    #[test]
    fn test_wasm_to_exchange_preserves_non_body_fields() {
        // wasm_to_exchange overwrites input, output, properties, and pattern
        // but must NOT touch extensions, correlation_id, otel_context, or error.
        // This test verifies the clone-then-merge contract used in both
        // streaming and non-streaming branches of `WasmProducer::call`.
        use opentelemetry::Context;
        use std::any::Any;

        // Type-based marker for otel_context verification (opentelemetry's
        // Context stores values keyed by TypeId, not string keys).
        #[derive(Debug, PartialEq)]
        struct OtelMarker(&'static str);

        let mut exchange = Exchange::new(Message::new("payload"));
        exchange.correlation_id = "preserve-corr".into();
        exchange.extensions.insert(
            "my-ext".into(),
            Arc::new(42u32) as Arc<dyn Any + Send + Sync>,
        );
        exchange.error = Some(CamelError::ProcessorError("test-err".into()));
        exchange.otel_context = Context::new().with_value(OtelMarker("preserved"));

        // Simulate the clone-then-merge path both streaming and non-streaming
        // branches take: wasm_to_exchange receives the guest result and merges
        // it into the cloned output exchange.
        let mut out = exchange.clone();
        let wasm = exchange_to_wasm(&exchange).expect("exchange_to_wasm");
        wasm_to_exchange(wasm, &mut out);

        // Non-body fields must survive unchanged.
        assert_eq!(out.correlation_id, "preserve-corr");
        let ext = out
            .extensions
            .get("my-ext")
            .and_then(|a| a.downcast_ref::<u32>());
        assert_eq!(ext, Some(&42u32));
        let err_msg = out.error.as_ref().map(|e| e.to_string());
        assert!(
            err_msg.is_some() && err_msg.unwrap().contains("test-err"),
            "error should survive"
        );
        let otel_val: Option<&OtelMarker> = out.otel_context.get::<OtelMarker>();
        assert_eq!(otel_val, Some(&OtelMarker("preserved")));
    }

    #[test]
    fn test_error_mapping() {
        assert!(matches!(
            camel_error_to_wasm_error(CamelError::TypeConversionFailed("x".into())),
            WasmError::TypeConversion(_)
        ));
        assert!(matches!(
            camel_error_to_wasm_error(CamelError::Io("x".into())),
            WasmError::Io(_)
        ));
        assert!(matches!(
            camel_error_to_wasm_error(CamelError::ProcessorError("x".into())),
            WasmError::ProcessorError(_)
        ));
    }
}
