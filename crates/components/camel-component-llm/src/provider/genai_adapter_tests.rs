//! Unit tests for the pure genai conversion/error-mapping layer.

use std::time::Duration;

use super::*;
use crate::LlmHttpPolicy;
use crate::error::{LlmError, TransportTimeoutPhase};
#[cfg(any(feature = "openai", feature = "ollama", feature = "all-providers"))]
use crate::provider::LlmProvider;
use crate::provider::http_errors::is_insufficient_quota;
use crate::provider::{
    ChatMessage, ChatRole, EmbedRequest, EmittedToolCall, FinishReason, LlmUsage, ToolChoice,
    ToolDefinition,
};
#[cfg(any(feature = "openai", feature = "ollama", feature = "all-providers"))]
use camel_api::SsrfPolicy;
#[cfg(any(feature = "ollama", feature = "all-providers"))]
use futures::future::join_all;
use genai::adapter::AdapterKind;
use genai::chat::{StopReason, ToolName, Usage};
use genai::resolver::AuthData;
use genai::{ModelIden, ModelSpec};

fn params() -> serde_json::Map<String, serde_json::Value> {
    serde_json::json!({
        "type": "object",
        "properties": { "city": { "type": "string" } },
        "required": ["city"]
    })
    .as_object()
    .expect("object")
    .clone()
}

fn model_iden() -> ModelIden {
    ModelIden::new(AdapterKind::OpenAI, "gpt-4o")
}

async fn connect_error() -> reqwest::Error {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    drop(listener);

    reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_millis(500))
        .build()
        .expect("client")
        .get(format!("http://{addr}/"))
        .send()
        .await
        .expect_err("connect must fail")
}

async fn timeout_error() -> reqwest::Error {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    let server = tokio::spawn(async move {
        let (_socket, _) = listener.accept().await.expect("accept");
        tokio::time::sleep(Duration::from_secs(5)).await;
    });

    let err = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_millis(50))
        .build()
        .expect("client")
        .get(format!("http://{addr}/"))
        .send()
        .await
        .expect_err("timeout must fail");

    server.abort();
    err
}

fn web_stream(err: reqwest::Error) -> genai::Error {
    genai::Error::WebStream {
        model_iden: model_iden(),
        cause: "transport".into(),
        error: Box::new(err),
    }
}

#[test]
fn map_tool_definition_function_name_and_description() {
    let def = ToolDefinition {
        name: "get_weather".into(),
        description: "Get weather for a city".into(),
        parameters: params(),
    };

    let tool = map_tool_definition(def);

    assert_eq!(tool.name, ToolName::Custom("get_weather".into()));
    assert_eq!(tool.description.as_deref(), Some("Get weather for a city"));
    assert_eq!(tool.schema, Some(serde_json::Value::Object(params())));
}

#[test]
fn map_tool_choice_auto_none_specific() {
    assert_eq!(
        map_tool_choice(&ToolChoice::Auto),
        genai::chat::ToolChoice::Auto
    );
    assert_eq!(
        map_tool_choice(&ToolChoice::None),
        genai::chat::ToolChoice::None
    );

    match map_tool_choice(&ToolChoice::Specific("get_weather".into())) {
        genai::chat::ToolChoice::Tool { name } => assert_eq!(name, "get_weather"),
        other => panic!("expected specific tool choice, got {other:?}"),
    }
}

#[test]
fn convert_chat_message_tool_role_keeps_tool_call_id() {
    let msg = ChatMessage {
        role: ChatRole::Tool {
            tool_call_id: "call_123".into(),
        },
        content: "weather result".into(),
        tool_calls: None,
    };

    let out = map_chat_message(&msg);

    assert_eq!(out.role, genai::chat::ChatRole::Tool);
    let responses = out.content.into_tool_responses();
    assert_eq!(responses.len(), 1);
    assert_eq!(responses[0].call_id, "call_123");
    assert_eq!(responses[0].content, "weather result");
}

#[test]
fn convert_chat_message_assistant_forwards_tool_calls() {
    let msg = ChatMessage {
        role: ChatRole::Assistant,
        content: "I'll check the weather".into(),
        tool_calls: Some(vec![EmittedToolCall {
            id: "call_123".into(),
            name: "get_weather".into(),
            arguments: r#"{"city":"London"}"#.into(),
        }]),
    };

    let out = map_chat_message(&msg);

    assert_eq!(out.role, genai::chat::ChatRole::Assistant);
    let parts: Vec<_> = out.content.into_iter().collect();
    let tool_call = parts
        .iter()
        .find_map(|part| part.as_tool_call())
        .expect("assistant tool call");
    assert_eq!(tool_call.call_id, "call_123");
    assert_eq!(tool_call.fn_name, "get_weather");
    assert_eq!(
        tool_call.fn_arguments,
        serde_json::json!({ "city": "London" })
    );
}

#[test]
fn chat_usage_missing_completion_is_none() {
    let usage = Usage {
        prompt_tokens: Some(10),
        ..Usage::default()
    };
    assert_eq!(convert_usage(&usage), None);
}

#[test]
fn chat_usage_totals_default_to_sum() {
    let usage = Usage {
        prompt_tokens: Some(10),
        completion_tokens: Some(5),
        total_tokens: None,
        ..Usage::default()
    };
    assert_eq!(
        convert_usage(&usage),
        Some(LlmUsage {
            prompt_tokens: 10,
            completion_tokens: 5,
            total_tokens: 15,
        })
    );
}

#[test]
fn chat_usage_negative_counts_are_missing() {
    let usage = Usage {
        prompt_tokens: Some(-1),
        completion_tokens: Some(5),
        ..Usage::default()
    };
    assert_eq!(convert_usage(&usage), None);
}

#[test]
fn embed_usage_completion_zero_total_defaults_to_prompt() {
    assert_eq!(
        convert_embed_usage(Some(7), None),
        Some(LlmUsage {
            prompt_tokens: 7,
            completion_tokens: 0,
            total_tokens: 7,
        })
    );
}

#[test]
fn finish_reason_maps_all_variants() {
    assert!(matches!(
        map_finish_reason(&StopReason::Completed("stop".into())),
        FinishReason::Stop
    ));
    assert!(matches!(
        map_finish_reason(&StopReason::StopSequence("stop_sequence".into())),
        FinishReason::Stop
    ));
    assert!(matches!(
        map_finish_reason(&StopReason::MaxTokens("length".into())),
        FinishReason::Length
    ));
    assert!(matches!(
        map_finish_reason(&StopReason::ToolCall("tool_calls".into())),
        FinishReason::ToolCall
    ));
    assert!(matches!(
        map_finish_reason(&StopReason::ContentFilter("content_filter".into())),
        FinishReason::ContentFilter
    ));

    match map_finish_reason(&StopReason::Other("failed".into())) {
        FinishReason::Other(reason) => assert_eq!(reason, "failed"),
        other => panic!("expected Other, got {other:?}"),
    }
}

#[test]
fn resolve_target_openai_kind_for_ollama_named_model() {
    let spec = resolve_target(
        AdapterKind::OpenAI,
        "https://api.example.com/v1",
        Some("sk"),
        "qwen3.5:4b",
    );

    match spec {
        ModelSpec::Target(target) => {
            assert_eq!(target.model.adapter_kind, AdapterKind::OpenAI);
            assert_eq!(target.model.model_name.as_str(), "qwen3.5:4b");
            assert_eq!(target.endpoint.base_url(), "https://api.example.com/v1/");
            assert!(matches!(&target.auth, AuthData::Key(key) if key == "sk"));
        }
        other => panic!("expected ModelSpec::Target, got {other:?}"),
    }
}

#[test]
fn resolve_target_ollama_auth_none() {
    let spec = resolve_target(
        AdapterKind::Ollama,
        "http://localhost:11434",
        None,
        "llama3",
    );

    match spec {
        ModelSpec::Target(target) => {
            assert_eq!(target.model.adapter_kind, AdapterKind::Ollama);
            assert!(matches!(target.auth, AuthData::None));
        }
        other => panic!("expected ModelSpec::Target, got {other:?}"),
    }
}

#[test]
fn resolve_target_appends_trailing_slash() {
    let spec = resolve_target(
        AdapterKind::Ollama,
        "http://localhost:11434",
        None,
        "llama3",
    );

    match spec {
        ModelSpec::Target(target) => {
            let endpoint = target.endpoint.base_url();
            assert!(endpoint.ends_with('/'));
            assert_eq!(endpoint, "http://localhost:11434/");
            assert!(!endpoint.contains("11434api/chat"));
        }
        other => panic!("expected ModelSpec::Target, got {other:?}"),
    }
}

#[cfg(any(feature = "openai", feature = "all-providers"))]
#[test]
fn openai_default_endpoint_when_base_url_none() {
    assert_eq!(openai_endpoint_for(None), "https://api.openai.com/v1/");
    assert_eq!(
        openai_endpoint_for(Some("http://host:9/v1")),
        "http://host:9/v1/"
    );
}

#[test]
fn embed_model_defaults_to_provider_default() {
    let mut req = EmbedRequest::new("", vec!["hello".into()]);
    assert_eq!(embed_model_for(&req, "embeddinggemma"), "embeddinggemma");

    req.model = "nomic-embed-text".into();
    assert_eq!(embed_model_for(&req, "embeddinggemma"), "nomic-embed-text");
}

#[test]
fn map_error_401_403_authfailed() {
    for status in [
        reqwest::StatusCode::UNAUTHORIZED,
        reqwest::StatusCode::FORBIDDEN,
    ] {
        let err = genai::Error::HttpError {
            status,
            canonical_reason: status.canonical_reason().unwrap_or("").into(),
            body: "access denied".into(),
        };
        assert!(matches!(
            map_genai_error(&err, LlmHttpPolicy::chat()),
            LlmError::AuthFailed { .. }
        ));
    }
}

#[test]
fn map_error_404_model_not_found() {
    let err = genai::Error::HttpError {
        status: reqwest::StatusCode::NOT_FOUND,
        canonical_reason: "Not Found".into(),
        body: "model gpt-99 does not exist".into(),
    };

    match map_genai_error(&err, LlmHttpPolicy::chat()) {
        LlmError::ModelNotFound(model) => assert!(!model.is_empty()),
        other => panic!("expected ModelNotFound, got {other:?}"),
    }
}

#[test]
fn map_error_400_422_invalid_request() {
    for status in [
        reqwest::StatusCode::BAD_REQUEST,
        reqwest::StatusCode::UNPROCESSABLE_ENTITY,
    ] {
        let err = genai::Error::HttpError {
            status,
            canonical_reason: status.canonical_reason().unwrap_or("").into(),
            body: "bad request".into(),
        };
        assert!(matches!(
            map_genai_error(&err, LlmHttpPolicy::chat()),
            LlmError::InvalidRequest(_)
        ));
    }
}

#[test]
fn map_error_5xx_provider_unavailable() {
    for status in [
        reqwest::StatusCode::INTERNAL_SERVER_ERROR,
        reqwest::StatusCode::SERVICE_UNAVAILABLE,
    ] {
        let err = genai::Error::HttpError {
            status,
            canonical_reason: status.canonical_reason().unwrap_or("").into(),
            body: "server error".into(),
        };
        assert!(matches!(
            map_genai_error(&err, LlmHttpPolicy::chat()),
            LlmError::ProviderUnavailable(_)
        ));
    }
}

#[tokio::test]
async fn map_error_timeout_and_connect() {
    let connect = connect_error().await;
    assert!(connect.is_connect());
    assert!(matches!(
        map_genai_error(&web_stream(connect), LlmHttpPolicy::chat()),
        LlmError::Network(_)
    ));

    // A pre-header transport timeout is a NOT-connect transport timeout: it
    // maps to the read-idle layer and reports the policy bound the transport
    // actually enforced (600s chat guard), never a configured deadline.
    let timeout = timeout_error().await;
    assert!(timeout.is_timeout());
    assert!(!timeout.is_connect());
    match map_genai_error(&web_stream(timeout), LlmHttpPolicy::chat()) {
        LlmError::TransportTimeout {
            phase: TransportTimeoutPhase::Idle,
            after,
        } => assert_eq!(after, Duration::from_secs(600)),
        other => panic!("expected TransportTimeout::Idle, got {other:?}"),
    }
}

#[test]
fn webc_404_model_not_found_from_model_iden() {
    let err = genai::Error::WebModelCall {
        model_iden: model_iden(),
        webc_error: genai::webc::Error::ResponseFailedStatus {
            status: reqwest::StatusCode::NOT_FOUND,
            body: "not found html".into(),
            headers: Box::new(reqwest::header::HeaderMap::new()),
        },
    };

    match map_genai_error(&err, LlmHttpPolicy::chat()) {
        LlmError::ModelNotFound(model) => assert_eq!(model, "gpt-4o"),
        other => panic!("expected ModelNotFound, got {other:?}"),
    }
}

#[test]
fn web_stream_unknown_genai_inner_maps_through_mapper() {
    let serde_error = serde_json::from_str::<serde_json::Value>("{").expect_err("invalid json");
    let err = genai::Error::WebStream {
        model_iden: model_iden(),
        cause: "stream".into(),
        error: Box::new(genai::Error::StreamParse {
            model_iden: model_iden(),
            serde_error,
        }),
    };

    assert!(matches!(
        map_genai_error(&err, LlmHttpPolicy::chat()),
        LlmError::Protocol(_)
    ));
}

#[test]
fn map_error_stream_parse_protocol() {
    let serde_error = serde_json::from_str::<serde_json::Value>("{").expect_err("invalid json");
    let err = genai::Error::StreamParse {
        model_iden: model_iden(),
        serde_error,
    };

    assert!(matches!(
        map_genai_error(&err, LlmHttpPolicy::chat()),
        LlmError::Protocol(_)
    ));
}

fn headers(retry_after: &str) -> Box<reqwest::header::HeaderMap> {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        reqwest::header::RETRY_AFTER,
        reqwest::header::HeaderValue::from_str(retry_after).expect("header value"),
    );
    Box::new(headers)
}

fn response_failed(
    status: reqwest::StatusCode,
    body: &str,
    retry_after: &str,
) -> genai::webc::Error {
    genai::webc::Error::ResponseFailedStatus {
        status,
        body: body.into(),
        headers: headers(retry_after),
    }
}

#[test]
fn webc_response_failed_status_maps_status_and_retry_after() {
    let rate_limit_body = r#"{"error":{"code":"rate_limit_exceeded","message":"slow down"}}"#;
    let err = genai::Error::WebModelCall {
        model_iden: model_iden(),
        webc_error: response_failed(reqwest::StatusCode::TOO_MANY_REQUESTS, rate_limit_body, "9"),
    };

    match map_genai_error(&err, LlmHttpPolicy::chat()) {
        LlmError::RateLimit { retry_after } => {
            assert_eq!(retry_after, Some(Duration::from_secs(9)))
        }
        other => panic!("expected RateLimit, got {other:?}"),
    }

    let err = genai::Error::WebModelCall {
        model_iden: model_iden(),
        webc_error: response_failed(
            reqwest::StatusCode::TOO_MANY_REQUESTS,
            rate_limit_body,
            "Wed, 21 Oct 2026 07:28:00 GMT",
        ),
    };

    match map_genai_error(&err, LlmHttpPolicy::chat()) {
        LlmError::RateLimit { retry_after } => assert_eq!(retry_after, None),
        other => panic!("expected RateLimit, got {other:?}"),
    }
}

#[tokio::test]
async fn webc_reqwest_timeout_maps_to_transport_idle() {
    let timeout = timeout_error().await;
    let err = genai::Error::WebModelCall {
        model_iden: model_iden(),
        webc_error: genai::webc::Error::Reqwest(timeout),
    };

    match map_genai_error(&err, LlmHttpPolicy::chat()) {
        LlmError::TransportTimeout {
            phase: TransportTimeoutPhase::Idle,
            after,
        } => assert_eq!(after, Duration::from_secs(600)),
        other => panic!("expected TransportTimeout::Idle, got {other:?}"),
    }
}

#[test]
fn web_adapter_call_variant_maps_identically() {
    let err = genai::Error::WebAdapterCall {
        adapter_kind: AdapterKind::OpenAI,
        webc_error: response_failed(
            reqwest::StatusCode::SERVICE_UNAVAILABLE,
            "upstream unavailable",
            "",
        ),
    };

    assert!(matches!(
        map_genai_error(&err, LlmHttpPolicy::chat()),
        LlmError::ProviderUnavailable(_)
    ));
}

#[test]
fn insufficient_quota_detected_by_code_and_type() {
    assert!(is_insufficient_quota(
        429,
        r#"{"error":{"code":"insufficient_quota"}}"#
    ));
    assert!(is_insufficient_quota(
        429,
        r#"{"error":{"type":"insufficient_quota"}}"#
    ));
    assert!(!is_insufficient_quota(429, r#"{"error":"rate limit"}"#));
    assert!(!is_insufficient_quota(429, "not json"));
    assert!(!is_insufficient_quota(
        500,
        r#"{"error":{"code":"insufficient_quota"}}"#
    ));
}

#[test]
fn retry_after_delta_seconds_parsed_http_date_rejected() {
    assert_eq!(parse_retry_after(Some("7")), Some(Duration::from_secs(7)));
    assert_eq!(
        parse_retry_after(Some("Wed, 21 Oct 2026 07:28:00 GMT")),
        None
    );
    assert_eq!(parse_retry_after(Some("soon")), None);
    assert_eq!(parse_retry_after(None), None);
}

#[cfg(any(feature = "openai", feature = "all-providers"))]
#[test]
fn genai_provider_implements_llm_provider() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<GenaiProvider>();

    let provider = GenaiProvider::new(
        "openai",
        "gpt-4o",
        GenaiConfig::OpenAi {
            api_key: "sk-test".into(),
            base_url: None,
        },
        SsrfPolicy::AllowInternal,
    );

    let provider: &dyn LlmProvider = &provider;
    assert_eq!(provider.id(), "openai");
    assert_eq!(provider.default_model(), "gpt-4o");
    assert!(provider.supports_embed());
}

#[test]
fn convert_chat_message_assistant_without_tool_calls_stays_text() {
    for tool_calls in [None, Some(vec![])] {
        let msg = ChatMessage {
            role: ChatRole::Assistant,
            content: "Just a plain response.".into(),
            tool_calls,
        };

        let out = map_chat_message(&msg);
        assert_eq!(out.role, genai::chat::ChatRole::Assistant);
        let parts: Vec<_> = out.content.into_iter().collect();
        assert_eq!(parts.len(), 1, "expected exactly the text part");
        assert!(matches!(&parts[0], ContentPart::Text(text) if text == "Just a plain response."));
    }
}

#[test]
fn chat_request_carries_system_tools_and_tool_choice() {
    let tool = ToolDefinition {
        name: "get_weather".into(),
        description: "Get weather".into(),
        parameters: params(),
    };
    let req = ChatRequest {
        model: "gpt-4o".into(),
        messages: vec![ChatMessage::user("weather?")],
        temperature: Some(0.2),
        max_tokens: Some(64),
        stop: Some(vec!["END".into()]),
        system_prompt: Some("be terse".into()),
        tools: vec![tool],
        tool_choice: Some(ToolChoice::Specific("get_weather".into())),
        extra: serde_json::Map::new(),
    };

    let out = build_chat_request(&req);
    assert_eq!(out.system.as_deref(), Some("be terse"));
    assert_eq!(out.messages.len(), 1);
    let tools = out.tools.expect("tools should be Some");
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name, ToolName::Custom("get_weather".into()));

    let options = build_chat_options(&req);
    assert_eq!(options.temperature, Some(0.2));
    assert_eq!(options.max_tokens, Some(64));
    assert_eq!(options.stop_sequences, vec!["END".to_string()]);
    assert_eq!(
        options.tool_choice,
        Some(genai::chat::ToolChoice::Tool {
            name: "get_weather".into()
        })
    );
}

#[cfg(any(feature = "openai", feature = "all-providers"))]
#[tokio::test]
async fn build_client_rejects_ssrf_link_local_base_url() {
    let config = GenaiConfig::OpenAi {
        api_key: "sk-test".into(),
        base_url: Some("http://169.254.169.254/".into()),
    };
    let result = build_client_from_config(&config, SsrfPolicy::PublicHttpsOnly).await;
    let err = result.expect_err("expected SSRF rejection"); // allow-unwrap
    assert!(
        matches!(err, LlmError::InvalidRequest(_)),
        "expected InvalidRequest, got: {err}"
    );
    assert!(
        err.to_string().contains("blocked"),
        "expected SSRF-blocked message, got: {err}"
    );
}

#[cfg(any(feature = "ollama", feature = "all-providers"))]
#[tokio::test]
async fn build_client_rejects_ollama_ssrf_link_local() {
    let config = GenaiConfig::Ollama {
        base_url: "http://169.254.169.254/".into(),
    };
    let result = build_client_from_config(&config, SsrfPolicy::PublicHttpsOnly).await;
    let err = result.expect_err("expected SSRF rejection"); // allow-unwrap
    assert!(
        matches!(err, LlmError::InvalidRequest(_)),
        "expected InvalidRequest, got: {err}"
    );
    assert!(
        err.to_string().contains("blocked"),
        "expected SSRF-blocked message, got: {err}"
    );
}

#[cfg(any(feature = "ollama", feature = "all-providers"))]
#[tokio::test]
async fn client_built_once_under_concurrency() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/embed"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "model": "llama3",
            "embeddings": [[0.1, 0.2]],
            "prompt_eval_count": 1
        })))
        .mount(&server)
        .await;

    // Bypass ambient proxies so every racing call reaches the mock server.
    let stub_builder: BuildOverride = Arc::new(|| {
        Box::pin(async {
            let client = reqwest::Client::builder()
                .no_proxy()
                .build()
                .map_err(|err| LlmError::Network(format!("test reqwest client: {err}")))?;
            Ok(genai::Client::builder().with_reqwest(client).build())
        })
    });

    let provider = Arc::new(GenaiProvider::new_with_build_override(
        "test",
        "llama3",
        GenaiConfig::Ollama {
            base_url: server.uri(),
        },
        LlmHttpPolicy::chat(),
        stub_builder,
        SsrfPolicy::AllowInternal,
    ));
    let builds = Arc::clone(&provider.client_builds);

    // 8 concurrent calls to the production `embed()` method exercise the
    // OnceCell path; every call must share the one cached client and succeed.
    let mut futs = vec![];
    for _ in 0..8 {
        let p = Arc::clone(&provider);
        futs.push(async move {
            p.embed(EmbedRequest::new("llama3", vec!["hello".into()]))
                .await
                .expect("all racing calls share the cached client and succeed")
        });
    }
    let responses = join_all(futs).await;
    assert_eq!(responses.len(), 8);
    assert_eq!(
        builds.load(Ordering::SeqCst),
        1,
        "OnceCell must construct the client exactly once across concurrent embed() calls"
    );
}

#[cfg(any(feature = "openai", feature = "all-providers"))]
#[test]
fn build_openai_builds_genai_provider() {
    use crate::config::OpenaiProviderConfig;

    let config = OpenaiProviderConfig {
        api_key: "sk-test".into(),
        base_url: None,
        default_model: "gpt-4o".into(),
        timeout_secs: None,
        max_concurrency: None,
        network_retry: None,
        pricing: None,
        cache_ttl_secs: None,
        cache_max_entries: None,
    };

    let provider = build_openai(
        "oai",
        &config,
        Duration::from_secs(30),
        SsrfPolicy::PublicHttpsOnly,
    )
    .expect("build_openai must not touch the network");

    assert_eq!(provider.id(), "oai");
    assert_eq!(provider.default_model(), "gpt-4o");
}

#[cfg(any(feature = "ollama", feature = "all-providers"))]
#[test]
fn build_ollama_builds_genai_provider() {
    use crate::config::OllamaProviderConfig;

    let config = OllamaProviderConfig {
        base_url: "http://localhost:11434".into(),
        default_model: "llama3".into(),
        timeout_secs: None,
        max_concurrency: None,
        network_retry: None,
        pricing: None,
        cache_ttl_secs: None,
        cache_max_entries: None,
    };

    let provider = build_ollama(
        "ol",
        &config,
        Duration::from_secs(30),
        SsrfPolicy::AllowInternal,
    )
    .expect("build_ollama must not touch the network");

    assert_eq!(provider.id(), "ol");
    assert_eq!(provider.default_model(), "llama3");
}

/// bd rc-ga0kl: an unresolvable base_url boots fine (provider
/// construction is lazy) and fails closed at FIRST USE, with the same
/// typed error and message the boot-time check used to produce.
#[cfg(any(feature = "ollama", feature = "all-providers"))]
#[tokio::test]
async fn first_use_fails_closed_on_unresolvable_base_url() {
    use crate::config::OllamaProviderConfig;
    use crate::provider::ChatRequest;
    use futures::StreamExt;

    let config = OllamaProviderConfig {
        base_url: "http://no-such-host.invalid:11434".into(),
        default_model: "llama3".into(),
        timeout_secs: None,
        max_concurrency: None,
        network_retry: None,
        pricing: None,
        cache_ttl_secs: None,
        cache_max_entries: None,
    };
    let provider = build_ollama(
        "ol",
        &config,
        Duration::from_secs(30),
        SsrfPolicy::AllowInternal,
    )
    .expect("boot must succeed: DNS resolution moved to first use");

    let mut stream =
        provider.chat_stream(ChatRequest::new("llama3", vec![ChatMessage::user("hi")]));
    let first = stream
        .next()
        .await
        .expect("the fail-closed client build must surface an event");
    let err = first.expect_err("first use must fail closed on an unresolvable host");
    let msg = err.to_string();
    assert!(
        msg.contains("cannot resolve llm base_url host 'no-such-host.invalid'"),
        "expected the typed DNS error, got: {msg}"
    );
}

#[test]
fn body_derived_error_details_are_truncated() {
    let body = "x".repeat(500);

    match map_status(401, &body, None, None) {
        LlmError::AuthFailed { detail } => {
            assert!(detail.starts_with("xxx"));
            assert!(detail.ends_with("...[truncated]"));
            assert!(detail.len() < 500);
        }
        other => panic!("expected AuthFailed, got {other:?}"),
    }

    match map_status(400, &body, None, None) {
        LlmError::InvalidRequest(detail) => assert!(detail.ends_with("...[truncated]")),
        other => panic!("expected InvalidRequest, got {other:?}"),
    }

    match map_status(404, &body, None, None) {
        LlmError::ModelNotFound(model) => assert!(model.ends_with("...[truncated]")),
        other => panic!("expected ModelNotFound, got {other:?}"),
    }

    match map_status(503, &body, None, None) {
        LlmError::ProviderUnavailable(detail) => assert!(detail.ends_with("...[truncated]")),
        other => panic!("expected ProviderUnavailable, got {other:?}"),
    }

    let quota_body = format!(
        r#"{{"error":{{"code":"insufficient_quota","message":"{}"}}}}"#,
        "x".repeat(500)
    );
    match map_status(429, &quota_body, None, None) {
        LlmError::QuotaExceeded { detail } => {
            assert!(detail.ends_with("...[truncated]"));
            assert!(detail.len() < quota_body.len());
        }
        other => panic!("expected QuotaExceeded, got {other:?}"),
    }
}

/// End-to-end acceptance tests for the genai-backed chat stream, driven by a
/// local `wiremock` server. These exercise the real OpenAI adapter path
/// (request serialisation, SSE parsing and event translation) without any
/// external network access.
#[cfg(any(feature = "openai", feature = "all-providers"))]
mod wiremock_chat {
    use std::sync::Arc;

    use camel_api::SsrfPolicy;
    use futures::StreamExt;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::super::{BuildOverride, GenaiConfig, GenaiProvider};
    use crate::LlmHttpPolicy;
    use crate::error::LlmError;
    use crate::provider::{
        ChatEvent, ChatMessage, ChatRequest, FinishReason, LlmProvider, LlmUsage, ToolDefinition,
    };

    type Events = Vec<Result<ChatEvent, LlmError>>;

    /// Build a genai client whose reqwest transport bypasses any ambient proxy,
    /// so the mock server on `127.0.0.1` is always reached directly.
    fn build_override() -> BuildOverride {
        Arc::new(|| {
            Box::pin(async {
                let client = reqwest::Client::builder()
                    .no_proxy()
                    .build()
                    .map_err(|err| LlmError::Network(format!("test reqwest client: {err}")))?;
                Ok(genai::Client::builder().with_reqwest(client).build())
            })
        })
    }

    /// OpenAI provider pointed at the mock server. The base URL deliberately
    /// omits the trailing slash (`/v1`) so the adapter's normalisation is
    /// exercised on every test.
    fn openai_provider(server: &MockServer, model: &str) -> GenaiProvider {
        GenaiProvider::new_with_build_override(
            "wiremock-openai",
            model,
            GenaiConfig::OpenAi {
                api_key: "sk-test".into(),
                base_url: Some(format!("{}/v1", server.uri())),
            },
            LlmHttpPolicy::chat(),
            build_override(),
            SsrfPolicy::AllowInternal,
        )
    }

    /// Join SSE `data:` payloads into a wire body, using the blank line the
    /// server-sent-events parser requires between events.
    fn sse(payloads: &[&str]) -> String {
        payloads
            .iter()
            .map(|payload| format!("data: {payload}\n\n"))
            .collect()
    }

    /// Mock server answering every OpenAI chat-completion request with `body`.
    async fn openai_sse_server(body: &str) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(
                ResponseTemplate::new(200).set_body_raw(body.to_string(), "text/event-stream"),
            )
            .mount(&server)
            .await;
        server
    }

    fn request(model: &str, tools: Vec<ToolDefinition>) -> ChatRequest {
        ChatRequest {
            model: model.into(),
            messages: vec![ChatMessage::user("hello")],
            temperature: None,
            max_tokens: None,
            stop: None,
            system_prompt: None,
            tools,
            tool_choice: None,
            extra: serde_json::Map::new(),
        }
    }

    fn tool(name: &str) -> ToolDefinition {
        ToolDefinition {
            name: name.into(),
            description: format!("{name} tool"),
            parameters: serde_json::json!({
                "type": "object",
                "properties": { "location": { "type": "string" } },
                "required": ["location"]
            })
            .as_object()
            .expect("object schema")
            .clone(),
        }
    }

    async fn collect(provider: &GenaiProvider, req: ChatRequest) -> Events {
        provider.chat_stream(req).collect().await
    }

    fn delta_texts(events: &[Result<ChatEvent, LlmError>]) -> Vec<String> {
        events
            .iter()
            .filter_map(|event| match event {
                Ok(ChatEvent::Delta { text }) => Some(text.clone()),
                _ => None,
            })
            .collect()
    }

    fn tool_calls(events: &[Result<ChatEvent, LlmError>]) -> Vec<(String, String, String)> {
        events
            .iter()
            .filter_map(|event| match event {
                Ok(ChatEvent::ToolCall {
                    id,
                    name,
                    arguments,
                }) => Some((id.clone(), name.clone(), arguments.clone())),
                _ => None,
            })
            .collect()
    }

    fn error_count(events: &[Result<ChatEvent, LlmError>]) -> usize {
        events.iter().filter(|event| event.is_err()).count()
    }

    /// Return the fields of the single `Finished` event, asserting there is
    /// exactly one.
    fn finished_event(
        events: &[Result<ChatEvent, LlmError>],
    ) -> (&Option<LlmUsage>, &Option<String>, &Option<FinishReason>) {
        let mut found = events.iter().filter_map(|event| match event {
            Ok(ChatEvent::Finished {
                usage,
                model,
                finish_reason,
                ..
            }) => Some((usage, model, finish_reason)),
            _ => None,
        });
        let finished = found.next().expect("expected exactly one Finished event");
        assert!(
            found.next().is_none(),
            "expected exactly one Finished event"
        );
        finished
    }

    fn assert_last_is_finished(events: &[Result<ChatEvent, LlmError>]) {
        assert!(
            matches!(events.last(), Some(Ok(ChatEvent::Finished { .. }))),
            "Finished must be the last event, got: {events:?}"
        );
    }

    #[tokio::test]
    async fn openai_stream_maps_deltas_usage_finish() {
        let body = sse(&[
            r#"{"choices":[{"delta":{"content":"He"}}]}"#,
            r#"{"choices":[{"delta":{"content":"llo"}}]}"#,
            r#"{"choices":[{"delta":{"content":"!"}}]}"#,
            r#"{"choices":[{"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":11,"completion_tokens":7,"total_tokens":18}}"#,
            "[DONE]",
        ]);
        let server = openai_sse_server(&body).await;
        let provider = openai_provider(&server, "gpt-4o");

        let events = collect(&provider, request("gpt-4o", vec![])).await;

        assert_eq!(delta_texts(&events), vec!["He", "llo", "!"]);
        assert_eq!(error_count(&events), 0, "unexpected error: {events:?}");
        assert_eq!(events.len(), 4, "unexpected events: {events:?}");
        assert_last_is_finished(&events);

        let (usage, model, finish_reason) = finished_event(&events);
        assert_eq!(
            *usage,
            Some(LlmUsage {
                prompt_tokens: 11,
                completion_tokens: 7,
                total_tokens: 18,
            })
        );
        assert_eq!(model.as_deref(), Some("gpt-4o"));
        assert!(matches!(finish_reason, Some(FinishReason::Stop)));
    }

    #[tokio::test]
    async fn openai_stream_request_contains_include_usage() {
        let body = sse(&[
            r#"{"choices":[{"delta":{"content":"hi"}}]}"#,
            r#"{"choices":[{"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":1,"completion_tokens":1,"total_tokens":2}}"#,
            "[DONE]",
        ]);
        let server = openai_sse_server(&body).await;
        let provider = openai_provider(&server, "gpt-4o");

        let events = collect(&provider, request("gpt-4o", vec![])).await;
        assert_eq!(error_count(&events), 0, "unexpected error: {events:?}");

        let requests = server
            .received_requests()
            .await
            .expect("request recording is enabled by default");
        assert_eq!(requests.len(), 1, "expected exactly one request");
        let raw = std::str::from_utf8(&requests[0].body).expect("utf-8 request body");
        assert!(
            raw.contains(r#""stream_options":{"include_usage":true}"#),
            "request body must request streaming usage: {raw}"
        );

        let json: serde_json::Value =
            serde_json::from_slice(&requests[0].body).expect("json request body");
        assert_eq!(json["stream_options"]["include_usage"], true);
    }

    #[tokio::test]
    async fn openai_reasoning_chunk_produces_no_delta() {
        let body = sse(&[
            r#"{"choices":[{"delta":{"content":"a"}}]}"#,
            r#"{"choices":[{"delta":{"reasoning_content":"internal monologue"}}]}"#,
            r#"{"choices":[{"delta":{"content":"b"}}]}"#,
            r#"{"choices":[{"delta":{},"finish_reason":"stop"}]}"#,
            "[DONE]",
        ]);
        let server = openai_sse_server(&body).await;
        let provider = openai_provider(&server, "gpt-4o");

        let events = collect(&provider, request("gpt-4o", vec![])).await;

        assert_eq!(delta_texts(&events), vec!["a", "b"]);
        assert_eq!(error_count(&events), 0, "unexpected error: {events:?}");
        assert_last_is_finished(&events);
        finished_event(&events);
    }

    #[tokio::test]
    async fn openai_missing_completion_usage_is_none() {
        let body = sse(&[
            r#"{"choices":[{"delta":{"content":"hi"}}]}"#,
            r#"{"choices":[{"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":11}}"#,
            "[DONE]",
        ]);
        let server = openai_sse_server(&body).await;
        let provider = openai_provider(&server, "gpt-4o");

        let events = collect(&provider, request("gpt-4o", vec![])).await;

        assert_eq!(delta_texts(&events), vec!["hi"]);
        assert_eq!(error_count(&events), 0, "unexpected error: {events:?}");
        let (usage, _, _) = finished_event(&events);
        assert_eq!(*usage, None, "incomplete usage must not be reported");
    }

    #[tokio::test]
    async fn openai_tool_call_chunked_args_emitted_once_complete() {
        let body = sse(&[
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","type":"function","function":{"name":"get_weather","arguments":"{\"loc"}}]}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"ation\":\"Paris\"}"}}]}}]}"#,
            r#"{"choices":[{"delta":{},"finish_reason":"tool_calls"}]}"#,
            "[DONE]",
        ]);
        let server = openai_sse_server(&body).await;
        let provider = openai_provider(&server, "gpt-4o");

        let events = collect(&provider, request("gpt-4o", vec![tool("get_weather")])).await;

        assert_eq!(error_count(&events), 0, "unexpected error: {events:?}");
        let calls = tool_calls(&events);
        assert_eq!(
            calls.len(),
            1,
            "expected one complete tool call: {events:?}"
        );
        assert_eq!(calls[0].0, "call_1");
        assert_eq!(calls[0].1, "get_weather");
        assert_eq!(calls[0].2, r#"{"location":"Paris"}"#);
        assert_last_is_finished(&events);

        let (_, _, finish_reason) = finished_event(&events);
        assert!(matches!(finish_reason, Some(FinishReason::ToolCall)));
    }

    #[tokio::test]
    async fn openai_two_parallel_tool_calls_two_events() {
        let body = sse(&[
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","type":"function","function":{"name":"get_weather","arguments":"{\"loc"}}]}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":1,"id":"call_2","type":"function","function":{"name":"get_time","arguments":"{\"zone"}}]}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"ation\":\"Paris\"}"}}]}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":1,"function":{"arguments":"\":\"UTC\"}"}}]}}]}"#,
            r#"{"choices":[{"delta":{},"finish_reason":"tool_calls"}]}"#,
            "[DONE]",
        ]);
        let server = openai_sse_server(&body).await;
        let provider = openai_provider(&server, "gpt-4o");
        let tools = vec![tool("get_weather"), tool("get_time")];

        let events = collect(&provider, request("gpt-4o", tools)).await;

        assert_eq!(error_count(&events), 0, "unexpected error: {events:?}");
        let calls = tool_calls(&events);
        assert_eq!(
            calls.len(),
            2,
            "expected one event per tool call id: {events:?}"
        );
        assert_eq!(calls[0].0, "call_1");
        assert_eq!(calls[0].1, "get_weather");
        assert_eq!(calls[0].2, r#"{"location":"Paris"}"#);
        assert_eq!(calls[1].0, "call_2");
        assert_eq!(calls[1].1, "get_time");
        assert_eq!(calls[1].2, r#"{"zone":"UTC"}"#);
        assert_last_is_finished(&events);

        let (_, _, finish_reason) = finished_event(&events);
        assert!(matches!(finish_reason, Some(FinishReason::ToolCall)));
    }

    /// Pins the known genai general-path limitation: when a single SSE delta
    /// packs two tool calls and carries no `finish_reason`, genai processes
    /// only `delta_tool_calls.first()`. The finish reason arrives in a later
    /// chunk, so the finish-reason path (which iterates every tool call) is
    /// bypassed and exactly one tool call reaches Camel.
    #[tokio::test]
    async fn openai_two_calls_in_one_delta_only_first_pinned() {
        let body = sse(&[
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","type":"function","function":{"name":"get_weather","arguments":"{\"location\":\"Paris\"}"}},{"index":1,"id":"call_2","type":"function","function":{"name":"get_time","arguments":"{\"zone\":\"UTC\"}"}}]}}]}"#,
            r#"{"choices":[{"delta":{},"finish_reason":"tool_calls"}]}"#,
            "[DONE]",
        ]);
        let server = openai_sse_server(&body).await;
        let provider = openai_provider(&server, "gpt-4o");
        let tools = vec![tool("get_weather"), tool("get_time")];

        let events = collect(&provider, request("gpt-4o", tools)).await;

        assert_eq!(error_count(&events), 0, "unexpected error: {events:?}");
        let calls = tool_calls(&events);
        assert_eq!(
            calls.len(),
            1,
            "genai general path keeps only the first tool call: {events:?}"
        );
        assert_eq!(calls[0].0, "call_1");
        assert_eq!(calls[0].1, "get_weather");
        assert_eq!(calls[0].2, r#"{"location":"Paris"}"#);
        assert_last_is_finished(&events);
    }

    #[tokio::test]
    async fn openai_mid_stream_error_yields_err_no_finished() {
        let body = sse(&[
            r#"{"choices":[{"delta":{"content":"hi"}}]}"#,
            r#"{"choices":[{"delta":"#,
            "[DONE]",
        ]);
        let server = openai_sse_server(&body).await;
        let provider = openai_provider(&server, "gpt-4o");

        let events = collect(&provider, request("gpt-4o", vec![])).await;

        assert_eq!(events.len(), 2, "unexpected events: {events:?}");
        assert!(matches!(&events[0], Ok(ChatEvent::Delta { text }) if text == "hi"));
        assert!(
            matches!(&events[1], Err(LlmError::Protocol(_))),
            "expected a protocol error, got: {:?}",
            events[1]
        );
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, Ok(ChatEvent::Finished { .. }))),
            "a mid-stream error must not emit Finished: {events:?}"
        );
    }

    #[tokio::test]
    async fn openai_config_ollama_style_model_hits_openai_route() {
        let body = sse(&[
            r#"{"choices":[{"delta":{"content":"ok"}}]}"#,
            r#"{"choices":[{"delta":{},"finish_reason":"stop"}]}"#,
            "[DONE]",
        ]);
        // The mock is mounted ONLY on the OpenAI route: a name-inferred Ollama
        // adapter would request `/api/chat` and get a 404 instead.
        let server = openai_sse_server(&body).await;
        let provider = openai_provider(&server, "qwen3.5:4b");

        let events = collect(&provider, request("qwen3.5:4b", vec![])).await;

        assert_eq!(delta_texts(&events), vec!["ok"]);
        assert_eq!(error_count(&events), 0, "unexpected error: {events:?}");
        assert_last_is_finished(&events);

        let requests = server
            .received_requests()
            .await
            .expect("request recording is enabled by default");
        assert_eq!(requests.len(), 1, "expected exactly one request");
        assert_eq!(requests[0].url.path(), "/v1/chat/completions");
    }

    /// The real OpenAI `include_usage` convention puts usage in a final chunk
    /// with an empty `choices` array. That chunk must still feed `Finished`.
    #[tokio::test]
    async fn openai_usage_chunk_with_empty_choices_still_yields_usage() {
        let body = sse(&[
            r#"{"choices":[{"delta":{"content":"hi"}}]}"#,
            r#"{"choices":[{"delta":{},"finish_reason":"stop"}]}"#,
            r#"{"choices":[],"usage":{"prompt_tokens":4,"completion_tokens":2,"total_tokens":6}}"#,
            "[DONE]",
        ]);
        let server = openai_sse_server(&body).await;
        let provider = openai_provider(&server, "gpt-4o");

        let events = collect(&provider, request("gpt-4o", vec![])).await;

        assert_eq!(delta_texts(&events), vec!["hi"]);
        assert_eq!(error_count(&events), 0, "unexpected error: {events:?}");
        assert_last_is_finished(&events);

        let (usage, _, _) = finished_event(&events);
        assert_eq!(
            *usage,
            Some(LlmUsage {
                prompt_tokens: 4,
                completion_tokens: 2,
                total_tokens: 6,
            }),
            "usage-only final chunk must still populate Finished.usage"
        );
    }
}

/// End-to-end acceptance tests for the genai-backed embedding path, driven by
/// a local `wiremock` server. The mock answers in request order with vectors
/// carrying per-input identity markers, proving the adapter neither drops nor
/// reorders rows. OpenAI hits `/v1/embeddings` through the explicit OpenAI
/// config; Ollama hits `/api/embed` through the bare `server.uri()` base URL,
/// which exercises the trailing-slash branch left uncovered by the chat suite.
#[cfg(any(feature = "openai", feature = "ollama", feature = "all-providers"))]
mod wiremock_embed {
    use std::sync::Arc;

    use camel_api::SsrfPolicy;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::super::{BuildOverride, GenaiConfig, GenaiProvider};
    use crate::LlmHttpPolicy;
    use crate::provider::{EmbedRequest, LlmProvider, LlmUsage};

    /// Reuse the chat suite's pattern: bypass ambient proxies for `127.0.0.1`.
    fn build_override() -> BuildOverride {
        Arc::new(|| {
            Box::pin(async {
                let client = reqwest::Client::builder()
                    .no_proxy()
                    .build()
                    .map_err(|err| {
                        crate::error::LlmError::Network(format!("test reqwest client: {err}"))
                    })?;
                Ok(genai::Client::builder().with_reqwest(client).build())
            })
        })
    }

    #[cfg(any(feature = "openai", feature = "all-providers"))]
    fn openai_provider(server: &MockServer, default_model: &str) -> GenaiProvider {
        GenaiProvider::new_with_build_override(
            "wiremock-openai-embed",
            default_model,
            GenaiConfig::OpenAi {
                api_key: "sk-test".into(),
                base_url: Some(format!("{}/v1", server.uri())),
            },
            LlmHttpPolicy::chat(),
            build_override(),
            SsrfPolicy::AllowInternal,
        )
    }

    #[cfg(any(feature = "ollama", feature = "all-providers"))]
    fn ollama_provider(server: &MockServer, default_model: &str) -> GenaiProvider {
        GenaiProvider::new_with_build_override(
            "wiremock-ollama-embed",
            default_model,
            GenaiConfig::Ollama {
                base_url: server.uri(),
            },
            LlmHttpPolicy::chat(),
            build_override(),
            SsrfPolicy::AllowInternal,
        )
    }

    /// Per-input identity markers: `input i` must surface at output slot `i`.
    fn assert_markers_in_request_order(embeddings: &[Vec<f32>]) {
        assert_eq!(embeddings.len(), 3, "expected one vector per input");
        for (i, vector) in embeddings.iter().enumerate() {
            assert_eq!(
                vector,
                &vec![1.0, i as f32, 0.5],
                "vector {i} lost its request-order identity marker"
            );
        }
    }

    #[cfg(any(feature = "openai", feature = "all-providers"))]
    fn openai_body() -> serde_json::Value {
        serde_json::json!({
            "object": "list",
            "data": [
                {"object": "embedding", "index": 0, "embedding": [1.0, 0.0, 0.5]},
                {"object": "embedding", "index": 1, "embedding": [1.0, 1.0, 0.5]},
                {"object": "embedding", "index": 2, "embedding": [1.0, 2.0, 0.5]}
            ],
            "model": "embeddinggemma",
            "usage": {"prompt_tokens": 3, "total_tokens": 3}
        })
    }

    #[cfg(any(feature = "openai", feature = "all-providers"))]
    async fn openai_embed_server() -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/embeddings"))
            .respond_with(ResponseTemplate::new(200).set_body_json(openai_body()))
            .mount(&server)
            .await;
        server
    }

    #[cfg(any(feature = "openai", feature = "all-providers"))]
    #[tokio::test]
    async fn embed_openai_vectors_match_request_order() {
        let server = openai_embed_server().await;
        let provider = openai_provider(&server, "embeddinggemma");

        let response = provider
            .embed(EmbedRequest::new(
                "",
                vec!["alpha".into(), "beta".into(), "gamma".into()],
            ))
            .await
            .expect("embed must succeed");

        assert_markers_in_request_order(&response.embeddings);
        assert_eq!(
            response.usage,
            Some(LlmUsage {
                prompt_tokens: 3,
                completion_tokens: 0,
                total_tokens: 3,
            })
        );
    }

    #[cfg(any(feature = "ollama", feature = "all-providers"))]
    #[tokio::test]
    async fn embed_ollama_vectors_match_request_order() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/embed"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "model": "embeddinggemma",
                "embeddings": [
                    [1.0, 0.0, 0.5],
                    [1.0, 1.0, 0.5],
                    [1.0, 2.0, 0.5]
                ],
                "prompt_eval_count": 9
            })))
            .mount(&server)
            .await;
        let provider = ollama_provider(&server, "embeddinggemma");

        let response = provider
            .embed(EmbedRequest::new(
                "",
                vec!["alpha".into(), "beta".into(), "gamma".into()],
            ))
            .await
            .expect("embed must succeed");

        assert_markers_in_request_order(&response.embeddings);
        assert_eq!(
            response.usage,
            Some(LlmUsage {
                prompt_tokens: 9,
                completion_tokens: 0,
                total_tokens: 9,
            }),
            "Ollama embeds carry no eval_count, so total must default to prompt"
        );
    }

    #[cfg(any(feature = "openai", feature = "all-providers"))]
    #[tokio::test]
    async fn embed_request_body_carries_model() {
        // Default applied when the request carries no explicit model.
        let server = openai_embed_server().await;
        let provider = openai_provider(&server, "embeddinggemma");

        let response = provider
            .embed(EmbedRequest::new("", vec!["alpha".into()]))
            .await
            .expect("embed must succeed");
        assert_eq!(response.model, "embeddinggemma");

        let requests = server
            .received_requests()
            .await
            .expect("request recording is enabled by default");
        assert_eq!(requests.len(), 1, "expected exactly one request");
        let body: serde_json::Value =
            serde_json::from_slice(&requests[0].body).expect("json request body");
        assert_eq!(body["model"], "embeddinggemma");
        assert_eq!(body["input"], serde_json::json!(["alpha"]));

        // Explicit per-request model wins over the provider default.
        let server = openai_embed_server().await;
        let provider = openai_provider(&server, "embeddinggemma");

        let response = provider
            .embed(EmbedRequest::new("text-embed-9", vec!["beta".into()]))
            .await
            .expect("embed must succeed");
        assert_eq!(response.model, "text-embed-9");

        let requests = server
            .received_requests()
            .await
            .expect("request recording is enabled by default");
        assert_eq!(requests.len(), 1, "expected exactly one request");
        let body: serde_json::Value =
            serde_json::from_slice(&requests[0].body).expect("json request body");
        assert_eq!(body["model"], "text-embed-9");
    }
}

/// End-to-end acceptance tests for the genai-backed Ollama chat stream. The
/// provider base URL is the bare `server.uri()` (no trailing slash), so the
/// target normalisation is exercised on the way to `/api/chat`, and the body
/// is the NDJSON shape Ollama emits.
#[cfg(any(feature = "ollama", feature = "all-providers"))]
mod wiremock_ollama {
    use std::sync::Arc;

    use camel_api::SsrfPolicy;
    use futures::StreamExt;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::super::{BuildOverride, GenaiConfig, GenaiProvider};
    use crate::LlmHttpPolicy;
    use crate::error::LlmError;
    use crate::provider::{
        ChatEvent, ChatMessage, ChatRequest, FinishReason, LlmProvider, LlmUsage,
    };

    fn build_override() -> BuildOverride {
        Arc::new(|| {
            Box::pin(async {
                let client = reqwest::Client::builder()
                    .no_proxy()
                    .build()
                    .map_err(|err| LlmError::Network(format!("test reqwest client: {err}")))?;
                Ok(genai::Client::builder().with_reqwest(client).build())
            })
        })
    }

    fn ollama_provider(server: &MockServer, default_model: &str) -> GenaiProvider {
        GenaiProvider::new_with_build_override(
            "wiremock-ollama",
            default_model,
            GenaiConfig::Ollama {
                base_url: server.uri(),
            },
            LlmHttpPolicy::chat(),
            build_override(),
            SsrfPolicy::AllowInternal,
        )
    }

    /// One JSON object per line, as Ollama's `application/x-ndjson` stream.
    fn ndjson(lines: &[&str]) -> String {
        lines.iter().map(|line| format!("{line}\n")).collect()
    }

    async fn ollama_chat_server(body: &str) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/chat"))
            .respond_with(
                ResponseTemplate::new(200).set_body_raw(body.to_string(), "application/x-ndjson"),
            )
            .mount(&server)
            .await;
        server
    }

    fn request() -> ChatRequest {
        ChatRequest::new("llama3", vec![ChatMessage::user("hello")])
    }

    fn delta_texts(events: &[Result<ChatEvent, LlmError>]) -> Vec<String> {
        events
            .iter()
            .filter_map(|event| match event {
                Ok(ChatEvent::Delta { text }) => Some(text.clone()),
                _ => None,
            })
            .collect()
    }

    /// Return the fields of the single `Finished` event, asserting there is
    /// exactly one.
    fn finished_event(
        events: &[Result<ChatEvent, LlmError>],
    ) -> (&Option<LlmUsage>, &Option<FinishReason>) {
        let mut found = events.iter().filter_map(|event| match event {
            Ok(ChatEvent::Finished {
                usage,
                finish_reason,
                ..
            }) => Some((usage, finish_reason)),
            _ => None,
        });
        let finished = found.next().expect("expected exactly one Finished event");
        assert!(
            found.next().is_none(),
            "expected exactly one Finished event"
        );
        finished
    }

    #[tokio::test]
    async fn ollama_chat_ndjson_usage_from_eval_counts() {
        let body = ndjson(&[
            r#"{"message":{"role":"assistant","content":"hi"},"done":false}"#,
            r#"{"message":{"role":"assistant","content":""},"done":true,"done_reason":"stop","prompt_eval_count":9,"eval_count":5}"#,
        ]);
        let server = ollama_chat_server(&body).await;
        let provider = ollama_provider(&server, "llama3");

        let events: Vec<_> = provider.chat_stream(request()).collect().await;

        assert_eq!(delta_texts(&events), vec!["hi"]);
        assert_eq!(
            events.iter().filter(|event| event.is_err()).count(),
            0,
            "unexpected error: {events:?}"
        );
        assert!(matches!(
            events.last(),
            Some(Ok(ChatEvent::Finished { .. }))
        ));

        let (usage, finish_reason) = finished_event(&events);
        assert_eq!(
            *usage,
            Some(LlmUsage {
                prompt_tokens: 9,
                completion_tokens: 5,
                total_tokens: 14,
            })
        );
        assert!(matches!(finish_reason, Some(FinishReason::Stop)));
    }

    #[tokio::test]
    async fn ollama_chat_missing_eval_count_usage_behaviour() {
        // Same done-line shape, but without `eval_count`: the all-or-nothing
        // chat usage rule must surface `None` rather than a partial usage.
        let body = ndjson(&[
            r#"{"message":{"role":"assistant","content":"hi"},"done":false}"#,
            r#"{"message":{"role":"assistant","content":""},"done":true,"done_reason":"stop","prompt_eval_count":9}"#,
        ]);
        let server = ollama_chat_server(&body).await;
        let provider = ollama_provider(&server, "llama3");

        let events: Vec<_> = provider.chat_stream(request()).collect().await;

        assert_eq!(delta_texts(&events), vec!["hi"]);
        assert_eq!(
            events.iter().filter(|event| event.is_err()).count(),
            0,
            "unexpected error: {events:?}"
        );
        let (usage, _) = finished_event(&events);
        assert_eq!(*usage, None, "partial chat usage must not be reported");
    }
}

/// End-to-end acceptance tests for HTTP status, transport and timeout errors
/// raised by the real OpenAI adapter against a local `wiremock` server.
///
/// These prove the taxonomy mapping survives the full genai request path
/// (including `Retry-After` header parsing) rather than only the pure
/// `map_genai_error` unit surface.
#[cfg(any(feature = "openai", feature = "all-providers"))]
mod wiremock_error {
    use std::net::TcpListener;
    use std::sync::Arc;
    use std::time::Duration;

    use camel_api::SsrfPolicy;
    use futures::StreamExt;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::super::{BuildOverride, GenaiConfig, GenaiProvider};
    use crate::LlmHttpPolicy;
    use crate::error::{LlmError, is_retryable};
    use crate::provider::{ChatMessage, ChatRequest, EmbedRequest, LlmProvider};

    const CHAT_PATH: &str = "/v1/chat/completions";
    const EMBED_PATH: &str = "/v1/embeddings";

    /// Bypass ambient proxies so the loopback mock server is reached directly.
    fn build_override() -> BuildOverride {
        Arc::new(|| {
            Box::pin(async {
                let client = reqwest::Client::builder()
                    .no_proxy()
                    .build()
                    .map_err(|err| LlmError::Network(format!("test reqwest client: {err}")))?;
                Ok(genai::Client::builder().with_reqwest(client).build())
            })
        })
    }

    fn openai_provider(
        base_url: String,
        transport_policy: LlmHttpPolicy,
        override_: BuildOverride,
    ) -> GenaiProvider {
        GenaiProvider::new_with_build_override(
            "wiremock-openai-errors",
            "gpt-4o",
            GenaiConfig::OpenAi {
                api_key: "sk-test".into(),
                base_url: Some(base_url),
            },
            transport_policy,
            override_,
            SsrfPolicy::AllowInternal,
        )
    }

    fn provider_for(server: &MockServer) -> GenaiProvider {
        openai_provider(
            format!("{}/v1", server.uri()),
            LlmHttpPolicy::chat(),
            build_override(),
        )
    }

    async fn mock_server(endpoint: &str, response: ResponseTemplate) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(endpoint))
            .respond_with(response)
            .mount(&server)
            .await;
        server
    }

    fn request() -> ChatRequest {
        ChatRequest::new("gpt-4o", vec![ChatMessage::user("hello")])
    }

    /// Drain the chat stream and return its single mapped error.
    pub(super) async fn chat_error(provider: &GenaiProvider) -> LlmError {
        let events: Vec<_> = provider.chat_stream(request()).collect().await;
        let mut errors = events.into_iter().filter_map(Result::err);
        let err = errors.next().expect("stream must yield an error");
        assert!(errors.next().is_none(), "expected exactly one error");
        err
    }

    async fn embed_error(provider: &GenaiProvider) -> LlmError {
        provider
            .embed(EmbedRequest::new("", vec!["hello".into()]))
            .await
            .expect_err("embed must fail")
    }

    fn rate_limit_body() -> serde_json::Value {
        serde_json::json!({"error": {"message": "rate limit"}})
    }

    fn quota_body() -> serde_json::Value {
        serde_json::json!({"error": {"code": "insufficient_quota", "message": "billing"}})
    }

    #[tokio::test]
    async fn chat_429_rate_limit_retryable_no_retry_after() {
        let server = mock_server(
            CHAT_PATH,
            ResponseTemplate::new(429).set_body_json(rate_limit_body()),
        )
        .await;
        let provider = provider_for(&server);

        let err = chat_error(&provider).await;

        assert!(
            matches!(err, LlmError::RateLimit { retry_after: None }),
            "expected RateLimit without retry_after, got: {err:?}"
        );
        assert!(is_retryable(&err), "rate limits must be retryable");
    }

    #[tokio::test]
    async fn embed_429_honours_delta_seconds_retry_after() {
        let server = mock_server(
            EMBED_PATH,
            ResponseTemplate::new(429)
                .insert_header("Retry-After", "7")
                .set_body_json(rate_limit_body()),
        )
        .await;
        let provider = provider_for(&server);

        let err = embed_error(&provider).await;

        assert!(
            matches!(
                err,
                LlmError::RateLimit {
                    retry_after: Some(after)
                } if after == Duration::from_secs(7)
            ),
            "expected RateLimit with 7s retry_after, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn embed_429_http_date_retry_after_maps_to_none() {
        let server = mock_server(
            EMBED_PATH,
            ResponseTemplate::new(429)
                .insert_header("Retry-After", "Wed, 21 Oct 2026 07:28:00 GMT")
                .set_body_json(rate_limit_body()),
        )
        .await;
        let provider = provider_for(&server);

        let err = embed_error(&provider).await;

        assert!(
            matches!(err, LlmError::RateLimit { retry_after: None }),
            "HTTP-date Retry-After must map to None, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn chat_429_insufficient_quota_not_retryable() {
        let server = mock_server(
            CHAT_PATH,
            ResponseTemplate::new(429).set_body_json(quota_body()),
        )
        .await;
        let provider = provider_for(&server);

        let err = chat_error(&provider).await;

        assert!(
            matches!(err, LlmError::QuotaExceeded { .. }),
            "expected QuotaExceeded, got: {err:?}"
        );
        assert!(
            !is_retryable(&err),
            "insufficient quota must not be retried"
        );
    }

    #[tokio::test]
    async fn embed_429_insufficient_quota_not_retryable() {
        let server = mock_server(
            EMBED_PATH,
            ResponseTemplate::new(429).set_body_json(quota_body()),
        )
        .await;
        let provider = provider_for(&server);

        let err = embed_error(&provider).await;

        assert!(
            matches!(err, LlmError::QuotaExceeded { .. }),
            "expected QuotaExceeded on the embed path, got: {err:?}"
        );
        assert!(
            !is_retryable(&err),
            "insufficient quota must not be retried"
        );
    }

    #[tokio::test]
    async fn chat_401_maps_auth_failed() {
        let server = mock_server(
            CHAT_PATH,
            ResponseTemplate::new(401)
                .set_body_json(serde_json::json!({"error": {"message": "invalid api key"}})),
        )
        .await;
        let provider = provider_for(&server);

        let err = chat_error(&provider).await;

        assert!(
            matches!(err, LlmError::AuthFailed { .. }),
            "expected AuthFailed, got: {err:?}"
        );
        assert!(!is_retryable(&err), "auth failures must not be retried");
    }

    #[tokio::test]
    async fn chat_503_maps_provider_unavailable_retryable() {
        let server = mock_server(
            CHAT_PATH,
            ResponseTemplate::new(503).set_body_json(serde_json::json!({"error": "unavailable"})),
        )
        .await;
        let provider = provider_for(&server);

        let err = chat_error(&provider).await;

        assert!(
            matches!(err, LlmError::ProviderUnavailable(_)),
            "expected ProviderUnavailable, got: {err:?}"
        );
        assert!(
            is_retryable(&err),
            "server-side transient errors must be retryable"
        );
    }

    #[tokio::test]
    async fn chat_connection_refused_maps_network_retryable() {
        // Bind then drop a loopback listener so the port is closed and the
        // connection is refused deterministically, without external network.
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind closed port");
        let addr = listener.local_addr().expect("closed port address");
        drop(listener);

        let provider = openai_provider(
            format!("http://{addr}/v1"),
            LlmHttpPolicy::chat(),
            build_override(),
        );

        let err = chat_error(&provider).await;

        assert!(
            matches!(err, LlmError::Network(_)),
            "expected Network, got: {err:?}"
        );
        assert!(is_retryable(&err), "connection failures must be retryable");
    }
}

/// Native generic-service targets, factory construction, embedding rejection,
/// and URL-policy/pinning behavior (llm-provider-extensibility 1.2).
#[cfg(any(feature = "generic-providers", feature = "all-providers"))]
mod generic_native {
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use camel_api::SsrfPolicy;
    use genai::ModelSpec;
    use genai::adapter::AdapterKind;
    use genai::resolver::AuthData;

    use super::super::{
        BuildOverride, GenaiConfig, GenaiProvider, collect_pinning_from_config, resolve_target,
    };
    use crate::LlmHttpPolicy;
    use crate::config::{
        GenericProviderConfig, LlmGlobalConfig, LlmProviderConfig, LlmServiceKind,
    };
    use crate::error::LlmError;
    use crate::provider::{ChatMessage, ChatRequest, EmbedRequest, LlmProvider};
    use crate::provider_factory::build_provider_map;

    fn generic_config(
        service: LlmServiceKind,
        api_key: &str,
        default_model: &str,
        base_url: Option<&str>,
        timeout_secs: Option<u64>,
    ) -> GenericProviderConfig {
        GenericProviderConfig {
            service,
            api_key: api_key.into(),
            default_model: default_model.into(),
            base_url: base_url.map(str::to_string),
            timeout_secs,
            max_concurrency: None,
            network_retry: None,
            pricing: None,
            cache_ttl_secs: None,
            cache_max_entries: None,
        }
    }

    fn global_with(
        name: &str,
        config: GenericProviderConfig,
        allow_internal: bool,
    ) -> LlmGlobalConfig {
        let mut providers = HashMap::new();
        providers.insert(name.into(), LlmProviderConfig::Generic(config));
        LlmGlobalConfig {
            providers,
            allow_internal,
            ..Default::default()
        }
    }

    #[test]
    fn generic_targets_are_explicit_and_normalized() {
        for (service, kind, default_url) in [
            (
                LlmServiceKind::Anthropic,
                AdapterKind::Anthropic,
                "https://api.anthropic.com/v1/",
            ),
            (
                LlmServiceKind::Gemini,
                AdapterKind::Gemini,
                "https://generativelanguage.googleapis.com/v1beta/",
            ),
        ] {
            let default_config = GenaiConfig::Generic {
                service,
                api_key: "explicit-key".into(),
                base_url: None,
            };
            assert_eq!(default_config.adapter_kind(), kind);
            assert_eq!(default_config.endpoint(), default_url);

            let override_config = GenaiConfig::Generic {
                service,
                api_key: "explicit-key".into(),
                base_url: Some("https://example.com/native".into()),
            };
            assert_eq!(override_config.adapter_kind(), kind);
            assert_eq!(override_config.endpoint(), "https://example.com/native/");

            let spec = resolve_target(
                override_config.adapter_kind(),
                &override_config.endpoint(),
                override_config.api_key(),
                "qwen3.5:4b",
            );

            match spec {
                ModelSpec::Target(target) => {
                    assert_eq!(target.model.adapter_kind, kind);
                    assert_eq!(target.model.model_name.as_str(), "qwen3.5:4b");
                    assert_eq!(target.endpoint.base_url(), "https://example.com/native/");
                    assert!(
                        matches!(&target.auth, AuthData::Key(key) if !key.is_empty()),
                        "generic targeting must carry explicit nonempty auth"
                    );
                }
                other => panic!("expected ModelSpec::Target, got {other:?}"),
            }
        }
    }

    #[test]
    fn generic_factory_builds_chat_only_providers() {
        let mut providers = HashMap::new();
        providers.insert(
            "anthropic-native".into(),
            LlmProviderConfig::Generic(generic_config(
                LlmServiceKind::Anthropic,
                "anthropic-key",
                "claude-test",
                None,
                Some(7),
            )),
        );
        providers.insert(
            "gemini-native".into(),
            LlmProviderConfig::Generic(generic_config(
                LlmServiceKind::Gemini,
                "gemini-key",
                "gemini-test",
                None,
                Some(7),
            )),
        );
        let config = LlmGlobalConfig {
            providers,
            ..Default::default()
        };

        let map = build_provider_map(&config).expect("lazy generic construction must succeed");
        assert_eq!(map.len(), 2);

        let anthropic = map.get("anthropic-native").expect("anthropic provider");
        assert_eq!(anthropic.id(), "anthropic-native");
        assert_eq!(anthropic.default_model(), "claude-test");
        assert!(!anthropic.supports_embed());

        let gemini = map.get("gemini-native").expect("gemini provider");
        assert_eq!(gemini.id(), "gemini-native");
        assert_eq!(gemini.default_model(), "gemini-test");
        assert!(!gemini.supports_embed());

        // Chat is the supported operation; constructing the lazy stream must
        // not require network I/O.
        let _chat = gemini.chat_stream(ChatRequest::new("", vec![ChatMessage::user("hello")]));
    }

    #[tokio::test]
    async fn generic_embed_rejects_before_client_initialization() {
        for service in [LlmServiceKind::Anthropic, LlmServiceKind::Gemini] {
            let builds = Arc::new(AtomicUsize::new(0));
            let builds_in_override = Arc::clone(&builds);
            let override_: BuildOverride = Arc::new(move || {
                builds_in_override.fetch_add(1, Ordering::SeqCst);
                Box::pin(async {
                    Err(LlmError::Network(
                        "embed must not initialize a generic client".into(),
                    ))
                })
            });
            let provider = GenaiProvider::new_with_build_override(
                "generic-native",
                "explicit-model",
                GenaiConfig::Generic {
                    service,
                    api_key: "explicit-key".into(),
                    base_url: None,
                },
                LlmHttpPolicy::chat(),
                override_,
                SsrfPolicy::AllowInternal,
            );

            assert!(
                !provider.supports_embed(),
                "generic services are chat-only for {service:?}"
            );

            let err = provider
                .embed(EmbedRequest::new("", vec!["hello".into()]))
                .await
                .expect_err("generic embed must be rejected");
            assert!(
                matches!(&err, LlmError::UnsupportedCapability(_)),
                "expected UnsupportedCapability, got: {err}"
            );
            assert_eq!(
                builds.load(Ordering::SeqCst),
                0,
                "embed must reject before client initialization for {service:?}"
            );
        }
    }

    #[tokio::test]
    async fn generic_url_policy_and_pinning() {
        for service in [LlmServiceKind::Anthropic, LlmServiceKind::Gemini] {
            for url in [
                "https://127.0.0.1/v1",
                "https://169.254.169.254/v1",
                "file:///tmp/llm",
            ] {
                let config = global_with(
                    "generic-native",
                    generic_config(service, "explicit-key", "explicit-model", Some(url), None),
                    false,
                );
                // bd rc-ga0kl: boot no longer resolves DNS, so the two
                // HTTPS IP literals BOOT fine now — their SSRF gating
                // moved to first use (asserted below through the same
                // pinned check the client build runs). The `file:` scheme
                // is still rejected at boot: the shape check is DNS-free.
                if url.starts_with("https://") {
                    let map = build_provider_map(&config)
                        .expect("boot must be lazy: no DNS resolution at construction");
                    assert_eq!(map.len(), 1, "provider registered for {url}");
                } else {
                    let err = match build_provider_map(&config) {
                        Ok(_) => panic!("non-http scheme must still be rejected at boot"),
                        Err(err) => err,
                    };
                    assert!(
                        matches!(&err, LlmError::InvalidRequest(_)),
                        "expected InvalidRequest for {service:?} {url}, got: {err}"
                    );
                }
            }

            // First-use gate (bd rc-ga0kl): the pinned check the client
            // build runs rejects blocked IPs fail-closed — the same gate
            // boot used to perform inline.
            for blocked in ["https://127.0.0.1/v1", "https://169.254.169.254/v1"] {
                let first_use = GenaiConfig::Generic {
                    service,
                    api_key: "explicit-key".into(),
                    base_url: Some(blocked.to_string()),
                };
                let err = match collect_pinning_from_config(&first_use, SsrfPolicy::PublicHttpsOnly)
                    .await
                {
                    Err(err) => err,
                    Ok(pinning) => {
                        panic!("blocked SSRF address must fail closed at first use: {pinning:?}")
                    }
                };
                assert!(
                    err.to_string().contains("blocked SSRF address"),
                    "expected the typed SSRF error for {blocked}, got: {err}"
                );
            }

            let ip_config = GenaiConfig::Generic {
                service,
                api_key: "explicit-key".into(),
                base_url: Some("http://127.0.0.1:12345/native".into()),
            };
            assert!(
                collect_pinning_from_config(&ip_config, SsrfPolicy::AllowInternal)
                    .await
                    .expect("IP literal validation")
                    .is_none(),
                "IP literals need no DNS pinning"
            );
            let ip_global = global_with(
                "generic-native",
                generic_config(
                    service,
                    "explicit-key",
                    "explicit-model",
                    Some("http://127.0.0.1:12345/native"),
                    None,
                ),
                true,
            );
            assert_eq!(
                build_provider_map(&ip_global)
                    .expect("lazy internal factory construction must succeed")
                    .len(),
                1
            );

            let hostname_config = GenaiConfig::Generic {
                service,
                api_key: "explicit-key".into(),
                base_url: Some("http://localhost:12345/native".into()),
            };
            let (host, addrs) =
                collect_pinning_from_config(&hostname_config, SsrfPolicy::AllowInternal)
                    .await
                    .expect("hostname validation")
                    .expect("hostname must be pinned");
            assert_eq!(host, "localhost");
            assert!(!addrs.is_empty(), "loopback resolution must be nonempty");
            assert!(
                addrs.iter().all(|addr| addr.ip().is_loopback()),
                "expected loopback addresses, got: {addrs:?}"
            );
            assert!(
                addrs.iter().all(|addr| addr.port() == 0),
                "resolver sockets must preserve the existing port-0 semantics: {addrs:?}"
            );
        }
    }
}

/// Native wire-protocol fixtures for generic providers (llm-provider-extensibility
/// 1.3). Providers are constructed through the production factory with
/// `allow_internal=true`; the mock server is the only network peer.
#[cfg(any(feature = "generic-providers", feature = "all-providers"))]
mod wiremock_generic {
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::time::Duration;

    use futures::StreamExt;
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use crate::config::{
        GenericProviderConfig, LlmGlobalConfig, LlmProviderConfig, LlmServiceKind,
    };
    use crate::error::{LlmError, is_retryable};
    use crate::provider::{
        ChatEvent, ChatMessage, ChatRequest, FinishReason, LlmProvider, LlmUsage,
    };
    use crate::provider_factory::build_provider_map;

    const TEST_TIMEOUT: Duration = Duration::from_secs(10);

    type Events = Vec<Result<ChatEvent, LlmError>>;

    /// Build a generic provider through the production factory. `allow_internal`
    /// is required because wiremock listens on a loopback HTTP address.
    fn factory_provider(
        name: &str,
        service: LlmServiceKind,
        api_key: &str,
        default_model: &str,
        base_url: String,
    ) -> Arc<dyn LlmProvider> {
        let config = LlmGlobalConfig {
            providers: HashMap::from([(
                name.to_string(),
                LlmProviderConfig::Generic(GenericProviderConfig {
                    service,
                    api_key: api_key.into(),
                    default_model: default_model.into(),
                    base_url: Some(base_url),
                    timeout_secs: None,
                    max_concurrency: None,
                    network_retry: None,
                    pricing: None,
                    cache_ttl_secs: None,
                    cache_max_entries: None,
                }),
            )]),
            allow_internal: true,
            ..Default::default()
        };
        let mut map =
            build_provider_map(&config).expect("lazy generic provider construction must succeed");
        map.remove(name)
            .expect("provider must be present in the map")
    }

    /// Drain a chat stream under a bounded timeout. Never sleeps.
    async fn collect_events(provider: &dyn LlmProvider, request: ChatRequest) -> Events {
        tokio::time::timeout(TEST_TIMEOUT, provider.chat_stream(request).collect())
            .await
            .expect("chat stream must complete within the bounded test timeout")
    }

    fn request() -> ChatRequest {
        ChatRequest::new("", vec![ChatMessage::user("hello")])
    }

    fn delta_texts(events: &[Result<ChatEvent, LlmError>]) -> Vec<String> {
        events
            .iter()
            .filter_map(|event| match event {
                Ok(ChatEvent::Delta { text }) => Some(text.clone()),
                _ => None,
            })
            .collect()
    }

    fn error_count(events: &[Result<ChatEvent, LlmError>]) -> usize {
        events.iter().filter(|event| event.is_err()).count()
    }

    /// Return the single `Finished` event's fields, asserting exactly one exists.
    fn finished_event(
        events: &[Result<ChatEvent, LlmError>],
    ) -> (&Option<LlmUsage>, &Option<String>, &Option<FinishReason>) {
        let mut found = events.iter().filter_map(|event| match event {
            Ok(ChatEvent::Finished {
                usage,
                model,
                finish_reason,
                ..
            }) => Some((usage, model, finish_reason)),
            _ => None,
        });
        let finished = found.next().expect("expected exactly one Finished event");
        assert!(
            found.next().is_none(),
            "expected exactly one Finished event"
        );
        finished
    }

    fn assert_single_finished_last(events: &[Result<ChatEvent, LlmError>]) {
        assert!(
            matches!(events.last(), Some(Ok(ChatEvent::Finished { .. }))),
            "Finished must be the last event, got: {events:?}"
        );
        finished_event(events);
    }

    /// Anthropic SSE frames in the native protocol shape: one `message_start`
    /// with input 3 / output 0, a content block, one `hello` delta, a
    /// `message_delta` carrying `end_turn` and output 2, then `message_stop`.
    fn anthropic_sse() -> String {
        [
            (
                "message_start",
                r#"{"type":"message_start","message":{"id":"msg_test","type":"message","role":"assistant","model":"claude-test","content":[],"stop_reason":null,"stop_sequence":null,"usage":{"input_tokens":3,"output_tokens":0}}}"#,
            ),
            (
                "content_block_start",
                r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
            ),
            (
                "content_block_delta",
                r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"hello"}}"#,
            ),
            (
                "content_block_stop",
                r#"{"type":"content_block_stop","index":0}"#,
            ),
            (
                "message_delta",
                r#"{"type":"message_delta","delta":{"stop_reason":"end_turn","stop_sequence":null},"usage":{"output_tokens":2}}"#,
            ),
            ("message_stop", r#"{"type":"message_stop"}"#),
        ]
        .iter()
        .map(|(event, data)| format!("event: {event}\ndata: {data}\n\n"))
        .collect()
    }

    /// One native Gemini candidates frame carrying `hello`, `STOP`, and usage.
    fn gemini_sse() -> String {
        format!(
            "data: {}\n\n",
            r#"{"candidates":[{"content":{"parts":[{"text":"hello"}],"role":"model"},"finishReason":"STOP","index":0}],"usageMetadata":{"promptTokenCount":3,"candidatesTokenCount":2,"totalTokenCount":5},"modelVersion":"gemini-test"}"#
        )
    }

    async fn anthropic_server() -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(
                ResponseTemplate::new(200).set_body_raw(anthropic_sse(), "text/event-stream"),
            )
            .mount(&server)
            .await;
        server
    }

    async fn received(server: &MockServer) -> Vec<wiremock::Request> {
        server
            .received_requests()
            .await
            .expect("request recording is enabled by default")
    }

    fn header<'a>(request: &'a wiremock::Request, name: &str) -> Option<&'a str> {
        request
            .headers
            .get(name)
            .and_then(|value| value.to_str().ok())
    }

    fn body_json(request: &wiremock::Request) -> serde_json::Value {
        serde_json::from_slice(&request.body).expect("json request body")
    }

    #[tokio::test]
    async fn anthropic_factory_stream_captures_usage() {
        let server = anthropic_server().await;
        let provider = factory_provider(
            "anthropic-native",
            LlmServiceKind::Anthropic,
            "anthropic-test-key",
            "claude-test",
            format!("{}/v1", server.uri()),
        );

        let events = collect_events(provider.as_ref(), request()).await;

        assert_eq!(delta_texts(&events), vec!["hello"]);
        assert_eq!(error_count(&events), 0, "unexpected error: {events:?}");
        assert_single_finished_last(&events);

        let (usage, model, finish_reason) = finished_event(&events);
        assert_eq!(
            *usage,
            Some(LlmUsage {
                prompt_tokens: 3,
                completion_tokens: 2,
                total_tokens: 5,
            })
        );
        assert_eq!(model.as_deref(), Some("claude-test"));
        assert!(matches!(finish_reason, Some(FinishReason::Stop)));

        let requests = received(&server).await;
        assert_eq!(requests.len(), 1, "expected exactly one request");
        assert_eq!(requests[0].url.path(), "/v1/messages");
        assert_eq!(
            header(&requests[0], "x-api-key"),
            Some("anthropic-test-key")
        );
        assert!(
            header(&requests[0], "anthropic-version").is_some(),
            "anthropic-version header must be sent"
        );
        let json = body_json(&requests[0]);
        assert_eq!(json["model"], "claude-test");
        assert_eq!(json["stream"], true);
    }

    #[tokio::test]
    async fn anthropic_request_model_override_keeps_service() {
        let server = anthropic_server().await;
        let provider = factory_provider(
            "anthropic-native",
            LlmServiceKind::Anthropic,
            "anthropic-test-key",
            "claude-test",
            format!("{}/v1", server.uri()),
        );

        let events = collect_events(
            provider.as_ref(),
            ChatRequest::new("qwen3.5:4b", vec![ChatMessage::user("hello")]),
        )
        .await;

        assert_eq!(delta_texts(&events), vec!["hello"]);
        assert_eq!(error_count(&events), 0, "unexpected error: {events:?}");
        assert_single_finished_last(&events);
        let (_, model, _) = finished_event(&events);
        assert_eq!(model.as_deref(), Some("qwen3.5:4b"));

        let requests = received(&server).await;
        assert_eq!(requests.len(), 1, "expected exactly one request");
        assert_eq!(requests[0].url.path(), "/v1/messages");
        assert_eq!(
            header(&requests[0], "x-api-key"),
            Some("anthropic-test-key")
        );
        assert_eq!(body_json(&requests[0])["model"], "qwen3.5:4b");
    }

    #[tokio::test]
    async fn gemini_factory_stream_captures_usage() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1beta/models/gemini-test:streamGenerateContent"))
            .and(query_param("alt", "sse"))
            .respond_with(
                ResponseTemplate::new(200).set_body_raw(gemini_sse(), "text/event-stream"),
            )
            .mount(&server)
            .await;
        let provider = factory_provider(
            "gemini-native",
            LlmServiceKind::Gemini,
            "gemini-test-key",
            "gemini-test",
            format!("{}/v1beta", server.uri()),
        );

        let events = collect_events(provider.as_ref(), request()).await;

        assert_eq!(delta_texts(&events), vec!["hello"]);
        assert_eq!(error_count(&events), 0, "unexpected error: {events:?}");
        assert_single_finished_last(&events);

        let (usage, model, finish_reason) = finished_event(&events);
        assert_eq!(
            *usage,
            Some(LlmUsage {
                prompt_tokens: 3,
                completion_tokens: 2,
                total_tokens: 5,
            })
        );
        assert_eq!(model.as_deref(), Some("gemini-test"));
        assert!(matches!(finish_reason, Some(FinishReason::Stop)));

        let requests = received(&server).await;
        assert_eq!(requests.len(), 1, "expected exactly one request");
        assert_eq!(
            requests[0].url.path(),
            "/v1beta/models/gemini-test:streamGenerateContent"
        );
        assert_eq!(requests[0].url.query(), Some("alt=sse"));
        assert_eq!(
            header(&requests[0], "x-goog-api-key"),
            Some("gemini-test-key")
        );
    }

    #[tokio::test]
    async fn gemini_resource_exhausted_is_retryable_rate_limit() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1beta/models/gemini-test:streamGenerateContent"))
            .and(query_param("alt", "sse"))
            .respond_with(ResponseTemplate::new(429).set_body_json(serde_json::json!({
                "error": {
                    "code": 429,
                    "status": "RESOURCE_EXHAUSTED",
                    "message": "quota or throttling"
                }
            })))
            .mount(&server)
            .await;
        let provider = factory_provider(
            "gemini-native",
            LlmServiceKind::Gemini,
            "gemini-test-key",
            "gemini-test",
            format!("{}/v1beta", server.uri()),
        );

        let events = collect_events(provider.as_ref(), request()).await;

        assert!(
            !events
                .iter()
                .any(|event| matches!(event, Ok(ChatEvent::Finished { .. }))),
            "an error response must not emit Finished: {events:?}"
        );
        let errors: Vec<&LlmError> = events
            .iter()
            .filter_map(|event| event.as_ref().err())
            .collect();
        assert_eq!(errors.len(), 1, "expected exactly one error: {events:?}");
        assert!(
            matches!(errors[0], LlmError::RateLimit { retry_after: None }),
            "expected RateLimit without retry_after, got: {:?}",
            errors[0]
        );
        assert!(
            is_retryable(errors[0]),
            "RESOURCE_EXHAUSTED must stay retryable"
        );
    }
}

/// Staged-server proofs for the chat-path timeout/deadline layers
/// (change `llm-timeout-deadline`, task 1.4).
///
/// The hidden 30s client total is gone: a peer that sends response headers
/// immediately and then stalls the body must not be cut by the transport.
/// `wiremock` cannot express that split (its delay covers headers too), so
/// these tests drive `HttpTestServer`'s staged responses.
///
/// `build_override_with_policy` / `policy_provider` / `staged_sse` /
/// `start_staged_server` / `send_after` are `pub(super)` so task 1.5 can
/// reuse them from a sibling module in this file.
#[cfg(any(feature = "openai", feature = "all-providers"))]
mod staged_deadline {
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use camel_api::{Body, Exchange, Message, SsrfPolicy};
    use camel_component_api::NetworkRetryPolicy;
    use camel_component_api::test_support::http::{
        HttpTestServer, Responder, Response, ResponseHeaders,
    };
    use tower::Service;

    use super::super::{BuildOverride, GenaiConfig, GenaiProvider};
    use super::wiremock_error::chat_error;
    use crate::LlmHttpPolicy;
    use crate::error::{LlmError, TransportTimeoutPhase};
    use crate::producer::LlmProducer;
    use crate::{LlmEndpointConfig, LlmOperation};

    /// Build override using the REAL production hardened builder with the
    /// given policy, so the client enforces exactly what the adapter maps.
    pub(super) fn build_override_with_policy(
        connect: Duration,
        read_idle: Option<Duration>,
    ) -> BuildOverride {
        Arc::new(move || {
            Box::pin(async move {
                let client = crate::hardened_http_client_with_policy(
                    LlmHttpPolicy { connect, read_idle },
                    None,
                )
                .map_err(|err| LlmError::Network(format!("test hardened client: {err}")))?;
                Ok(genai::Client::builder().with_reqwest(client).build())
            })
        })
    }

    /// Provider whose enforced client policy and mapped policy are the SAME
    /// value (policy threading agreement).
    pub(super) fn policy_provider(base_url: String, policy: LlmHttpPolicy) -> GenaiProvider {
        GenaiProvider::new_with_build_override(
            "staged-openai",
            "gpt-4o",
            GenaiConfig::OpenAi {
                api_key: "sk-test".into(),
                base_url: Some(base_url),
            },
            policy,
            build_override_with_policy(policy.connect, policy.read_idle),
            SsrfPolicy::AllowInternal,
        )
    }

    /// Staged SSE response: headers flush immediately, the body follows `rx`.
    ///
    /// `transfer-encoding: chunked` is declared because the harness frames
    /// staged chunks as HTTP chunks; without it the client would read the
    /// framing lines as body bytes (connection-delimited response).
    pub(super) fn staged_sse(rx: tokio::sync::mpsc::Receiver<Vec<u8>>) -> Response {
        Response::staged(
            ResponseHeaders::new(
                200,
                "OK",
                vec![
                    ("content-type".to_string(), "text/event-stream".to_string()),
                    ("transfer-encoding".to_string(), "chunked".to_string()),
                ],
            ),
            rx,
        )
    }

    /// Start a staged localhost server and return its body sender.
    ///
    /// Base URL: [`base_url`]. The IP literal avoids DNS under paused clocks.
    pub(super) async fn start_staged_server() -> (HttpTestServer, tokio::sync::mpsc::Sender<Vec<u8>>)
    {
        let (tx, rx) = tokio::sync::mpsc::channel::<Vec<u8>>(16);
        let slot = Arc::new(std::sync::Mutex::new(Some(rx)));
        let server = HttpTestServer::start(Responder::route(move |_req| {
            let rx = slot
                .lock()
                .expect("staged response slot")
                .take()
                .expect("staged response is requested once");
            staged_sse(rx)
        }))
        .await
        .expect("start staged HTTP server");
        (server, tx)
    }

    /// OpenAI-compatible base URL for a staged server.
    pub(super) fn base_url(server: &HttpTestServer) -> String {
        format!("http://127.0.0.1:{}/v1", server.port())
    }

    /// Send `chunk` after `delay`.
    ///
    /// Nested helper: `lint-test-sleep` exempts sleeps inside helpers, and the
    /// sleep participates in virtual time under `start_paused`.
    pub(super) async fn send_after(
        tx: &tokio::sync::mpsc::Sender<Vec<u8>>,
        delay: Duration,
        chunk: &[u8],
    ) {
        tokio::time::sleep(delay).await;
        let _ = tx.send(chunk.to_vec()).await;
    }

    fn sse_data(payload: &str) -> Vec<u8> {
        format!("data: {payload}\n\n").into_bytes()
    }

    fn sse_chunk(text: &str) -> Vec<u8> {
        sse_data(&serde_json::json!({"choices":[{"delta":{"content": text}}]}).to_string())
    }

    fn sse_finish() -> Vec<u8> {
        sse_data(r#"{"choices":[{"delta":{},"finish_reason":"stop"}]}"#)
    }

    fn sse_done() -> Vec<u8> {
        sse_data("[DONE]")
    }

    /// Materialized chat producer (`stream=false`) with the given deadline.
    pub(super) fn materialized_producer(
        provider: GenaiProvider,
        timeout: Option<Duration>,
    ) -> LlmProducer {
        let config = LlmEndpointConfig {
            operation: LlmOperation::Chat,
            stream: false,
            ..Default::default()
        };
        LlmProducer::new(config, Arc::new(provider), 32768, "test-route".into())
            .with_timeout(timeout)
            .build()
    }

    /// Materialized chat producer with deadline and network retry (task 1.5).
    pub(super) fn materialized_producer_with_retry(
        provider: GenaiProvider,
        timeout: Option<Duration>,
        retry: Option<NetworkRetryPolicy>,
    ) -> LlmProducer {
        let config = LlmEndpointConfig {
            operation: LlmOperation::Chat,
            stream: false,
            ..Default::default()
        };
        LlmProducer::new(config, Arc::new(provider), 32768, "test-route".into())
            .with_timeout(timeout)
            .with_retry(retry)
            .build()
    }

    /// Two-attempt network retry policy with negligible real backoff.
    fn two_attempt_retry() -> NetworkRetryPolicy {
        NetworkRetryPolicy {
            enabled: true,
            max_attempts: 2,
            initial_delay: Duration::from_millis(1),
            multiplier: 1.0,
            max_delay: Duration::from_millis(5),
            jitter_factor: 0.0,
            max_attempts_absolute: None,
        }
    }

    fn exchange() -> Exchange {
        Exchange::new(Message::new(Body::Text("hello".into())))
    }
    /// Blessed-spec scenario: a body gap beyond the removed 30s client total
    /// completes under the configured deadline. THE discriminating test — on
    /// pre-change code the 30s client total cut this stream.
    #[tokio::test(start_paused = true)]
    async fn body_gap_beyond_30s_completes_under_configured_deadline() {
        let (server, tx) = start_staged_server().await;
        let provider = policy_provider(
            base_url(&server),
            LlmHttpPolicy {
                connect: Duration::from_secs(10),
                read_idle: Some(Duration::from_secs(600)),
            },
        );
        let mut producer = materialized_producer(provider, Some(Duration::from_secs(90)));

        let writer = tokio::spawn(async move {
            // Headers flush at request time; the first body byte waits 45s of
            // VIRTUAL time — beyond the removed 30s client total.
            send_after(&tx, Duration::from_secs(45), &sse_chunk("hello")).await;
            send_after(&tx, Duration::from_millis(1), &sse_chunk(" world")).await;
            send_after(&tx, Duration::from_millis(1), &sse_finish()).await;
            send_after(&tx, Duration::from_millis(1), &sse_done()).await;
        });

        // Bound 120s (virtual): above the 90s configured deadline and the 45s
        // intentional gap, so only a real regression trips this bound.
        let result = tokio::time::timeout(Duration::from_secs(120), producer.call(exchange()))
            .await
            .expect("producer call must finish within the 120s test bound");

        let out = result.expect("the 45s body gap must not abort the chat");
        match &out.input.body {
            Body::Text(text) => assert_eq!(text, "hello world"),
            other => panic!("expected materialized text body, got {other:?}"),
        }
        writer.abort();
        server.shutdown().await;
    }

    /// Blessed-spec scenario: the transport read-idle guard lapses before the
    /// configured producer deadline, so the transport layer is reported.
    ///
    /// Producer-level precedence proof (display text only): a provider-only
    /// probe cannot see which producer deadline would have fired.
    #[tokio::test]
    async fn read_idle_lapse_maps_transport_timeout_idle() {
        let (server, tx) = start_staged_server().await;
        let provider = policy_provider(
            base_url(&server),
            LlmHttpPolicy {
                connect: Duration::from_secs(10),
                read_idle: Some(Duration::from_millis(200)),
            },
        );
        let mut producer = materialized_producer(provider, Some(Duration::from_secs(5)));

        // Headers plus one chunk flush immediately; keeping the sender alive
        // stalls the rest of the body (dropping it would end the response).
        tx.try_send(sse_chunk("partial"))
            .expect("staged body channel has capacity");
        let _stall = tx;

        // Bound 5s (real time): the 200ms idle guard must win well inside it.
        let result = tokio::time::timeout(Duration::from_secs(5), producer.call(exchange()))
            .await
            .expect("producer call must end within the 5s test bound");

        let err = result.expect_err("a stalled body must time out");
        let display = err.to_string();
        assert!(
            display.contains("timeout"),
            "expected timeout, got: {display}"
        );
        assert!(
            display.contains("no response activity for 200ms"),
            "the transport idle guard must win before the 5s deadline, got: {display}"
        );
        server.shutdown().await;
    }

    /// Honest contract replacing the retired stale test (which asserted the
    /// misreport — a 100ms transport lapse reported as a configured 1s
    /// deadline): a transport read-idle lapse reports the layer that fired and
    /// the bound the policy enforced (100ms), never a configured deadline.
    #[tokio::test]
    async fn transport_idle_lapse_reports_idle_layer_not_configured_deadline() {
        let (server, tx) = start_staged_server().await;
        let provider = policy_provider(
            base_url(&server),
            LlmHttpPolicy {
                connect: Duration::from_secs(10),
                read_idle: Some(Duration::from_millis(100)),
            },
        );

        // Headers flush immediately; keeping the sender alive stalls the body.
        // (Dropping it would end the body cleanly instead of stalling, so no
        // timeout would fire at all.)
        let _stall = tx;

        // Bound 5s (real time): above the 100ms enforced idle guard. This is a
        // test bound only; chat_error bypasses the producer deadline.
        let err = tokio::time::timeout(Duration::from_secs(5), chat_error(&provider))
            .await
            .expect("chat error must surface within the 5s test bound");

        assert!(
            matches!(
                err,
                LlmError::TransportTimeout {
                    phase: TransportTimeoutPhase::Idle,
                    after,
                } if after == Duration::from_millis(100)
            ),
            "expected TransportTimeout::Idle after the enforced 100ms, got: {err:?}"
        );
        server.shutdown().await;
    }

    /// Blessed-spec scenario: the reported idle bound equals the ENFORCED
    /// policy (policy threading proof). Typed variant, no producer involved.
    #[tokio::test]
    async fn transport_idle_variant_is_typed() {
        let (server, tx) = start_staged_server().await;
        let provider = policy_provider(
            base_url(&server),
            LlmHttpPolicy {
                connect: Duration::from_secs(10),
                read_idle: Some(Duration::from_millis(200)),
            },
        );
        tx.try_send(sse_chunk("partial"))
            .expect("staged body channel has capacity");
        let _stall = tx;

        // Bound 5s (real time): above the 200ms guard that must fire.
        let err = tokio::time::timeout(Duration::from_secs(5), chat_error(&provider))
            .await
            .expect("chat error must surface within the 5s test bound");

        assert!(
            matches!(
                err,
                LlmError::TransportTimeout {
                    phase: TransportTimeoutPhase::Idle,
                    after,
                } if after == Duration::from_millis(200)
            ),
            "expected TransportTimeout::Idle after the enforced 200ms, got: {err:?}"
        );
        server.shutdown().await;
    }

    /// Blessed-spec scenario: the read-idle guard resets per read, not per
    /// response. Chunks arrive 100ms apart under a 300ms guard.
    #[tokio::test(start_paused = true)]
    async fn read_idle_resets_per_read_not_total() {
        let (server, tx) = start_staged_server().await;
        let provider = policy_provider(
            base_url(&server),
            LlmHttpPolicy {
                connect: Duration::from_secs(10),
                read_idle: Some(Duration::from_millis(300)),
            },
        );
        let mut producer = materialized_producer(provider, Some(Duration::from_secs(5)));

        let writer = tokio::spawn(async move {
            let mut ticker = tokio::time::interval(Duration::from_millis(100));
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            for index in 0..12 {
                ticker.tick().await;
                let _ = tx.send(sse_chunk(&index.to_string())).await;
            }
            ticker.tick().await;
            let _ = tx.send(sse_finish()).await;
            ticker.tick().await;
            let _ = tx.send(sse_done()).await;
        });

        // Bound 120s (virtual): above the ~1.3s paced timeline.
        let result = tokio::time::timeout(Duration::from_secs(120), producer.call(exchange()))
            .await
            .expect("producer call must finish within the 120s test bound");

        let out = result.expect("paced chunks under the idle guard must complete");
        let expected: String = (0..12).map(|index| index.to_string()).collect();
        match &out.input.body {
            Body::Text(text) => assert_eq!(text, &expected),
            other => panic!("expected materialized text body, got {other:?}"),
        }
        writer.abort();
        server.shutdown().await;
    }

    /// Blessed-spec scenario: the producer deadline maps to `Timeout` with the
    /// CONFIGURED duration; no idle guard can shadow it (`read_idle: None`).
    ///
    /// The typed `Timeout` variant is owned by 1.2's unit tests
    /// (`apply_timeout` lives in the producer), so this asserts display text.
    #[tokio::test(start_paused = true)]
    async fn producer_deadline_maps_timeout_with_configured_duration() {
        let (server, tx) = start_staged_server().await;
        let provider = policy_provider(
            base_url(&server),
            LlmHttpPolicy {
                connect: Duration::from_secs(10),
                read_idle: None,
            },
        );
        let mut producer = materialized_producer(provider, Some(Duration::from_millis(400)));
        let _stall = tx;

        // Bound 120s (virtual): above the 400ms configured deadline.
        let result = tokio::time::timeout(Duration::from_secs(120), producer.call(exchange()))
            .await
            .expect("producer call must finish within the 120s test bound");

        let err = result.expect_err("the 400ms deadline must fire");
        let display = err.to_string();
        assert!(
            display.contains("timeout"),
            "expected timeout, got: {display}"
        );
        assert!(
            display.contains("400ms"),
            "expected the configured 400ms, got: {display}"
        );
        server.shutdown().await;
    }

    /// Blessed-spec scenario "Hung chat connection is still bounded" at test
    /// scale: with no configured deadline (`timeout_secs` unset), the transport
    /// read-idle guard still ends the call — no silent hang.
    #[tokio::test]
    async fn hung_connection_bounded_when_deadline_unset() {
        let (server, tx) = start_staged_server().await;
        let provider = policy_provider(
            base_url(&server),
            LlmHttpPolicy {
                connect: Duration::from_secs(10),
                read_idle: Some(Duration::from_millis(200)),
            },
        );
        let mut producer = materialized_producer(provider, None);
        tx.try_send(sse_chunk("partial"))
            .expect("staged body channel has capacity");
        let _stall = tx;

        // Bound 5s (real time): above the 200ms guard. There is no producer
        // deadline here, so the test supplies the only bound.
        let result = tokio::time::timeout(Duration::from_secs(5), producer.call(exchange()))
            .await
            .expect("producer call must end within the 5s test bound");

        let err = result.expect_err("a hung connection without a deadline must still end");
        let display = err.to_string();
        assert!(
            display.contains("timeout"),
            "expected timeout, got: {display}"
        );
        assert!(
            display.contains("no response activity for 200ms"),
            "the transport idle guard must bound the hang, got: {display}"
        );
        server.shutdown().await;
    }

    /// Blessed-spec scenario: a transport read-idle lapse BEFORE any content
    /// Delta is retryable — the retry loop issues a second HTTP attempt and
    /// the second answer completes.
    #[tokio::test]
    async fn transport_timeout_before_content_is_retried() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let stalled: Arc<Mutex<Vec<tokio::sync::mpsc::Sender<Vec<u8>>>>> =
            Arc::new(Mutex::new(Vec::new()));
        let counter = Arc::clone(&attempts);
        let holders = Arc::clone(&stalled);
        let server = HttpTestServer::start(Responder::route(move |_req| {
            let attempt = counter.fetch_add(1, Ordering::SeqCst);
            let (tx, rx) = tokio::sync::mpsc::channel::<Vec<u8>>(16);
            if attempt == 0 {
                // Park the sender: headers flush, the body never ends, and the
                // read-idle guard fires before any content Delta.
                holders.lock().expect("stall holder").push(tx);
            } else {
                tx.try_send(sse_chunk("recovered"))
                    .expect("staged body channel has capacity");
                tx.try_send(sse_finish())
                    .expect("staged body channel has capacity");
                tx.try_send(sse_done())
                    .expect("staged body channel has capacity");
                // Dropping `tx` here ends the body after the buffered chunks.
            }
            staged_sse(rx)
        }))
        .await
        .expect("start retry staged server");
        let provider = policy_provider(
            base_url(&server),
            LlmHttpPolicy {
                connect: Duration::from_secs(10),
                read_idle: Some(Duration::from_millis(150)),
            },
        );
        let mut producer = materialized_producer_with_retry(
            provider,
            Some(Duration::from_secs(10)),
            Some(two_attempt_retry()),
        );

        // Bound 5s (real time): above 150ms idle guard + 1ms backoff.
        let result = tokio::time::timeout(Duration::from_secs(5), producer.call(exchange()))
            .await
            .expect("producer call must finish within the 5s test bound");

        let out = result.expect("a pre-content transport timeout must be retried");
        match &out.input.body {
            Body::Text(text) => assert_eq!(text, "recovered"),
            other => panic!("expected materialized text body, got {other:?}"),
        }
        assert_eq!(
            server.recorded().len(),
            2,
            "one stalled attempt plus one retry"
        );
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
        drop(stalled);
        server.shutdown().await;
    }

    /// Blessed-spec scenario: a transport read-idle lapse AFTER content
    /// started must NOT be retried (unchanged producer rule at
    /// `producer.rs:173`); the call fails with the transport layer text.
    #[tokio::test]
    async fn transport_timeout_after_content_is_not_retried() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let stalled: Arc<Mutex<Vec<tokio::sync::mpsc::Sender<Vec<u8>>>>> =
            Arc::new(Mutex::new(Vec::new()));
        let counter = Arc::clone(&attempts);
        let holders = Arc::clone(&stalled);
        let server = HttpTestServer::start(Responder::route(move |_req| {
            let attempt = counter.fetch_add(1, Ordering::SeqCst);
            let (tx, rx) = tokio::sync::mpsc::channel::<Vec<u8>>(16);
            tx.try_send(sse_chunk("partial"))
                .expect("staged body channel has capacity");
            if attempt == 0 {
                // Content already started; parking the sender stalls the body
                // so the read-idle guard fires after the Delta.
                holders.lock().expect("stall holder").push(tx);
            } else {
                // Must never be served: content-started suppresses the retry.
                tx.try_send(sse_done())
                    .expect("staged body channel has capacity");
            }
            staged_sse(rx)
        }))
        .await
        .expect("start no-retry staged server");
        let provider = policy_provider(
            base_url(&server),
            LlmHttpPolicy {
                connect: Duration::from_secs(10),
                read_idle: Some(Duration::from_millis(150)),
            },
        );
        let mut producer = materialized_producer_with_retry(
            provider,
            Some(Duration::from_secs(10)),
            Some(two_attempt_retry()),
        );

        // Bound 5s (real time): above the 150ms idle guard.
        let result = tokio::time::timeout(Duration::from_secs(5), producer.call(exchange()))
            .await
            .expect("producer call must finish within the 5s test bound");

        let err = result.expect_err("a post-content transport timeout must surface");
        let display = err.to_string();
        assert!(
            display.contains("timeout"),
            "expected timeout, got: {display}"
        );
        assert!(
            display.contains("no response activity for 150ms"),
            "expected the enforced idle bound, got: {display}"
        );
        assert_eq!(
            server.recorded().len(),
            1,
            "content started: the attempt must not be retried"
        );
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
        drop(stalled);
        server.shutdown().await;
    }
}
