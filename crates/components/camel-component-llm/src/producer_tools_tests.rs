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

use super::producer_test_helpers::{collect_stream_frames, make_exchange};

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

#[tokio::test]
async fn tool_turn_with_text_sets_body_and_headers() {
    let provider = Arc::new(
        MockProvider::new("test", MockMode::Fixed("unused".into())).with_tool_calls_and_text(
            "The answer is 42.",
            vec![("call_1", "get_weather", r#"{"city":"London"}"#)],
        ),
    );
    let config = LlmEndpointConfig {
        operation: LlmOperation::Chat,
        stream: false,
        ..Default::default()
    };
    let producer = LlmProducer::new(config, provider, 32768, "test-route".into()).build();
    let mut exchange = make_exchange(Body::Text("prompt".into()));

    producer.handle_chat(&mut exchange).await.expect("chat ok");

    assert_eq!(exchange.input.body, Body::Text("The answer is 42.".into()));
    let calls: Vec<crate::EmittedToolCall> =
        serde_json::from_value(exchange.input.headers[CAMEL_LLM_TOOL_CALLS].clone())
            .expect("tool calls header");
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].id, "call_1");
    assert_eq!(exchange.input.headers[CAMEL_LLM_TEXT], "The answer is 42.");
}

#[tokio::test]
async fn duplicate_tool_call_ids_dedup_first_wins() {
    let provider = Arc::new(
        MockProvider::new("test", MockMode::Fixed("unused".into())).with_tool_calls_and_text(
            "Done.",
            vec![
                ("call_1", "get_weather", r#"{"city":"London"}"#),
                ("call_1", "get_weather", r#"{"city":"Paris"}"#),
            ],
        ),
    );
    let config = LlmEndpointConfig {
        operation: LlmOperation::Chat,
        stream: false,
        ..Default::default()
    };
    let producer = LlmProducer::new(config, provider, 32768, "test-route".into()).build();
    let mut exchange = make_exchange(Body::Text("prompt".into()));

    producer.handle_chat(&mut exchange).await.expect("chat ok");

    let calls: Vec<crate::EmittedToolCall> =
        serde_json::from_value(exchange.input.headers[CAMEL_LLM_TOOL_CALLS].clone())
            .expect("tool calls header");
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].id, "call_1");
    assert_eq!(calls[0].arguments, r#"{"city":"London"}"#);
}

#[tokio::test]
async fn duplicate_tool_call_ids_verbatim_repeat_collapses() {
    let provider = Arc::new(
        MockProvider::new("test", MockMode::Fixed("unused".into())).with_tool_calls_and_text(
            "Done.",
            vec![
                ("call_1", "get_weather", r#"{"city":"London"}"#),
                ("call_1", "get_weather", r#"{"city":"London"}"#),
            ],
        ),
    );
    let config = LlmEndpointConfig {
        operation: LlmOperation::Chat,
        stream: false,
        ..Default::default()
    };
    let producer = LlmProducer::new(config, provider, 32768, "test-route".into()).build();
    let mut exchange = make_exchange(Body::Text("prompt".into()));

    producer.handle_chat(&mut exchange).await.expect("chat ok");

    let calls: Vec<crate::EmittedToolCall> =
        serde_json::from_value(exchange.input.headers[CAMEL_LLM_TOOL_CALLS].clone())
            .expect("tool calls header");
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].id, "call_1");
    assert_eq!(calls[0].arguments, r#"{"city":"London"}"#);
}

#[tokio::test]
async fn tool_turn_without_text_keeps_empty_body() {
    let provider = Arc::new(
        MockProvider::new("test", MockMode::Fixed("unused".into())).with_tool_call(
            "call_1",
            "get_weather",
            r#"{"city":"London"}"#,
        ),
    );
    let config = LlmEndpointConfig {
        operation: LlmOperation::Chat,
        stream: false,
        ..Default::default()
    };
    let producer = LlmProducer::new(config, provider, 32768, "test-route".into()).build();
    let mut exchange = make_exchange(Body::Text("prompt".into()));

    producer.handle_chat(&mut exchange).await.expect("chat ok");

    assert_eq!(exchange.input.body, Body::Empty);
    let calls: Vec<crate::EmittedToolCall> =
        serde_json::from_value(exchange.input.headers[CAMEL_LLM_TOOL_CALLS].clone())
            .expect("tool calls header");
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].id, "call_1");
    assert!(!exchange.input.headers.contains_key(CAMEL_LLM_TEXT));
}

#[tokio::test]
async fn text_only_turn_sets_body() {
    let provider = Arc::new(MockProvider::new(
        "test",
        MockMode::Fixed("plain answer".into()),
    ));
    let config = LlmEndpointConfig {
        operation: LlmOperation::Chat,
        stream: false,
        ..Default::default()
    };
    let producer = LlmProducer::new(config, provider, 32768, "test-route".into()).build();
    let mut exchange = make_exchange(Body::Text("prompt".into()));

    producer.handle_chat(&mut exchange).await.expect("chat ok");

    assert_eq!(exchange.input.body, Body::Text("plain answer".into()));
    assert!(!exchange.input.headers.contains_key(CAMEL_LLM_TOOL_CALLS));
}

// -----------------------------------------------------------------------
// Streaming tool-call dedup (first-wins on id, mirrors materialized)
// -----------------------------------------------------------------------

/// Extract the `type == "tool_call"` JSON frames from collected stream
/// chunks, preserving arrival order.
fn tool_call_frames(frames: &[String]) -> Vec<Value> {
    frames
        .iter()
        .filter_map(|f| serde_json::from_str::<Value>(f).ok())
        .filter(|v| v["type"] == "tool_call")
        .collect()
}

#[tokio::test]
async fn streaming_duplicate_tool_call_ids_forwarded_once() {
    let provider = Arc::new(
        MockProvider::new("test", MockMode::Fixed("unused".into())).with_tool_calls_and_text(
            "Done.",
            vec![
                ("call_1", "get_weather", r#"{"city":"London"}"#),
                ("call_1", "get_weather", r#"{"city":"London"}"#),
            ],
        ),
    );
    let config = LlmEndpointConfig {
        operation: LlmOperation::Chat,
        stream: true,
        ..Default::default()
    };
    let producer = LlmProducer::new(config, provider, 32768, "test-route".into()).build();
    let mut exchange = make_exchange(Body::Text("prompt".into()));

    producer.handle_chat(&mut exchange).await.expect("chat ok");

    let frames = collect_stream_frames(&mut exchange).await;
    let calls = tool_call_frames(&frames);
    assert_eq!(
        calls.len(),
        1,
        "duplicate tool-call id must be forwarded exactly once; frames: {frames:?}"
    );
    assert_eq!(calls[0]["id"], "call_1");
    assert_eq!(calls[0]["arguments"], r#"{"city":"London"}"#);
}

#[tokio::test]
async fn streaming_conflicting_dup_payload_first_wins() {
    let provider = Arc::new(
        MockProvider::new("test", MockMode::Fixed("unused".into())).with_tool_calls_and_text(
            "Done.",
            vec![
                ("call_1", "get_weather", r#"{"city":"London"}"#),
                ("call_1", "get_weather", r#"{"city":"Paris"}"#),
            ],
        ),
    );
    let config = LlmEndpointConfig {
        operation: LlmOperation::Chat,
        stream: true,
        ..Default::default()
    };
    let producer = LlmProducer::new(config, provider, 32768, "test-route".into()).build();
    let mut exchange = make_exchange(Body::Text("prompt".into()));

    producer.handle_chat(&mut exchange).await.expect("chat ok");

    let frames = collect_stream_frames(&mut exchange).await;
    let calls = tool_call_frames(&frames);
    assert_eq!(
        calls.len(),
        1,
        "conflicting duplicate must be dropped; frames: {frames:?}"
    );
    assert_eq!(calls[0]["id"], "call_1");
    assert_eq!(
        calls[0]["arguments"], r#"{"city":"London"}"#,
        "first occurrence must win over conflicting repeat"
    );
}

#[tokio::test]
async fn streaming_distinct_ids_all_forwarded_in_order() {
    let provider = Arc::new(
        MockProvider::new("test", MockMode::Fixed("unused".into())).with_tool_calls_and_text(
            "Done.",
            vec![
                ("call_1", "get_weather", r#"{"city":"London"}"#),
                ("call_2", "get_time", r#"{"zone":"UTC"}"#),
                ("call_1", "get_weather", r#"{"city":"London"}"#),
            ],
        ),
    );
    let config = LlmEndpointConfig {
        operation: LlmOperation::Chat,
        stream: true,
        ..Default::default()
    };
    let producer = LlmProducer::new(config, provider, 32768, "test-route".into()).build();
    let mut exchange = make_exchange(Body::Text("prompt".into()));

    producer.handle_chat(&mut exchange).await.expect("chat ok");

    let frames = collect_stream_frames(&mut exchange).await;
    let calls = tool_call_frames(&frames);
    let ids: Vec<&str> = calls
        .iter()
        .map(|c| c["id"].as_str().expect("tool_call id is a string"))
        .collect();
    assert_eq!(
        ids,
        vec!["call_1", "call_2"],
        "distinct ids forwarded in arrival order, trailing duplicate dropped"
    );
    // Mock emits exactly: 1 text delta + 3 tool-call events + 1 empty
    // Finished terminator. After dedup: 1 text + 2 tool calls + 1
    // terminator = 4 frames — no phantom empty frame from the dedup.
    assert_eq!(frames.len(), 4, "no phantom frame; frames: {frames:?}");
}

#[tokio::test]
async fn streaming_dedup_matches_materialized() {
    // Conflicting duplicate (London then Paris): first-wins in BOTH modes.
    let script = vec![
        ("call_1", "get_weather", r#"{"city":"London"}"#),
        ("call_1", "get_weather", r#"{"city":"Paris"}"#),
    ];

    // Materialized: header carries exactly one deduped tool call.
    let provider = Arc::new(
        MockProvider::new("test", MockMode::Fixed("unused".into()))
            .with_tool_calls_and_text("Done.", script.clone()),
    );
    let config = LlmEndpointConfig {
        operation: LlmOperation::Chat,
        stream: false,
        ..Default::default()
    };
    let producer = LlmProducer::new(config, provider, 32768, "test-route".into()).build();
    let mut exchange = make_exchange(Body::Text("prompt".into()));
    producer.handle_chat(&mut exchange).await.expect("chat ok");
    let materialized: Vec<crate::EmittedToolCall> =
        serde_json::from_value(exchange.input.headers[CAMEL_LLM_TOOL_CALLS].clone())
            .expect("tool calls header");

    // Streaming: body frames carry exactly one deduped tool call.
    let provider = Arc::new(
        MockProvider::new("test", MockMode::Fixed("unused".into()))
            .with_tool_calls_and_text("Done.", script.clone()),
    );
    let config = LlmEndpointConfig {
        operation: LlmOperation::Chat,
        stream: true,
        ..Default::default()
    };
    let producer = LlmProducer::new(config, provider, 32768, "test-route".into()).build();
    let mut exchange = make_exchange(Body::Text("prompt".into()));
    producer.handle_chat(&mut exchange).await.expect("chat ok");
    let frames = collect_stream_frames(&mut exchange).await;
    let calls = tool_call_frames(&frames);

    assert_eq!(materialized.len(), 1, "materialized dedup keeps first");
    assert_eq!(calls.len(), 1, "streaming dedup must match materialized");
    assert_eq!(materialized[0].id, calls[0]["id"]);
    assert_eq!(materialized[0].arguments, calls[0]["arguments"]);
}
