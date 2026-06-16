//! Multi-turn tool calling end-to-end test (F6).
//!
//! Exercises the full roundtrip:
//!   Turn 1: prompt + tools → mock emits ToolCall → CamelLlmToolCalls header
//!   Turn 2: CamelLlmMessages = [User, Assistant{tool_calls}, Tool] →
//!           mock verifies received message order → emits final response

use std::sync::{Arc, Mutex};

use camel_api::{Body, Exchange, Message};
use camel_component_llm::config::{LlmEndpointConfig, LlmOperation};
use camel_component_llm::headers::*;
use camel_component_llm::producer::LlmProducer;
use camel_component_llm::provider::mock::{MockMode, MockProvider};
use camel_component_llm::provider::{ChatMessage, ChatRole, EmittedToolCall};
use tower::Service;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_exchange(body: Body) -> Exchange {
    Exchange::new(Message::new(body))
}

fn make_producer(
    provider: Arc<dyn camel_component_llm::provider::LlmProvider>,
    stream: bool,
) -> LlmProducer {
    let config = LlmEndpointConfig {
        operation: LlmOperation::Chat,
        stream,
        ..Default::default()
    };
    LlmProducer::new(config, provider, 32768, "test-route".into()).build()
}

// ---------------------------------------------------------------------------
// Multi-turn tool calling roundtrip
// ---------------------------------------------------------------------------

#[tokio::test]
async fn multi_turn_tool_calling_roundtrip() {
    // =====================================================================
    // Turn 1: prompt + tools → provider emits ToolCall
    // =====================================================================
    let recorder1 = Arc::new(Mutex::new(Vec::new()));
    let provider1 = Arc::new(
        MockProvider::new("test", MockMode::Fixed(String::new()))
            .with_tool_call("call_1", "get_weather", r#"{"city":"London"}"#)
            .with_messages_recorder(Arc::clone(&recorder1)),
    );
    let mut producer1 = make_producer(provider1, /* stream */ false);

    let mut exchange1 = make_exchange(Body::Text("What's the weather in London?".into()));
    // Set tool definitions header
    exchange1.input.headers.insert(
        CAMEL_LLM_TOOLS.to_string(),
        serde_json::json!([
            {
                "name": "get_weather",
                "description": "Get weather for a city",
                "parameters": {}
            }
        ]),
    );

    let result1 = producer1
        .call(exchange1)
        .await
        .expect("turn 1 should succeed");

    // Assert CamelLlmToolCalls header is present with the expected tool call
    let tool_calls_val = result1
        .input
        .headers
        .get(CAMEL_LLM_TOOL_CALLS)
        .expect("CamelLlmToolCalls header must be set after tool call");
    let tool_calls: Vec<EmittedToolCall> =
        serde_json::from_value(tool_calls_val.clone()).expect("valid EmittedToolCall array");
    assert_eq!(tool_calls.len(), 1, "expected exactly one tool call");
    assert_eq!(tool_calls[0].id, "call_1");
    assert_eq!(tool_calls[0].name, "get_weather");
    assert_eq!(tool_calls[0].arguments, r#"{"city":"London"}"#);

    // Body must be Empty when tool calls are present
    assert!(
        matches!(result1.input.body, Body::Empty),
        "body must be Empty when tool calls are emitted"
    );

    // Verify the provider received the user message (scoped to drop guard before .await)
    {
        let received1 = recorder1.lock().expect("recorder1 poisoned");
        assert_eq!(
            received1.len(),
            1,
            "provider should receive 1 message in turn 1"
        );
        assert!(matches!(received1[0].role, ChatRole::User));
        assert_eq!(received1[0].content, "What's the weather in London?");
    }

    // =====================================================================
    // Turn 2: multi-turn conversation with tool result
    // =====================================================================
    let recorder2 = Arc::new(Mutex::new(Vec::new()));
    let provider2 = Arc::new(
        MockProvider::new(
            "test",
            MockMode::Fixed("The temperature in London is 22°C.".into()),
        )
        .with_messages_recorder(Arc::clone(&recorder2)),
    );
    let mut producer2 = make_producer(provider2, /* stream */ false);

    let mut exchange2 = make_exchange(Body::Text("irrelevant — messages come from header".into()));
    // Build turn-2 messages FROM turn-1's tool_calls output (real contract test)
    let messages = vec![
        ChatMessage {
            role: ChatRole::User,
            content: "What's the weather in London?".into(),
            tool_calls: None,
        },
        ChatMessage {
            role: ChatRole::Assistant,
            content: String::new(),
            tool_calls: Some(tool_calls.clone()),
        },
        ChatMessage {
            role: ChatRole::Tool {
                tool_call_id: tool_calls[0].id.clone(),
            },
            content: r#"{"temperature": 22, "unit": "C"}"#.into(),
            tool_calls: None,
        },
    ];
    let messages_header = serde_json::to_value(&messages).expect("messages serialize");
    exchange2
        .input
        .headers
        .insert(CAMEL_LLM_MESSAGES.to_string(), messages_header);

    let result2 = producer2
        .call(exchange2)
        .await
        .expect("turn 2 should succeed");

    // Verify the provider received the correct message order (scoped to drop guard)
    {
        let received2 = recorder2.lock().expect("recorder2 poisoned");
        assert_eq!(
            received2.len(),
            3,
            "provider should receive 3 messages in turn 2"
        );

        // Message 0: User
        assert!(
            matches!(received2[0].role, ChatRole::User),
            "msg[0] role should be User"
        );
        assert_eq!(received2[0].content, "What's the weather in London?");

        // Message 1: Assistant with tool_calls
        assert!(
            matches!(received2[1].role, ChatRole::Assistant),
            "msg[1] role should be Assistant"
        );
        assert!(
            received2[1].tool_calls.is_some(),
            "msg[1] should have tool_calls"
        );
        let assistant_tool_calls = received2[1].tool_calls.as_ref().unwrap();
        assert_eq!(assistant_tool_calls.len(), 1);
        assert_eq!(assistant_tool_calls[0].id, "call_1");
        assert_eq!(assistant_tool_calls[0].name, "get_weather");
        assert_eq!(
            assistant_tool_calls[0].arguments, r#"{"city":"London"}"#,
            "arguments must survive serde round-trip"
        );

        // Message 2: Tool result
        assert!(
            matches!(received2[2].role, ChatRole::Tool { .. }),
            "msg[2] role should be Tool"
        );
        if let ChatRole::Tool { tool_call_id } = &received2[2].role {
            assert_eq!(tool_call_id, "call_1", "tool_call_id should match");
        }
        assert_eq!(received2[2].content, r#"{"temperature": 22, "unit": "C"}"#);
    }

    // Verify the final response contains the expected text
    match &result2.input.body {
        Body::Text(s) => {
            assert!(
                s.contains("22°C"),
                "response should contain temperature: {s}"
            );
        }
        other => panic!("expected Text body, got {other:?}"),
    }
}
