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

impl From<WasmExchange> for BeanWasmExchange {
    fn from(v: WasmExchange) -> Self {
        BeanWasmExchange {
            input: v.input.into(),
            output: v.output.map(Into::into),
            properties: v.properties,
            pattern: v.pattern.into(),
            correlation_id: v.correlation_id,
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

impl From<WasmBody> for BeanWasmBody {
    fn from(v: WasmBody) -> Self {
        match v {
            WasmBody::Empty => BeanWasmBody::Empty,
            WasmBody::Text(s) => BeanWasmBody::Text(s),
            WasmBody::Bytes(b) => BeanWasmBody::Bytes(b),
            WasmBody::Json(s) => BeanWasmBody::Json(s),
            WasmBody::Xml(s) => BeanWasmBody::Xml(s),
        }
    }
}

impl From<BeanWasmBody> for WasmBody {
    fn from(v: BeanWasmBody) -> Self {
        match v {
            BeanWasmBody::Empty => WasmBody::Empty,
            BeanWasmBody::Text(s) => WasmBody::Text(s),
            BeanWasmBody::Bytes(b) => WasmBody::Bytes(b),
            BeanWasmBody::Json(s) => WasmBody::Json(s),
            BeanWasmBody::Xml(s) => WasmBody::Xml(s),
        }
    }
}

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

pub fn exchange_to_wasm(exchange: &Exchange) -> Result<WasmExchange, CamelError> {
    let input = message_to_wasm(&exchange.input)?;
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
    })
}

pub fn wasm_to_exchange(wasm: WasmExchange, original: &mut Exchange) {
    original.input = wasm_to_message(wasm.input);
    original.output = wasm.output.map(wasm_to_message);
    original.properties = kv_list_to_values(wasm.properties);
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
        CamelError::Stopped => WasmError::ProcessorError("Exchange stopped by Stop EIP".into()),
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

fn body_to_wasm(body: &Body) -> Result<WasmBody, CamelError> {
    match body {
        Body::Empty => Ok(WasmBody::Empty),
        Body::Text(s) => Ok(WasmBody::Text(s.clone())),
        Body::Bytes(b) => Ok(WasmBody::Bytes(b.to_vec())),
        Body::Json(v) => Ok(WasmBody::Json(serde_json::to_string(v).unwrap_or_default())),
        Body::Xml(s) => Ok(WasmBody::Xml(s.clone())),
        Body::Stream(_) => Err(CamelError::ProcessorError(
            "wasm: streaming body not supported".into(),
        )),
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
    }
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
        let body = Body::Stream(StreamBody {
            stream: Arc::new(Mutex::new(None)),
            metadata: StreamMetadata::default(),
        });

        let err = body_to_wasm(&body).expect_err("stream must return error");
        assert!(err.to_string().contains("streaming body not supported"));
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
