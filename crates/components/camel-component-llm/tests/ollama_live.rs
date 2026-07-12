//! Live integration tests against a local Ollama instance.
//!
//! Requirements:
//!   - Ollama running at http://localhost:11434
//!   - Chat model: `qwen3.5:4b` (ollama pull qwen3.5:4b)
//!   - Embedding model: `embeddinggemma:latest` (ollama pull embeddinggemma)
//!
//! Run with:
//!   cargo test -p camel-component-llm --test ollama_live --features ollama -- --ignored
//!
//! Or individual test:
//!   cargo test -p camel-component-llm --test ollama_live --features ollama -- --ignored ollama_chat_materialized

use std::collections::HashMap;
use std::sync::Arc;

use camel_api::{Body, Exchange, Message};
use camel_component_api::Component;
use camel_component_llm::{
    LlmComponent, LlmGlobalConfig, LlmProviderConfig, config::OllamaProviderConfig, headers::*,
};
use tower::Service;

const OLLAMA_BASE_URL: &str = "http://localhost:11434";
const CHAT_MODEL: &str = "qwen3.5:4b";

fn make_ollama_component() -> LlmComponent {
    let mut providers = HashMap::new();
    providers.insert(
        "local".into(),
        LlmProviderConfig::Ollama(OllamaProviderConfig {
            base_url: OLLAMA_BASE_URL.into(),
            default_model: CHAT_MODEL.into(),
            timeout_secs: None,
            max_concurrency: None,
            network_retry: None,
            pricing: None,
            cache_ttl_secs: None,
            cache_max_entries: None,
        }),
    );
    LlmComponent::new(LlmGlobalConfig {
        providers,
        allow_internal: true,
        ..Default::default()
    })
    .expect("component")
}

fn make_producer_ctx() -> camel_component_api::ProducerContext {
    camel_component_api::ProducerContext::new().with_route_id("test-route")
}

fn make_rt() -> Arc<dyn camel_component_api::RuntimeObservability> {
    Arc::new(camel_component_api::NoopRuntimeObservability)
}

#[tokio::test]
#[ignore = "requires local Ollama with qwen3.5:4b"]
async fn ollama_chat_streaming() {
    let component = make_ollama_component();
    let endpoint = component
        .create_endpoint(
            "llm:chat?provider=local&stream=true",
            &camel_component_api::NoOpComponentContext,
        )
        .expect("endpoint");

    let mut producer = endpoint
        .create_producer(make_rt(), &make_producer_ctx())
        .expect("producer");

    let exchange = Exchange::new(Message::new(Body::Text("Say hello in one word.".into())));
    let result = producer.call(exchange).await.expect("producer ok");

    assert!(
        matches!(result.input.body, Body::Stream(_)),
        "expected Body::Stream"
    );

    if let Body::Stream(ref sb) = result.input.body {
        let mut guard = sb.stream.lock().await;
        if let Some(stream) = guard.take() {
            use futures::StreamExt;
            let mut stream = stream;
            let mut full_response = String::new();
            let mut had_error = false;
            while let Some(chunk) = stream.next().await {
                match chunk {
                    Ok(bytes) => full_response.push_str(&String::from_utf8_lossy(&bytes)),
                    Err(e) => {
                        eprintln!("Stream error: {e}");
                        had_error = true;
                    }
                }
            }
            assert!(!had_error, "stream had errors");
            assert!(!full_response.is_empty(), "stream produced no output");
            println!("Streaming response: {full_response}");
        } else {
            panic!("stream already consumed");
        }
    }

    assert_eq!(
        result
            .input
            .headers
            .get(CAMEL_LLM_PROVIDER)
            .and_then(|v| v.as_str()),
        Some("local")
    );
    assert_eq!(
        result
            .input
            .headers
            .get(CAMEL_LLM_STREAM)
            .and_then(|v| v.as_bool()),
        Some(true)
    );
}

#[tokio::test]
#[ignore = "requires local Ollama with qwen3.5:4b"]
async fn ollama_chat_materialized() {
    let component = make_ollama_component();
    let endpoint = component
        .create_endpoint(
            "llm:chat?provider=local&stream=false",
            &camel_component_api::NoOpComponentContext,
        )
        .expect("endpoint");

    let mut producer = endpoint
        .create_producer(make_rt(), &make_producer_ctx())
        .expect("producer");

    let exchange = Exchange::new(Message::new(Body::Text(
        "What is 2+2? Reply with just the number.".into(),
    )));
    let result = producer.call(exchange).await.expect("producer ok");

    match &result.input.body {
        Body::Text(text) => {
            println!("Materialized response: {text}");
            assert!(!text.is_empty(), "response should not be empty");
            assert!(text.contains("4"), "response should contain the answer '4'");
        }
        other => panic!("expected Body::Text, got {other:?}"),
    }

    assert_eq!(
        result
            .input
            .headers
            .get(CAMEL_LLM_USAGE_AVAILABLE)
            .and_then(|v| v.as_bool()),
        Some(true),
        "usage should be available for materialized"
    );
    assert!(
        result.input.headers.contains_key(CAMEL_LLM_TOKENS_IN),
        "should have token count header"
    );
    assert!(
        result.input.headers.contains_key(CAMEL_LLM_MODEL),
        "should have model header"
    );
}

#[tokio::test]
#[ignore = "requires local Ollama with embeddinggemma"]
async fn ollama_embed() {
    let component = make_ollama_component();
    let endpoint = component
        .create_endpoint(
            "llm:embed?provider=local&model=embeddinggemma:latest",
            &camel_component_api::NoOpComponentContext,
        )
        .expect("endpoint");

    let mut producer = endpoint
        .create_producer(make_rt(), &make_producer_ctx())
        .expect("producer");

    let exchange = Exchange::new(Message::new(Body::Text("Hello, world!".into())));
    let result = producer.call(exchange).await.expect("producer ok");

    match &result.input.body {
        Body::Json(value) => {
            let embeddings = value.as_array().expect("embeddings should be a JSON array");
            assert_eq!(
                embeddings.len(),
                1,
                "expected 1 embedding vector for single input"
            );
            let vec = embeddings[0]
                .as_array()
                .expect("embedding should be an array of floats");
            assert!(!vec.is_empty(), "embedding vector should not be empty");
            println!("Embedding dimensions: {}", vec.len());
        }
        other => panic!("expected Body::Json, got {other:?}"),
    }

    assert_eq!(
        result
            .input
            .headers
            .get(CAMEL_LLM_USAGE_AVAILABLE)
            .and_then(|v| v.as_bool()),
        Some(true)
    );
    assert_eq!(
        result
            .input
            .headers
            .get(CAMEL_LLM_MODEL)
            .and_then(|v| v.as_str()),
        Some("embeddinggemma:latest")
    );
}

#[tokio::test]
#[ignore = "requires local Ollama with qwen3.5:4b"]
async fn ollama_chat_with_system_prompt() {
    let component = make_ollama_component();
    let endpoint = component
        .create_endpoint(
            "llm:chat?provider=local&stream=false",
            &camel_component_api::NoOpComponentContext,
        )
        .expect("endpoint");

    let mut producer = endpoint
        .create_producer(make_rt(), &make_producer_ctx())
        .expect("producer");

    let mut exchange = Exchange::new(Message::new(Body::Text("What is your name?".into())));
    exchange.input.headers.insert(
        CAMEL_LLM_SYSTEM_PROMPT.to_string(),
        serde_json::Value::String("You are a helpful assistant named Bob.".into()),
    );

    let result = producer.call(exchange).await.expect("producer ok");

    if let Body::Text(text) = &result.input.body {
        println!("System prompt response: {text}");
        assert!(
            text.to_lowercase().contains("bob"),
            "response should reference the system prompt identity"
        );
    } else {
        panic!("expected Body::Text");
    }
}

// -----------------------------------------------------------------------
// Ollama chat with tools (requires a model that supports tools)
// -----------------------------------------------------------------------
//
// Prerequisites:
//   - Ollama with qwen3.5:4b (supports tools)
//   - Tools must be defined via CamelLlmTools header
//   - Response must contain either ChatEvent::ToolCall (streaming)
//     or CamelLlmToolCalls header (materialized)

#[tokio::test]
#[ignore = "requires local Ollama with qwen3.5:4b (tool-supporting model)"]
async fn ollama_chat_with_tools() {
    let component = make_ollama_component();
    let endpoint = component
        .create_endpoint(
            "llm:chat?provider=local&stream=false",
            &camel_component_api::NoOpComponentContext,
        )
        .expect("endpoint");

    let mut producer = endpoint
        .create_producer(make_rt(), &make_producer_ctx())
        .expect("producer");

    let mut exchange = Exchange::new(Message::new(Body::Text(
        "What is the weather in London? Use the get_weather tool.".into(),
    )));
    // Set tool definitions header
    exchange.input.headers.insert(
        camel_component_llm::headers::CAMEL_LLM_TOOLS.to_string(),
        serde_json::json!([
            {
                "name": "get_weather",
                "description": "Get weather for a city",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "city": {"type": "string"}
                    },
                    "required": ["city"]
                }
            }
        ]),
    );

    let result = producer.call(exchange).await.expect("producer ok");

    // Materialized mode with tools: expect CamelLlmToolCalls header
    // or Body::Text (model may respond with text instead of calling the tool)
    let has_tool_calls = result
        .input
        .headers
        .contains_key(camel_component_llm::headers::CAMEL_LLM_TOOL_CALLS);
    if has_tool_calls {
        let tool_calls_val = result
            .input
            .headers
            .get(camel_component_llm::headers::CAMEL_LLM_TOOL_CALLS)
            .expect("tool calls header");
        println!("Tool calls: {tool_calls_val}");
        let tool_calls: Vec<camel_component_llm::EmittedToolCall> =
            serde_json::from_value(tool_calls_val.clone()).expect("valid EmittedToolCall array");
        assert!(!tool_calls.is_empty(), "expected at least one tool call");
        assert_eq!(tool_calls[0].name, "get_weather");
    } else {
        // Model may respond with text instead of tool call — that's acceptable
        match &result.input.body {
            Body::Text(text) => {
                println!("Text response (no tool call): {text}");
                assert!(!text.is_empty());
            }
            other => panic!("expected Text body when no tool calls, got: {other:?}"),
        }
    }
}

// -----------------------------------------------------------------------
// Ollama multi-turn conversation with tool result
// -----------------------------------------------------------------------
//
// Sends a multi-turn conversation [User, Assistant{tool_calls}, Tool]
// via CamelLlmMessages and verifies the model understands the context.
//
// Prerequisites:
//   - Ollama with qwen3.5:4b (supports tools)
//   - Tool definitions set via CamelLlmTools header

#[tokio::test]
#[ignore = "requires local Ollama with qwen3.5:4b (tool-supporting model)"]
async fn ollama_multi_turn() {
    let component = make_ollama_component();
    let endpoint = component
        .create_endpoint(
            "llm:chat?provider=local&stream=false",
            &camel_component_api::NoOpComponentContext,
        )
        .expect("endpoint");

    let mut producer = endpoint
        .create_producer(make_rt(), &make_producer_ctx())
        .expect("producer");

    let mut exchange = Exchange::new(Message::new(Body::Text("irrelevant".into())));

    // Set tool definitions (needed so the model knows the tool schema)
    exchange.input.headers.insert(
        camel_component_llm::headers::CAMEL_LLM_TOOLS.to_string(),
        serde_json::json!([
            {
                "name": "get_weather",
                "description": "Get weather for a city",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "city": {"type": "string"}
                    },
                    "required": ["city"]
                }
            }
        ]),
    );

    // Multi-turn conversation: User asks → Assistant calls tool → Tool returns result
    let messages = serde_json::json!([
        {
            "role": "User",
            "content": "What's the weather in London?",
            "tool_calls": null,
        },
        {
            "role": "Assistant",
            "content": "",
            "tool_calls": [
                {
                    "id": "call_london",
                    "name": "get_weather",
                    "arguments": r#"{"city":"London"}"#,
                }
            ],
        },
        {
            "role": {"Tool": {"tool_call_id": "call_london"}},
            "content": r#"{"temperature": 22, "unit": "C", "conditions": "sunny"}"#,
            "tool_calls": null,
        },
    ]);
    exchange.input.headers.insert(
        camel_component_llm::headers::CAMEL_LLM_MESSAGES.to_string(),
        messages,
    );

    let result = producer.call(exchange).await.expect("producer ok");

    match &result.input.body {
        Body::Text(text) => {
            println!("Multi-turn response: {text}");
            assert!(!text.is_empty(), "response should not be empty");
            // The model should reference the weather data provided in the tool result
            assert!(
                text.to_lowercase().contains("22") || text.to_lowercase().contains("sunny"),
                "response should reference tool result data, got: {text}"
            );
        }
        other => panic!("expected Body::Text, got: {other:?}"),
    }
}

// -----------------------------------------------------------------------
// Ollama cost computed
// -----------------------------------------------------------------------
//
// Configures pricing in the provider config and verifies that the
// CamelLlmEstimatedCostUsd header is set on the materialized response.
//
// Prerequisites:
//   - Ollama with qwen3.5:4b

#[tokio::test]
#[ignore = "requires local Ollama with qwen3.5:4b and pricing configured"]
async fn ollama_cost_computed() {
    let mut providers = std::collections::HashMap::new();
    providers.insert(
        "local".into(),
        camel_component_llm::LlmProviderConfig::Ollama(
            camel_component_llm::config::OllamaProviderConfig {
                base_url: OLLAMA_BASE_URL.into(),
                default_model: CHAT_MODEL.into(),
                timeout_secs: None,
                max_concurrency: None,
                network_retry: None,
                pricing: Some(camel_component_llm::cost::PricingTable {
                    input_per_1k_tokens: 0.0025,
                    output_per_1k_tokens: 0.01,
                }),
                cache_ttl_secs: None,
                cache_max_entries: None,
            },
        ),
    );
    let component = camel_component_llm::LlmComponent::new(camel_component_llm::LlmGlobalConfig {
        providers,
        ..Default::default()
    })
    .expect("component");

    let endpoint = component
        .create_endpoint(
            "llm:chat?provider=local&stream=false",
            &camel_component_api::NoOpComponentContext,
        )
        .expect("endpoint");

    let mut producer = endpoint
        .create_producer(make_rt(), &make_producer_ctx())
        .expect("producer");

    let exchange = Exchange::new(Message::new(Body::Text("Say hello in one word.".into())));
    let result = producer.call(exchange).await.expect("producer ok");

    assert!(
        result
            .input
            .headers
            .contains_key(camel_component_llm::headers::CAMEL_LLM_ESTIMATED_COST_USD),
        "CamelLlmEstimatedCostUsd header must be set when pricing is configured"
    );
    let cost = result
        .input
        .headers
        .get(camel_component_llm::headers::CAMEL_LLM_ESTIMATED_COST_USD)
        .and_then(|v| v.as_f64());
    assert!(cost.is_some(), "cost must be a number");
    assert!(
        cost.unwrap() > 0.0,
        "cost must be positive for non-zero token usage"
    );
    println!("Estimated cost: ${:.6}", cost.unwrap());
}

// -----------------------------------------------------------------------
// Ollama cache hit
// -----------------------------------------------------------------------
//
// Sends the same prompt twice with cache configured. The first call
// populates the cache; the second should return the cached value
// without calling the provider again.
//
// Prerequisites:
//   - Ollama with qwen3.5:4b

#[tokio::test]
#[ignore = "requires local Ollama with qwen3.5:4b and cache configured"]
async fn ollama_cache_hit() {
    let mut providers = std::collections::HashMap::new();
    providers.insert(
        "local".into(),
        camel_component_llm::LlmProviderConfig::Ollama(
            camel_component_llm::config::OllamaProviderConfig {
                base_url: OLLAMA_BASE_URL.into(),
                default_model: CHAT_MODEL.into(),
                timeout_secs: None,
                max_concurrency: None,
                network_retry: None,
                pricing: None,
                cache_ttl_secs: Some(60),
                cache_max_entries: Some(100),
            },
        ),
    );
    let component = camel_component_llm::LlmComponent::new(camel_component_llm::LlmGlobalConfig {
        providers,
        ..Default::default()
    })
    .expect("component");

    let endpoint = component
        .create_endpoint(
            "llm:chat?provider=local&stream=false",
            &camel_component_api::NoOpComponentContext,
        )
        .expect("endpoint");

    let mut producer = endpoint
        .create_producer(make_rt(), &make_producer_ctx())
        .expect("producer");

    // First call — should hit the provider and cache the result
    let exchange1 = Exchange::new(Message::new(Body::Text(
        "Reply with the word 'cached'.".into(),
    )));
    let start1 = std::time::Instant::now();
    let result1 = producer.call(exchange1).await.expect("first call ok");
    let elapsed1 = start1.elapsed();

    let text1 = match &result1.input.body {
        Body::Text(s) => s.clone(),
        other => panic!("expected Body::Text, got: {other:?}"),
    };
    println!("First call ({elapsed1:?}): {text1}");

    // Second call — should hit the cache and return much faster
    let exchange2 = Exchange::new(Message::new(Body::Text(
        "Reply with the word 'cached'.".into(),
    )));
    let start2 = std::time::Instant::now();
    let result2 = producer.call(exchange2).await.expect("second call ok");
    let elapsed2 = start2.elapsed();

    let text2 = match &result2.input.body {
        Body::Text(s) => s.clone(),
        other => panic!("expected Body::Text, got: {other:?}"),
    };
    println!("Second call ({elapsed2:?}): {text2}");

    // Both responses should be identical (from cache)
    assert_eq!(text1, text2, "cached response must match original");
    // Second call should be significantly faster (cache hit ~microseconds vs provider ~seconds)
    assert!(
        elapsed2 < elapsed1 / 2,
        "cache hit must be significantly faster (elapsed1={elapsed1:?}, elapsed2={elapsed2:?})"
    );
}
