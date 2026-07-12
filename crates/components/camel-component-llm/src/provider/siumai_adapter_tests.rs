use std::time::Duration;

use super::*;
use futures::future::join_all;
use siumai_core::error::LlmError as SiumaiLlmError;
use siumai_core::streaming::ChatStream;
use siumai_core::types::{ChatMessage as SiumaiChatMessage, ChatResponse, EmbeddingResponse, Tool};

/// Stub chat capability — never called by `embed()` but must be constructable
/// so it can be placed in `CachedClient` inside the build override.
struct StubChat;

#[async_trait::async_trait]
impl ChatCapability for StubChat {
    async fn chat_with_tools(
        &self,
        _messages: Vec<SiumaiChatMessage>,
        _tools: Option<Vec<Tool>>,
    ) -> Result<ChatResponse, SiumaiLlmError> {
        unreachable!("StubChat should not be called in OnceCell dedup test")
    }

    async fn chat_stream(
        &self,
        _messages: Vec<SiumaiChatMessage>,
        _tools: Option<Vec<Tool>>,
    ) -> Result<ChatStream, SiumaiLlmError> {
        unreachable!("StubChat should not be called in OnceCell dedup test")
    }
}

/// Stub embedding capability — returns a valid embedding response so the
/// production `embed()` method can complete successfully.
struct StubEmbed;

#[async_trait::async_trait]
impl EmbeddingCapability for StubEmbed {
    async fn embed(&self, _input: Vec<String>) -> Result<EmbeddingResponse, SiumaiLlmError> {
        Ok(EmbeddingResponse::new(
            vec![vec![1.0, 2.0, 3.0]],
            "stub-model".into(),
        ))
    }

    fn as_embedding_extensions(&self) -> Option<&dyn EmbeddingExtensions> {
        Some(self)
    }

    fn embedding_dimension(&self) -> usize {
        3
    }
}

#[async_trait::async_trait]
impl EmbeddingExtensions for StubEmbed {}

// ========================================================================
// Tool mapping tests
// ========================================================================

#[test]
fn map_tool_definition_function_name_and_description() {
    let params = serde_json::json!({
        "type": "object",
        "properties": {
            "location": { "type": "string" }
        }
    });
    let def = ToolDefinition {
        name: "get_weather".into(),
        description: "Get weather for a city".into(),
        parameters: params.as_object().unwrap().clone(),
    };
    let tool = map_tool_definition(def);
    let func = tool.function_ref().expect("expected function tool");
    assert_eq!(func.name, "get_weather");
    assert_eq!(func.description, "Get weather for a city");
    assert_eq!(
        func.parameters,
        serde_json::json!({
            "type": "object",
            "properties": {
                "location": { "type": "string" }
            }
        })
    );
}

#[test]
fn map_tool_choice_auto() {
    let siumai = map_tool_choice(ToolChoice::Auto);
    assert_eq!(siumai, SiumaiToolChoice::Auto);
}

#[test]
fn map_tool_choice_none() {
    let siumai = map_tool_choice(ToolChoice::None);
    assert_eq!(siumai, SiumaiToolChoice::None);
}

#[test]
fn map_tool_choice_specific() {
    let siumai = map_tool_choice(ToolChoice::Specific("get_weather".into()));
    assert_eq!(siumai, SiumaiToolChoice::tool("get_weather"));
}

#[test]
fn convert_stream_part_tool_call() {
    use siumai_core::types::ChatStreamPart;
    use siumai_core::types::ChatStreamToolCall;

    let part = ChatStreamPart::ToolCall(ChatStreamToolCall {
        tool_call_id: "call_123".into(),
        tool_name: "get_weather".into(),
        input: r#"{"city":"London"}"#.into(),
        provider_executed: None,
        dynamic: None,
        provider_metadata: None,
    });

    let event = convert_stream_part(part).expect("expected Some");
    if let ChatEvent::ToolCall {
        id,
        name,
        arguments,
    } = event
    {
        assert_eq!(id, "call_123");
        assert_eq!(name, "get_weather");
        assert_eq!(arguments, r#"{"city":"London"}"#);
    } else {
        panic!("expected ToolCall");
    }
}

#[test]
fn extract_tool_call_delta_accumulation() {
    use siumai_core::streaming::ChatStreamEvent;
    use siumai_core::types::ChatStreamPart;

    let mut buffers: HashMap<String, (String, String)> = HashMap::new();

    // ToolInputStart: initialises buffer, emits nothing
    let start = ChatStreamEvent::Part {
        part: ChatStreamPart::ToolInputStart {
            id: "call_1".into(),
            tool_name: "get_weather".into(),
            provider_metadata: None,
            provider_executed: None,
            dynamic: None,
            title: None,
        },
    };
    assert!(
        extract_tool_call_event(&start, &mut buffers).is_none(),
        "ToolInputStart should not emit ToolCall"
    );
    assert!(
        buffers.contains_key("call_1"),
        "buffer should be initialised"
    );

    // ToolInputDelta: accumulates args, emits nothing
    let delta1 = ChatStreamEvent::Part {
        part: ChatStreamPart::ToolInputDelta {
            id: "call_1".into(),
            delta: r#"{"city":"L"#.into(),
            provider_metadata: None,
        },
    };
    assert!(extract_tool_call_event(&delta1, &mut buffers).is_none());

    let delta2 = ChatStreamEvent::Part {
        part: ChatStreamPart::ToolInputDelta {
            id: "call_1".into(),
            delta: "ondon\"}".into(),
            provider_metadata: None,
        },
    };
    assert!(extract_tool_call_event(&delta2, &mut buffers).is_none());

    // ToolInputEnd: emits accumulated args
    let end = ChatStreamEvent::Part {
        part: ChatStreamPart::ToolInputEnd {
            id: "call_1".into(),
            provider_metadata: None,
        },
    };
    let result = extract_tool_call_event(&end, &mut buffers);
    let tc = result.expect("end should emit ToolCall");
    if let ChatEvent::ToolCall {
        id,
        name,
        arguments,
    } = tc
    {
        assert_eq!(id, "call_1");
        assert_eq!(name, "get_weather");
        assert_eq!(arguments, r#"{"city":"London"}"#);
    } else {
        panic!("expected ToolCall");
    }
    // Buffer entry should be removed
    assert!(!buffers.contains_key("call_1"));
}

#[test]
fn convert_chat_message_tool_role() {
    let msg = ChatMessage {
        role: ChatRole::Tool {
            tool_call_id: "call_99".into(),
        },
        content: "weather: sunny, 22°C".into(),
        tool_calls: None,
    };
    let siumai = convert_chat_message(msg);
    assert_eq!(siumai.role, siumai_core::types::MessageRole::Tool);
    // tool_result_text wraps the content as a ToolResultContentPart
    let results = siumai.tool_results();
    assert_eq!(results.len(), 1, "expected one tool result");
    let info = results[0].as_tool_result().expect("tool result info");
    assert_eq!(info.tool_call_id, "call_99");
    // Output should contain the text content
    let output_dbg = format!("{:#?}", info.output);
    assert!(
        output_dbg.contains("sunny"),
        "tool result output: {output_dbg}"
    );
    assert!(
        output_dbg.contains("22"),
        "tool result output: {output_dbg}"
    );
}

#[test]
fn convert_chat_message_assistant_forwards_tool_calls() {
    let msg = ChatMessage {
        role: ChatRole::Assistant,
        content: "Let me check the weather.".into(),
        tool_calls: Some(vec![EmittedToolCall {
            id: "call_42".into(),
            name: "get_weather".into(),
            arguments: r#"{"city":"London"}"#.into(),
        }]),
    };
    let siumai = convert_chat_message(msg);
    assert_eq!(siumai.role, siumai_core::types::MessageRole::Assistant);
    let tool_calls = siumai.tool_calls();
    assert_eq!(tool_calls.len(), 1, "expected one tool call");
    let info = tool_calls[0].as_tool_call().expect("tool call info");
    assert_eq!(info.tool_call_id, "call_42");
    assert_eq!(info.tool_name, "get_weather");
    assert_eq!(info.arguments, &serde_json::json!({"city":"London"}));
    // Text content should also be present
    assert_eq!(siumai.content_text(), Some("Let me check the weather."));
}

#[test]
fn convert_chat_message_assistant_no_tool_calls_stays_text() {
    let msg = ChatMessage {
        role: ChatRole::Assistant,
        content: "Just a plain response.".into(),
        tool_calls: None,
    };
    let siumai = convert_chat_message(msg);
    assert_eq!(siumai.role, siumai_core::types::MessageRole::Assistant);
    assert!(siumai.tool_calls().is_empty(), "no tool calls expected");
    assert_eq!(siumai.content_text(), Some("Just a plain response."));
}

#[test]
fn convert_chat_message_assistant_empty_tool_calls_stays_text() {
    let msg = ChatMessage {
        role: ChatRole::Assistant,
        content: "Empty array.".into(),
        tool_calls: Some(vec![]),
    };
    let siumai = convert_chat_message(msg);
    assert_eq!(siumai.role, siumai_core::types::MessageRole::Assistant);
    assert!(siumai.tool_calls().is_empty(), "no tool calls expected");
    assert_eq!(siumai.content_text(), Some("Empty array."));
}

#[test]
fn chat_request_with_tools_and_tool_choice() {
    let params = serde_json::json!({
        "type": "object",
        "properties": {
            "city": { "type": "string" }
        }
    });
    let tool = ToolDefinition {
        name: "get_weather".into(),
        description: "Get weather".into(),
        parameters: params.as_object().unwrap().clone(),
    };
    let req = ChatRequest {
        model: "gpt-4o".into(),
        messages: vec![ChatMessage::user("weather?")],
        temperature: None,
        max_tokens: None,
        stop: None,
        system_prompt: None,
        extra: serde_json::Map::new(),
        tools: vec![tool],
        tool_choice: Some(ToolChoice::Specific("get_weather".into())),
    };
    let siumai_req = convert_chat_request(req, "gpt-4o");
    let tools = siumai_req.tools.expect("tools should be Some");
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].function_ref().unwrap().name, "get_weather");
    let tc = siumai_req.tool_choice.expect("tool_choice should be Some");
    assert_eq!(tc, SiumaiToolChoice::tool("get_weather"));
}

/// Verifies that the production `embed()` method constructs the client
/// exactly once under 8-way concurrency.
///
/// Uses a build override to avoid network I/O while exercising the real
/// `build_cached_client` → `OnceCell::get_or_try_init` code path.
#[tokio::test]
async fn client_built_once_under_concurrency() {
    let stub_builder: BuildOverride = Arc::new(|| {
        Box::pin(async {
            Ok::<_, LlmError>((
                Arc::new(StubChat) as Arc<dyn ChatCapability>,
                Arc::new(StubEmbed) as Arc<dyn EmbeddingCapability>,
            ))
        })
    });

    let provider = Arc::new(SiumaiProvider::new_with_build_override(
        "test",
        "gpt-4",
        SiumaiConfig::OpenAi {
            api_key: "sk-test".into(),
            base_url: Some("http://127.0.0.1:1".into()),
            model: "gpt-4".into(),
        },
        Duration::from_secs(30),
        stub_builder,
        SsrfPolicy::PublicHttpsOnly,
    ));
    let builds = Arc::clone(&provider.client_builds);

    // 8 concurrent calls to the PRODUCTION `embed()` method — exercises
    // the real `OnceCell::get_or_try_init` path inside `build_cached_client`.
    let mut futs = vec![];
    for _ in 0..8 {
        let p = Arc::clone(&provider);
        futs.push(async move {
            let _ = p
                .embed(EmbedRequest::new("gpt-4", vec!["hello".into()]))
                .await;
        });
    }
    join_all(futs).await;
    assert_eq!(
        builds.load(Ordering::SeqCst),
        1,
        "OnceCell must construct the client exactly once across concurrent embed() calls"
    );
}

// ========================================================================
// SSRF validation tests (D-M10)
// ========================================================================

#[tokio::test]
async fn build_client_rejects_ssrf_link_local_base_url() {
    let config = SiumaiConfig::OpenAi {
        api_key: "sk-test".into(),
        base_url: Some("http://169.254.169.254/".into()),
        model: "gpt-4".into(),
    };
    let result = build_client_from_config(
        &config,
        Duration::from_secs(30),
        SsrfPolicy::PublicHttpsOnly,
    )
    .await;
    assert!(result.is_err(), "expected Err for SSRF-blocked base_url");
    let err = result.err().expect("is_err"); // allow-unwrap
    assert!(
        matches!(err, LlmError::InvalidRequest(_)),
        "expected InvalidRequest, got: {err}"
    );
    let msg = err.to_string();
    assert!(
        msg.contains("blocked"),
        "expected SSRF-blocked message, got: {msg}"
    );
}

// ========================================================================
// Ollama SSRF rejection test
// ========================================================================

#[cfg(any(feature = "ollama", feature = "all-providers"))]
#[tokio::test]
async fn build_client_rejects_ollama_ssrf_link_local() {
    let config = SiumaiConfig::Ollama {
        base_url: "http://169.254.169.254/".into(),
        model: "llama3".into(),
    };
    let result = build_client_from_config(
        &config,
        Duration::from_secs(30),
        SsrfPolicy::PublicHttpsOnly,
    )
    .await;
    assert!(result.is_err(), "expected Err for SSRF-blocked base_url");
    let err = result.err().expect("is_err"); // allow-unwrap
    assert!(
        matches!(err, LlmError::InvalidRequest(_)),
        "expected InvalidRequest, got: {err}"
    );
    let msg = err.to_string();
    assert!(
        msg.contains("blocked"),
        "expected SSRF-blocked message, got: {msg}"
    );
}

// ========================================================================
// Timeout error mapping tests (N4)
// ========================================================================

#[test]
fn timeout_maps_to_llm_error_timeout_not_network() {
    let err = map_siumai_error(
        SiumaiLlmError::TimeoutError("elapsed".into()),
        Duration::from_secs(10),
    );
    assert!(matches!(err, LlmError::Timeout(d) if d == Duration::from_secs(10)));
}

#[test]
fn other_errors_still_map_correctly() {
    // Verify that non-timeout errors still map correctly with the new param
    let err = map_siumai_error(
        SiumaiLlmError::ConnectionError("conn reset".into()),
        Duration::from_secs(30),
    );
    assert!(matches!(err, LlmError::Network(_)));
}
