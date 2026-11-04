//! Unit tests for the LLM configuration contract.

use super::*;

#[test]
fn rejects_zero_global_timeout() {
    let cfg = LlmGlobalConfig {
        default_provider: None,
        timeout_secs: Some(0), // Some(0) is invalid; None is valid
        max_prompt_bytes: 32768,
        max_header_json_bytes: 65_536,
        providers: HashMap::new(),
        allow_internal: false,
        ..Default::default()
    };
    assert!(
        cfg.validate().is_err(),
        "Some(0) global timeout_secs must be rejected"
    );
}

#[test]
fn accepts_none_global_timeout() {
    let cfg = LlmGlobalConfig {
        default_provider: None,
        timeout_secs: None, // None = no timeout, valid
        max_prompt_bytes: 32768,
        max_header_json_bytes: 65_536,
        providers: HashMap::new(),
        allow_internal: false,
        ..Default::default()
    };
    assert!(
        cfg.validate().is_ok(),
        "None global timeout_secs must be valid"
    );
}

#[test]
fn rejects_zero_provider_timeout() {
    let mut providers = HashMap::new();
    providers.insert(
        "bad".into(),
        LlmProviderConfig::Openai(OpenaiProviderConfig {
            api_key: "sk-test".into(),
            base_url: None,
            default_model: "gpt-4o".into(),
            timeout_secs: Some(0), // invalid
            max_concurrency: None,
            network_retry: None,
            pricing: None,
            cache_ttl_secs: None,
            cache_max_entries: None,
        }),
    );
    let cfg = LlmGlobalConfig {
        default_provider: None,
        timeout_secs: None,
        max_prompt_bytes: 32768,
        max_header_json_bytes: 65_536,
        providers,
        allow_internal: false,
        ..Default::default()
    };
    assert!(
        cfg.validate().is_err(),
        "Some(0) provider timeout_secs must be rejected"
    );
}

#[test]
fn rejects_zero_max_concurrency() {
    let mut providers = HashMap::new();
    providers.insert(
        "bad".into(),
        LlmProviderConfig::Openai(OpenaiProviderConfig {
            api_key: "sk-test".into(),
            base_url: None,
            default_model: "gpt-4o".into(),
            timeout_secs: None,
            max_concurrency: Some(0), // invalid
            network_retry: None,
            pricing: None,
            cache_ttl_secs: None,
            cache_max_entries: None,
        }),
    );
    let cfg = LlmGlobalConfig {
        default_provider: None,
        timeout_secs: None,
        max_prompt_bytes: 32768,
        max_header_json_bytes: 65_536,
        providers,
        allow_internal: false,
        ..Default::default()
    };
    assert!(
        cfg.validate().is_err(),
        "Some(0) max_concurrency must be rejected"
    );
}

#[test]
fn rejects_zero_ollama_timeout() {
    let mut providers = HashMap::new();
    providers.insert(
        "bad".into(),
        LlmProviderConfig::Ollama(OllamaProviderConfig {
            base_url: "http://localhost:11434".into(),
            default_model: "llama3".into(),
            timeout_secs: Some(0), // invalid
            max_concurrency: None,
            network_retry: None,
            pricing: None,
            cache_ttl_secs: None,
            cache_max_entries: None,
        }),
    );
    let cfg = LlmGlobalConfig {
        default_provider: None,
        timeout_secs: None,
        max_prompt_bytes: 32768,
        max_header_json_bytes: 65_536,
        providers,
        allow_internal: false,
        ..Default::default()
    };
    assert!(
        cfg.validate().is_err(),
        "Some(0) Ollama timeout_secs must be rejected"
    );
}

#[test]
fn rejects_negative_pricing() {
    let mut providers = HashMap::new();
    providers.insert(
        "bad".into(),
        LlmProviderConfig::Openai(OpenaiProviderConfig {
            api_key: "sk-test".into(),
            base_url: None,
            default_model: "gpt-4o".into(),
            timeout_secs: None,
            max_concurrency: None,
            network_retry: None,
            pricing: Some(PricingTable {
                input_per_1k_tokens: -0.01,
                output_per_1k_tokens: 0.03,
            }),
            cache_ttl_secs: None,
            cache_max_entries: None,
        }),
    );
    let cfg = LlmGlobalConfig {
        default_provider: None,
        timeout_secs: None,
        max_prompt_bytes: 32768,
        max_header_json_bytes: 65_536,
        providers,
        allow_internal: false,
        ..Default::default()
    };
    assert!(
        cfg.validate().is_err(),
        "Negative input_per_1k_tokens must be rejected"
    );
}

#[test]
fn rejects_zero_cache_ttl() {
    let mut providers = HashMap::new();
    providers.insert(
        "bad-cache".into(),
        LlmProviderConfig::Openai(OpenaiProviderConfig {
            api_key: "sk-test".into(),
            base_url: None,
            default_model: "gpt-4o".into(),
            timeout_secs: None,
            max_concurrency: None,
            network_retry: None,
            pricing: None,
            cache_ttl_secs: Some(0), // invalid
            cache_max_entries: None,
        }),
    );
    let cfg = LlmGlobalConfig {
        default_provider: None,
        timeout_secs: None,
        max_prompt_bytes: 32768,
        max_header_json_bytes: 65_536,
        providers,
        allow_internal: false,
        ..Default::default()
    };
    assert!(
        cfg.validate().is_err(),
        "Some(0) cache_ttl_secs must be rejected"
    );
}

#[test]
fn rejects_zero_cache_max_entries() {
    let mut providers = HashMap::new();
    providers.insert(
        "bad-entries".into(),
        LlmProviderConfig::Openai(OpenaiProviderConfig {
            api_key: "sk-test".into(),
            base_url: None,
            default_model: "gpt-4o".into(),
            timeout_secs: None,
            max_concurrency: None,
            network_retry: None,
            pricing: None,
            cache_ttl_secs: None,
            cache_max_entries: Some(0), // invalid
        }),
    );
    let cfg = LlmGlobalConfig {
        default_provider: None,
        timeout_secs: None,
        max_prompt_bytes: 32768,
        max_header_json_bytes: 65_536,
        providers,
        allow_internal: false,
        ..Default::default()
    };
    assert!(
        cfg.validate().is_err(),
        "Some(0) cache_max_entries must be rejected"
    );
}

#[test]
fn accepts_valid_provider_config() {
    let mut providers = HashMap::new();
    providers.insert(
        "valid".into(),
        LlmProviderConfig::Openai(OpenaiProviderConfig {
            api_key: "sk-test".into(),
            base_url: None,
            default_model: "gpt-4o".into(),
            timeout_secs: Some(30),
            max_concurrency: Some(5),
            network_retry: None,
            pricing: None,
            cache_ttl_secs: None,
            cache_max_entries: None,
        }),
    );
    let cfg = LlmGlobalConfig {
        default_provider: None,
        timeout_secs: Some(60),
        max_prompt_bytes: 32768,
        max_header_json_bytes: 65_536,
        providers,
        allow_internal: false,
        ..Default::default()
    };
    assert!(
        cfg.validate().is_ok(),
        "Valid config with non-zero timeouts and concurrency must pass"
    );
}

#[test]
fn validate_error_contains_provider_name() {
    let mut providers = HashMap::new();
    providers.insert(
        "my-openai".into(),
        LlmProviderConfig::Openai(OpenaiProviderConfig {
            api_key: "sk-test".into(),
            base_url: None,
            default_model: "gpt-4o".into(),
            timeout_secs: Some(0),
            max_concurrency: None,
            network_retry: None,
            pricing: None,
            cache_ttl_secs: None,
            cache_max_entries: None,
        }),
    );
    let cfg = LlmGlobalConfig {
        default_provider: None,
        timeout_secs: None,
        max_prompt_bytes: 32768,
        max_header_json_bytes: 65_536,
        providers,
        allow_internal: false,
        ..Default::default()
    };
    let err = cfg.validate().unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("my-openai"),
        "validation error should include provider name: {msg}"
    );
}

#[test]
fn deserialize_global_config_with_providers() {
    let toml_str = r#"
default_provider = "my-openai"

[providers.my-openai]
type = "openai"
api_key = "secret-key"
default_model = "gpt-4o"

[providers.local]
type = "ollama"
base_url = "http://localhost:11434"
default_model = "llama3"

[providers.test]
type = "mock"
response = "echo"
default_model = "mock-model"
"#;
    let cfg: LlmGlobalConfig = toml::from_str(toml_str).expect("parse");
    assert_eq!(cfg.default_provider.as_deref(), Some("my-openai"));
    assert_eq!(cfg.providers.len(), 3);
    assert!(matches!(
        cfg.providers["my-openai"],
        LlmProviderConfig::Openai(_)
    ));
    assert!(matches!(
        cfg.providers["local"],
        LlmProviderConfig::Ollama(_)
    ));
    assert!(matches!(cfg.providers["test"], LlmProviderConfig::Mock(_)));
}

#[test]
fn default_config_has_no_providers() {
    let cfg = LlmGlobalConfig::default();
    assert!(cfg.providers.is_empty());
    assert_eq!(cfg.timeout_secs, None);
    assert_eq!(cfg.max_prompt_bytes, 32768);
    assert_eq!(cfg.max_header_json_bytes, 65_536);
}

#[test]
fn debug_redacts_api_key() {
    let cfg = LlmProviderConfig::Openai(OpenaiProviderConfig {
        api_key: "sk-secret123".into(),
        base_url: None,
        default_model: "gpt-4o".into(),
        timeout_secs: None,
        max_concurrency: None,
        network_retry: None,
        pricing: None,
        cache_ttl_secs: None,
        cache_max_entries: None,
    });
    let debug_str = format!("{:?}", cfg);
    assert!(debug_str.contains("[REDACTED]"));
    assert!(!debug_str.contains("sk-secret123"));
}

#[test]
fn endpoint_config_from_uri_chat() {
    let ec = LlmEndpointConfig::from_uri(
        "llm:chat?provider=my-openai&model=gpt-4o&temperature=0.7&stream=false",
    );
    let ec = ec.expect("parse");
    assert_eq!(ec.operation, LlmOperation::Chat);
    assert_eq!(ec.provider.as_deref(), Some("my-openai"));
    assert_eq!(ec.model.as_deref(), Some("gpt-4o"));
    assert!(!ec.stream);
}

#[test]
fn endpoint_config_from_uri_embed() {
    let ec = LlmEndpointConfig::from_uri("llm:embed?provider=local");
    let ec = ec.expect("parse");
    assert_eq!(ec.operation, LlmOperation::Embed);
    assert!(ec.stream); // default
}

#[test]
fn from_uri_unknown_operation_returns_invalid_uri() {
    let result = LlmEndpointConfig::from_uri("llm:summarize?provider=x");
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(matches!(err, CamelError::InvalidUri(_)));
    assert!(err.to_string().contains("summarize"));
}

#[test]
fn mock_config_with_error_message_deserializes() {
    let toml_str = r#"
[providers.err]
type = "mock"
error_message = "boom"
"#;
    let cfg: LlmGlobalConfig = toml::from_str(toml_str).expect("parse");
    let mock_cfg = match &cfg.providers["err"] {
        LlmProviderConfig::Mock(c) => c,
        _ => panic!("expected Mock"),
    };
    assert_eq!(mock_cfg.error_message.as_deref(), Some("boom"));
}

#[test]
fn from_uri_stream_parsing() {
    // Explicit false
    let ec = LlmEndpointConfig::from_uri("llm:chat?stream=false").unwrap();
    assert!(!ec.stream);

    // Explicit true
    let ec = LlmEndpointConfig::from_uri("llm:chat?stream=true").unwrap();
    assert!(ec.stream);

    // Numeric true
    let ec = LlmEndpointConfig::from_uri("llm:chat?stream=1").unwrap();
    assert!(ec.stream);

    // Default (no param)
    let ec = LlmEndpointConfig::from_uri("llm:chat").unwrap();
    assert!(ec.stream);
}

#[test]
fn decide_uri_parses() {
    let ec = LlmEndpointConfig::from_uri("llm:decide?questions=triage&provider=fake&model=m")
        .expect("parse");
    assert_eq!(ec.operation, LlmOperation::Decide);
    assert_eq!(ec.questions.as_deref(), Some("triage"));
    assert_eq!(ec.provider.as_deref(), Some("fake"));
    assert_eq!(ec.model.as_deref(), Some("m"));
}

#[test]
fn decide_requires_questions() {
    let result = LlmEndpointConfig::from_uri("llm:decide");
    match result {
        Err(CamelError::InvalidUri(m)) => assert!(m.contains("questions"), "message: {m}"),
        other => panic!("expected InvalidUri, got {other:?}"),
    }
}

#[test]
fn decide_rejects_temperature() {
    let result = LlmEndpointConfig::from_uri("llm:decide?questions=t&temperature=0.2");
    match result {
        Err(e) => assert!(e.to_string().contains("temperature"), "error: {e}"),
        Ok(_) => panic!("expected error for temperature on llm:decide"),
    }
}

#[test]
fn decide_rejects_stream() {
    let result = LlmEndpointConfig::from_uri("llm:decide?questions=t&stream=false");
    match result {
        Err(e) => assert!(e.to_string().contains("stream"), "error: {e}"),
        Ok(_) => panic!("expected error for stream on llm:decide"),
    }
}

#[test]
fn chat_rejects_questions() {
    let result = LlmEndpointConfig::from_uri("llm:chat?questions=t");
    match result {
        Err(e) => assert!(e.to_string().contains("questions"), "error: {e}"),
        Ok(_) => panic!("expected error for questions on llm:chat"),
    }
}

// -----------------------------------------------------------------------
// Generic provider configuration (llm-provider-extensibility 1.1)
// -----------------------------------------------------------------------

fn generic_config(service: LlmServiceKind) -> GenericProviderConfig {
    GenericProviderConfig {
        service,
        api_key: "test-key".into(),
        base_url: None,
        default_model: "explicit-model".into(),
        timeout_secs: None,
        max_concurrency: None,
        network_retry: None,
        pricing: None,
        cache_ttl_secs: None,
        cache_max_entries: None,
    }
}

fn generic_global_config(generic: GenericProviderConfig) -> LlmGlobalConfig {
    let mut providers = HashMap::new();
    providers.insert("generic-test".into(), LlmProviderConfig::Generic(generic));
    LlmGlobalConfig {
        providers,
        ..Default::default()
    }
}

#[test]
fn generic_services_parse_and_preserve_values() {
    let toml_str = r#"
[providers.anthropic-generic]
type = "generic"
service = "anthropic"
api_key = " test-key "
default_model = "explicit-model"

[providers.gemini-generic]
type = "generic"
service = "gemini"
api_key = " test-key "
default_model = "explicit-model"
"#;
    let cfg: LlmGlobalConfig = toml::from_str(toml_str).expect("parse");
    cfg.validate().expect("validate");

    assert_eq!(
        cfg.providers.len(),
        2,
        "both generic entries must be present"
    );

    for (name, expected) in [
        ("anthropic-generic", LlmServiceKind::Anthropic),
        ("gemini-generic", LlmServiceKind::Gemini),
    ] {
        let generic = match &cfg.providers[name] {
            LlmProviderConfig::Generic(c) => c,
            other => panic!("expected Generic for {name}, got {other:?}"),
        };
        assert_eq!(generic.service, expected);
        assert_eq!(generic.api_key, " test-key ", "key must stay untrimmed");
        assert_eq!(generic.default_model, "explicit-model");
        assert_eq!(generic.base_url, None);
        assert_eq!(generic.timeout_secs, None);
        assert_eq!(generic.max_concurrency, None);
        assert_eq!(generic.network_retry, None);
        assert!(generic.pricing.is_none());
        assert_eq!(generic.cache_ttl_secs, None);
        assert_eq!(generic.cache_max_entries, None);
    }
}

#[test]
fn generic_unknown_service_is_actionable() {
    let toml_str = r#"
[providers.typo]
type = "generic"
service = "anthropci"
api_key = "test-key"
default_model = "explicit-model"
"#;
    let err = toml::from_str::<LlmGlobalConfig>(toml_str).expect_err("unknown service must fail");
    let msg = err.to_string();
    assert!(
        msg.contains("anthropci"),
        "must name the rejected service: {msg}"
    );
    assert!(
        msg.contains("anthropic"),
        "must list supported anthropic: {msg}"
    );
    assert!(msg.contains("gemini"), "must list supported gemini: {msg}");
}

#[test]
fn generic_required_fields_are_enforced() {
    let missing_service = r#"
[providers.missing-service]
type = "generic"
api_key = "test-key"
default_model = "explicit-model"
"#;
    let err =
        toml::from_str::<LlmGlobalConfig>(missing_service).expect_err("missing service must fail");
    assert!(err.to_string().contains("service"), "error: {err}");

    let missing_key = r#"
[providers.missing-key]
type = "generic"
service = "anthropic"
default_model = "explicit-model"
"#;
    let err =
        toml::from_str::<LlmGlobalConfig>(missing_key).expect_err("missing api_key must fail");
    assert!(err.to_string().contains("api_key"), "error: {err}");

    let missing_model = r#"
[providers.missing-model]
type = "generic"
service = "gemini"
api_key = "test-key"
"#;
    let err = toml::from_str::<LlmGlobalConfig>(missing_model)
        .expect_err("missing default_model must fail");
    assert!(err.to_string().contains("default_model"), "error: {err}");

    for (api_key, default_model, expected_field, forbidden) in [
        ("", "explicit-model", "api_key", "explicit-model"),
        ("  ", "explicit-model", "api_key", "explicit-model"),
        ("super-secret-key", "", "default_model", "super-secret-key"),
        (
            "super-secret-key",
            "  ",
            "default_model",
            "super-secret-key",
        ),
    ] {
        let cfg = generic_global_config(GenericProviderConfig {
            api_key: api_key.into(),
            default_model: default_model.into(),
            ..generic_config(LlmServiceKind::Anthropic)
        });
        let err = cfg.validate().expect_err("blank content must be rejected");
        assert!(
            matches!(&err, CamelError::Config(_)),
            "expected Config error, got: {err}"
        );
        let msg = err.to_string();
        assert!(
            msg.contains(expected_field),
            "must name {expected_field}: {msg}"
        );
        assert!(
            !msg.contains(forbidden),
            "must not expose {forbidden}: {msg}"
        );
    }
}

#[test]
fn generic_debug_redacts_credentials_and_url() {
    let generic = GenericProviderConfig {
        service: LlmServiceKind::Gemini,
        api_key: "generic-secret-marker".into(),
        base_url: Some(
            "https://user:password-marker@example.com/v1?api_key=query-secret-marker".into(),
        ),
        ..generic_config(LlmServiceKind::Gemini)
    };
    let nested_debug = format!("{generic:?}");
    let enum_debug = format!("{:?}", LlmProviderConfig::Generic(generic.clone()));
    let global_debug = format!("{:?}", generic_global_config(generic));

    for rendered in [&nested_debug, &enum_debug, &global_debug] {
        for secret in [
            "generic-secret-marker",
            "password-marker",
            "query-secret-marker",
            "user:password-marker",
        ] {
            assert!(!rendered.contains(secret), "leaked {secret}: {rendered}");
        }
        assert!(
            rendered.contains("[REDACTED]"),
            "key redaction missing: {rendered}"
        );
        assert!(
            rendered.contains("***@") || rendered.contains("[redacted]"),
            "URL redaction missing: {rendered}"
        );
    }
}

#[test]
fn generic_policy_fields_round_trip_and_validate() {
    let toml_str = r#"
[providers.generic-anthropic]
type = "generic"
service = "anthropic"
api_key = "test-key"
default_model = "explicit-model"
timeout_secs = 7
max_concurrency = 2
cache_ttl_secs = 9
cache_max_entries = 4
network_retry = { enabled = true, max_attempts = 3, initial_delay_ms = 1, max_delay_ms = 5, multiplier = 2.0, jitter_factor = 0.0 }
pricing = { input_per_1k_tokens = 0.01, output_per_1k_tokens = 0.02 }

[providers.generic-gemini]
type = "generic"
service = "gemini"
api_key = "test-key"
default_model = "explicit-model"
timeout_secs = 7
max_concurrency = 2
cache_ttl_secs = 9
cache_max_entries = 4
network_retry = { enabled = true, max_attempts = 3, initial_delay_ms = 1, max_delay_ms = 5, multiplier = 2.0, jitter_factor = 0.0 }
pricing = { input_per_1k_tokens = 0.01, output_per_1k_tokens = 0.02 }
"#;
    let cfg: LlmGlobalConfig = toml::from_str(toml_str).expect("parse");
    cfg.validate().expect("valid policy fields");

    for name in ["generic-anthropic", "generic-gemini"] {
        let provider = &cfg.providers[name];
        assert_eq!(provider.timeout_secs(), Some(7));
        assert_eq!(provider.max_concurrency(), Some(2));
        let retry = provider.network_retry().expect("retry policy");
        assert!(retry.enabled);
        assert_eq!(retry.max_attempts, 3);
        assert_eq!(retry.initial_delay, std::time::Duration::from_millis(1));
        assert_eq!(retry.max_delay, std::time::Duration::from_millis(5));
        assert_eq!(retry.multiplier, 2.0);
        assert_eq!(retry.jitter_factor, 0.0);
        assert_eq!(retry.max_attempts_absolute, None);
        let pricing = provider.pricing().expect("pricing");
        assert_eq!(pricing.input_per_1k_tokens, 0.01);
        assert_eq!(pricing.output_per_1k_tokens, 0.02);
        assert_eq!(provider.cache_ttl_secs(), Some(9));
        assert_eq!(provider.cache_max_entries(), Some(4));
    }

    for service in [LlmServiceKind::Anthropic, LlmServiceKind::Gemini] {
        let assert_rejected = |generic: GenericProviderConfig, field: &str| {
            let err = generic_global_config(generic)
                .validate()
                .expect_err("invalid generic policy must be rejected");
            assert!(
                matches!(&err, CamelError::Config(_)),
                "expected Config error, got: {err}"
            );
            assert!(err.to_string().contains(field), "must name {field}: {err}");
        };

        let mut config = generic_config(service);
        config.timeout_secs = Some(0);
        assert_rejected(config, "timeout_secs");

        let mut config = generic_config(service);
        config.max_concurrency = Some(0);
        assert_rejected(config, "max_concurrency");

        let mut config = generic_config(service);
        config.cache_ttl_secs = Some(0);
        assert_rejected(config, "cache_ttl_secs");

        let mut config = generic_config(service);
        config.cache_max_entries = Some(0);
        assert_rejected(config, "cache_max_entries");

        let mut config = generic_config(service);
        config.pricing = Some(PricingTable {
            input_per_1k_tokens: -1.0,
            output_per_1k_tokens: 0.02,
        });
        assert_rejected(config, "pricing");

        let mut config = generic_config(service);
        config.pricing = Some(PricingTable {
            input_per_1k_tokens: 0.01,
            output_per_1k_tokens: -1.0,
        });
        assert_rejected(config, "pricing");
    }
}
