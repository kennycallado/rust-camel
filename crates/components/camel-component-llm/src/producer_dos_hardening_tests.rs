// DoS hardening tests: oversized JSON header rejection in `build_chat_request`.
// These tests guard against the L1 spec requirement: a misbehaving (or
// adversarial) upstream route must not be able to push a multi-megabyte
// CamelLlmMessages/Tools/ToolChoice header through the producer and force
// expensive serde_json deserialization. The producer rejects with
// `LlmError::InvalidRequest` *before* `serde_json::from_value` runs.

use std::sync::Arc;

use camel_api::Body;

use crate::LlmEndpointConfig;
use crate::config::LlmOperation;
use crate::error::LlmError;
use crate::headers::*;
use crate::producer::LlmProducer;
use crate::provider::mock::{MockMode, MockProvider};

use super::producer_test_helpers::make_exchange;

/// Build a JSON array of N messages, each with a `content` string of the
/// given byte length. Used to deterministically produce headers of a
/// specific serialized size for the DoS tests.
fn messages_header_of_size(count: usize, content_bytes: usize) -> serde_json::Value {
    let content: String = "x".repeat(content_bytes);
    let msgs: Vec<serde_json::Value> = (0..count)
        .map(|i| {
            serde_json::json!({
                "role": "User",
                "content": format!("{i}-{content}"),
                "tool_calls": null,
            })
        })
        .collect();
    serde_json::Value::Array(msgs)
}

/// Build a JSON array of N tool definitions with long names.
fn tools_header_of_size(count: usize, name_bytes: usize) -> serde_json::Value {
    let long_name: String = "t".repeat(name_bytes);
    let tools: Vec<serde_json::Value> = (0..count)
        .map(|i| {
            serde_json::json!({
                "name": format!("{i}_{long_name}"),
                "description": "noop",
                "parameters": {},
            })
        })
        .collect();
    serde_json::Value::Array(tools)
}

fn make_producer_with_max(max_header_json_bytes: usize) -> LlmProducer {
    let provider = Arc::new(MockProvider::new("test", MockMode::Echo));
    let config = LlmEndpointConfig {
        operation: LlmOperation::Chat,
        stream: false,
        ..Default::default()
    };
    LlmProducer::new(config, provider, 32768, "test-route".into())
        .with_max_header_json_bytes(max_header_json_bytes)
        .build()
}

#[test]
fn test_oversized_messages_header_rejected() {
    // Default `max_header_json_bytes` is 65_536. Build a CamelLlmMessages
    // header comfortably larger than that so the check trips regardless of
    // exact serialization drift.
    let producer = make_producer_with_max(crate::config::default_max_header_json_bytes());
    let mut exchange = make_exchange(Body::Text("prompt".into()));

    // 1000 messages * ~230 bytes each = ~230 KB total.
    let oversized = messages_header_of_size(1000, 200);
    let serialized_len = oversized.to_string().len();
    assert!(
        serialized_len > 65_536,
        "test setup must exceed default threshold (got {serialized_len})"
    );

    exchange
        .input
        .headers
        .insert(CAMEL_LLM_MESSAGES.to_string(), oversized);

    let err = producer
        .build_chat_request("prompt", &exchange)
        .expect_err("oversized messages header must be rejected");
    match &err {
        LlmError::InvalidRequest(msg) => {
            assert!(
                msg.contains("CamelLlmMessages"),
                "error must name the offending header: {msg}"
            );
            assert!(
                msg.contains("max_header_json_bytes"),
                "error must reference the threshold name: {msg}"
            );
        }
        other => panic!("expected InvalidRequest, got {other:?}"),
    }
}

#[test]
fn test_oversized_tools_header_rejected() {
    let producer = make_producer_with_max(crate::config::default_max_header_json_bytes());
    let mut exchange = make_exchange(Body::Text("prompt".into()));

    let oversized = tools_header_of_size(1000, 200);
    let serialized_len = oversized.to_string().len();
    assert!(
        serialized_len > 65_536,
        "test setup must exceed default threshold (got {serialized_len})"
    );

    exchange
        .input
        .headers
        .insert(CAMEL_LLM_TOOLS.to_string(), oversized);

    let err = producer
        .build_chat_request("prompt", &exchange)
        .expect_err("oversized tools header must be rejected");
    match &err {
        LlmError::InvalidRequest(msg) => {
            assert!(
                msg.contains("CamelLlmTools"),
                "error must name the offending header: {msg}"
            );
            assert!(
                msg.contains("max_header_json_bytes"),
                "error must reference the threshold name: {msg}"
            );
        }
        other => panic!("expected InvalidRequest, got {other:?}"),
    }
}

#[test]
fn test_normal_headers_accepted() {
    // 5 small messages, well under 1 KB serialized.
    let producer = make_producer_with_max(crate::config::default_max_header_json_bytes());
    let mut exchange = make_exchange(Body::Text("prompt".into()));

    let small = messages_header_of_size(5, 10);
    let serialized_len = small.to_string().len();
    assert!(
        serialized_len < 1024,
        "test setup must stay under 1 KB (got {serialized_len})"
    );

    exchange
        .input
        .headers
        .insert(CAMEL_LLM_MESSAGES.to_string(), small);

    let req = producer
        .build_chat_request("prompt", &exchange)
        .expect("small messages header must be accepted");
    assert_eq!(req.messages.len(), 5);
}

#[test]
fn test_custom_threshold_respected() {
    // Producer with a tight 1 KB threshold. A 2 KB messages header must be
    // rejected, even though it would be accepted under the default 64 KB.
    let producer = make_producer_with_max(1024);
    let mut exchange = make_exchange(Body::Text("prompt".into()));

    let big = messages_header_of_size(20, 100);
    let serialized_len = big.to_string().len();
    assert!(
        serialized_len > 1024,
        "test setup must exceed custom 1 KB threshold (got {serialized_len})"
    );

    exchange
        .input
        .headers
        .insert(CAMEL_LLM_MESSAGES.to_string(), big);

    let err = producer
        .build_chat_request("prompt", &exchange)
        .expect_err("2 KB header must be rejected with 1 KB threshold");
    match &err {
        LlmError::InvalidRequest(msg) => {
            assert!(
                msg.contains("max_header_json_bytes"),
                "error must reference the threshold name: {msg}"
            );
        }
        other => panic!("expected InvalidRequest, got {other:?}"),
    }
}

#[test]
fn test_oversized_tool_choice_header_rejected() {
    // 1 KB threshold. A 2 KB tool_choice object must be rejected.
    let producer = make_producer_with_max(1024);
    let mut exchange = make_exchange(Body::Text("prompt".into()));

    // Use a long Specific-tool name to inflate serialized size beyond 1 KB.
    let long_name: String = "n".repeat(2048);
    let tool_choice = serde_json::json!({ "Specific": long_name });
    let serialized_len = tool_choice.to_string().len();
    assert!(
        serialized_len > 1024,
        "test setup must exceed custom 1 KB threshold (got {serialized_len})"
    );

    exchange
        .input
        .headers
        .insert(CAMEL_LLM_TOOL_CHOICE.to_string(), tool_choice);

    let err = producer
        .build_chat_request("prompt", &exchange)
        .expect_err("oversized tool_choice header must be rejected");
    match &err {
        LlmError::InvalidRequest(msg) => {
            assert!(
                msg.contains("CamelLlmToolChoice"),
                "error must name the offending header: {msg}"
            );
            assert!(
                msg.contains("max_header_json_bytes"),
                "error must reference the threshold name: {msg}"
            );
        }
        other => panic!("expected InvalidRequest, got {other:?}"),
    }
}
