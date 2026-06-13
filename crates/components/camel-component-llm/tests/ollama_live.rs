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
        }),
    );
    LlmComponent::new(LlmGlobalConfig {
        providers,
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
