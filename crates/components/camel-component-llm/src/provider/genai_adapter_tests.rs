//! Unit tests for the pure genai conversion/error-mapping layer.

use std::time::Duration;

use super::*;
use crate::error::LlmError;
use crate::provider::{
    ChatMessage, ChatRole, EmbedRequest, EmittedToolCall, FinishReason, LlmProvider, LlmUsage,
    ToolChoice, ToolDefinition,
};
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
            map_genai_error(&err, Duration::from_secs(30)),
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

    match map_genai_error(&err, Duration::from_secs(30)) {
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
            map_genai_error(&err, Duration::from_secs(30)),
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
            map_genai_error(&err, Duration::from_secs(30)),
            LlmError::ProviderUnavailable(_)
        ));
    }
}

#[tokio::test]
async fn map_error_timeout_and_connect() {
    let connect = connect_error().await;
    assert!(connect.is_connect());
    assert!(matches!(
        map_genai_error(&web_stream(connect), Duration::from_secs(42)),
        LlmError::Network(_)
    ));

    let timeout = timeout_error().await;
    assert!(timeout.is_timeout());
    match map_genai_error(&web_stream(timeout), Duration::from_secs(42)) {
        LlmError::Timeout(after) => assert_eq!(after, Duration::from_secs(42)),
        other => panic!("expected Timeout, got {other:?}"),
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

    match map_genai_error(&err, Duration::from_secs(30)) {
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
        map_genai_error(&err, Duration::from_secs(30)),
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
        map_genai_error(&err, Duration::from_secs(30)),
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

    match map_genai_error(&err, Duration::from_secs(30)) {
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

    match map_genai_error(&err, Duration::from_secs(30)) {
        LlmError::RateLimit { retry_after } => assert_eq!(retry_after, None),
        other => panic!("expected RateLimit, got {other:?}"),
    }
}

#[tokio::test]
async fn webc_reqwest_timeout_maps_to_timeout() {
    let timeout = timeout_error().await;
    let err = genai::Error::WebModelCall {
        model_iden: model_iden(),
        webc_error: genai::webc::Error::Reqwest(timeout),
    };

    match map_genai_error(&err, Duration::from_secs(42)) {
        LlmError::Timeout(after) => assert_eq!(after, Duration::from_secs(42)),
        other => panic!("expected Timeout, got {other:?}"),
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
        map_genai_error(&err, Duration::from_secs(30)),
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
        Duration::from_secs(30),
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
        Duration::from_secs(30),
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
    use std::time::Duration;

    use camel_api::SsrfPolicy;
    use futures::StreamExt;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::super::{BuildOverride, GenaiConfig, GenaiProvider};
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
            Duration::from_secs(10),
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
    use std::time::Duration;

    use camel_api::SsrfPolicy;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::super::{BuildOverride, GenaiConfig, GenaiProvider};
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
            Duration::from_secs(10),
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
            Duration::from_secs(10),
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
    use std::time::Duration;

    use camel_api::SsrfPolicy;
    use futures::StreamExt;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::super::{BuildOverride, GenaiConfig, GenaiProvider};
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
            Duration::from_secs(10),
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

    /// Transport client with a timeout shorter than the provider's configured
    /// timeout, so the mapping test can prove which value reaches `Timeout`.
    fn build_override_with_transport_timeout(timeout: Duration) -> BuildOverride {
        Arc::new(move || {
            Box::pin(async move {
                let client = reqwest::Client::builder()
                    .no_proxy()
                    .timeout(timeout)
                    .build()
                    .map_err(|err| LlmError::Network(format!("test reqwest client: {err}")))?;
                Ok(genai::Client::builder().with_reqwest(client).build())
            })
        })
    }

    fn openai_provider(
        base_url: String,
        configured_timeout: Duration,
        override_: BuildOverride,
    ) -> GenaiProvider {
        GenaiProvider::new_with_build_override(
            "wiremock-openai-errors",
            "gpt-4o",
            GenaiConfig::OpenAi {
                api_key: "sk-test".into(),
                base_url: Some(base_url),
            },
            configured_timeout,
            override_,
            SsrfPolicy::AllowInternal,
        )
    }

    fn provider_for(server: &MockServer) -> GenaiProvider {
        openai_provider(
            format!("{}/v1", server.uri()),
            Duration::from_secs(10),
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
    async fn chat_error(provider: &GenaiProvider) -> LlmError {
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
            Duration::from_secs(10),
            build_override(),
        );

        let err = chat_error(&provider).await;

        assert!(
            matches!(err, LlmError::Network(_)),
            "expected Network, got: {err:?}"
        );
        assert!(is_retryable(&err), "connection failures must be retryable");
    }

    #[tokio::test]
    async fn chat_timeout_maps_timeout_with_configured_duration() {
        // The transport gives up after 100ms while the provider is configured
        // for 1s. The mapped error must carry the CONFIGURED duration.
        let server = mock_server(
            CHAT_PATH,
            ResponseTemplate::new(200)
                .set_delay(Duration::from_millis(300))
                .set_body_raw("data: [DONE]\n\n", "text/event-stream"),
        )
        .await;
        let provider = openai_provider(
            format!("{}/v1", server.uri()),
            Duration::from_secs(1),
            build_override_with_transport_timeout(Duration::from_millis(100)),
        );

        let err = chat_error(&provider).await;

        assert!(
            matches!(err, LlmError::Timeout(after) if after == Duration::from_secs(1)),
            "expected Timeout with the configured 1s duration, got: {err:?}"
        );
    }
}
