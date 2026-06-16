// Tool calling + multi-turn tests: CamelLlmTools header parsing,
// CamelLlmMessages header parsing, tool-call emission in response.

use std::sync::Arc;

use camel_api::Body;
use serde_json::Value;

use crate::LlmEndpointConfig;
use crate::config::LlmOperation;
use crate::headers::*;
use crate::producer::LlmProducer;
use crate::provider::LlmProvider;
use crate::provider::mock::{MockMode, MockProvider};

use super::producer_test_helpers::make_exchange;

// -----------------------------------------------------------------------
// Multi-turn messages header test
// -----------------------------------------------------------------------

#[tokio::test]
async fn messages_header_parsed_into_request() {
    let provider = Arc::new(MockProvider::new("test", MockMode::Echo));
    let config = LlmEndpointConfig {
        operation: LlmOperation::Chat,
        stream: false,
        ..Default::default()
    };
    let producer = LlmProducer::new(config, provider, 32768, "test-route".into()).build();
    let mut exchange = make_exchange(Body::Text("latest prompt".into()));

    // Set a multi-turn messages header
    exchange.input.headers.insert(
        CAMEL_LLM_MESSAGES.to_string(),
        serde_json::json!([
            {
                "role": "User",
                "content": "what's the temperature?",
                "tool_calls": null,
            },
            {
                "role": "Assistant",
                "content": "",
                "tool_calls": [
                    {
                        "id": "call_1",
                        "name": "get_temperature",
                        "arguments": r#"{"city":"London"}"#,
                    }
                ],
            },
            {
                "role": {"Tool": {"tool_call_id": "call_1"}},
                "content": "22°C",
                "tool_calls": null,
            },
        ]),
    );

    producer.handle_chat(&mut exchange).await.expect("chat ok");
    // Body should be from the echo of multi-turn user messages.
    // Echo mode concatenates only User-role messages.
    match &exchange.input.body {
        Body::Text(s) => {
            assert!(s.contains("what's the temperature"), "text: {s}");
        }
        other => panic!("expected Text, got {other:?}"),
    }
}

// -----------------------------------------------------------------------
// Tool call parsing tests
// -----------------------------------------------------------------------

#[tokio::test]
async fn tools_header_is_parsed_into_request() {
    let mock = Arc::new(
        MockProvider::new("test", MockMode::Fixed("dummy".into())).with_tool_call(
            "call_1",
            "get_weather",
            r#"{"city":"London"}"#,
        ),
    );
    let provider = mock.clone() as Arc<dyn LlmProvider>;
    let config = LlmEndpointConfig {
        operation: LlmOperation::Chat,
        stream: true,
        ..Default::default()
    };
    let producer = LlmProducer::new(config, provider, 32768, "test-route".into()).build();
    let mut exchange = make_exchange(Body::Text("what's the weather?".into()));

    // Set tools header
    exchange.input.headers.insert(
        CAMEL_LLM_TOOLS.to_string(),
        serde_json::json!([
            {
                "name": "get_weather",
                "description": "Get weather for a city",
                "parameters": {}
            }
        ]),
    );

    producer.handle_chat(&mut exchange).await.expect("chat ok");

    // Consume the stream — should get a tool call JSON chunk
    let body = std::mem::replace(&mut exchange.input.body, Body::Empty);
    assert!(matches!(body, Body::Stream(_)), "expected stream body");
    let sb = match body {
        Body::Stream(sb) => sb,
        _ => unreachable!(),
    };
    use futures::StreamExt;
    let mut guard = sb.stream.lock().await;
    let stream = guard
        .as_mut()
        .expect("stream must be present after handle_chat");
    let chunk = stream.next().await.unwrap().expect("chunk ok");
    let text = String::from_utf8_lossy(&chunk);
    let parsed: serde_json::Value = serde_json::from_str(&text).expect("valid json chunk");
    assert_eq!(parsed["type"], "tool_call");
    assert_eq!(parsed["id"], "call_1");
    assert_eq!(parsed["name"], "get_weather");
    assert_eq!(parsed["arguments"], r#"{"city":"London"}"#);
}

#[tokio::test]
async fn malformed_tools_header_errors_before_provider_call() {
    let mock = Arc::new(MockProvider::new("test", MockMode::Fixed("dummy".into())));
    let provider = mock.clone() as Arc<dyn LlmProvider>;
    let config = LlmEndpointConfig {
        operation: LlmOperation::Chat,
        stream: false,
        ..Default::default()
    };
    let producer = LlmProducer::new(config, provider, 32768, "test-route".into()).build();
    let mut exchange = make_exchange(Body::Text("hello".into()));

    exchange.input.headers.insert(
        CAMEL_LLM_TOOLS.to_string(),
        Value::String("not valid json".into()),
    );

    let result = producer.handle_chat(&mut exchange).await;
    assert!(result.is_err(), "malformed tools header should error");
    assert_eq!(
        mock.call_count(),
        0,
        "provider must not be called when tools header is malformed"
    );
}

// -----------------------------------------------------------------------
// Empty messages header validation
// -----------------------------------------------------------------------

#[tokio::test]
async fn empty_messages_rejected() {
    let provider = Arc::new(MockProvider::echo());
    let config = LlmEndpointConfig {
        operation: LlmOperation::Chat,
        stream: false,
        ..Default::default()
    };
    let producer = LlmProducer::new(config, provider, 32768, "test-route".into()).build();
    let mut exchange = make_exchange(Body::Text("irrelevant".into()));

    // Set empty messages header
    exchange
        .input
        .headers
        .insert(CAMEL_LLM_MESSAGES.to_string(), serde_json::json!([]));

    let result = producer.handle_chat(&mut exchange).await;
    assert!(result.is_err(), "empty messages must be rejected");
    let err = result.unwrap_err();
    assert!(
        matches!(&err, crate::error::LlmError::InvalidRequest(msg) if msg.contains("non-empty")),
        "expected InvalidRequest about non-empty, got: {err}"
    );
}
