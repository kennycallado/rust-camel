//! Structural parity between the hand-maintained generic-provider JSON Schema
//! and the runtime serde configuration.
//!
//! The schema is read from the workspace at test runtime; it is never embedded
//! in library code and no JSON Schema validation engine is used here.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use camel_component_llm::{LlmGlobalConfig, LlmProviderConfig, LlmServiceKind};
use serde_json::{Value, json};

const GENERIC_PROVIDER_SCHEMA: &str = "../../../schemas/llm-generic-provider.schema.json";

fn load_schema() -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(GENERIC_PROVIDER_SCHEMA);
    let raw = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read schema {}: {error}", path.display()));
    serde_json::from_str(&raw).expect("generic provider schema is valid JSON")
}

fn sorted_strings(value: &Value) -> Vec<&str> {
    let mut values: Vec<&str> = value
        .as_array()
        .expect("expected a JSON array of strings")
        .iter()
        .map(|entry| entry.as_str().expect("expected a JSON string"))
        .collect();
    values.sort_unstable();
    values
}

fn generic_fixture(service: &str) -> Value {
    json!({
        "type": "generic",
        "service": service,
        "api_key": "test-key",
        "default_model": "explicit-model",
    })
}

fn parse_provider(fixture: &Value) -> Result<LlmProviderConfig, serde_json::Error> {
    serde_json::from_value(fixture.clone())
}

fn global_config(provider: LlmProviderConfig) -> LlmGlobalConfig {
    let mut providers = HashMap::new();
    providers.insert("generic-entry".to_owned(), provider);
    LlmGlobalConfig {
        providers,
        ..Default::default()
    }
}

#[test]
fn generic_schema_discriminants_match_runtime() {
    let schema = load_schema();

    assert_eq!(schema["type"].as_str(), Some("object"), "root is an entry");
    assert_eq!(
        schema["properties"]["type"]["const"].as_str(),
        Some("generic"),
        "the entry discriminator is the generic tag"
    );
    assert_eq!(
        sorted_strings(&schema["properties"]["service"]["enum"]),
        vec!["anthropic", "gemini"],
        "schema service allowlist must match the runtime selector"
    );
    assert_eq!(
        sorted_strings(&schema["required"]),
        vec!["api_key", "default_model", "service", "type"],
        "schema required fields must match runtime required fields"
    );

    for (service, expected) in [
        ("anthropic", LlmServiceKind::Anthropic),
        ("gemini", LlmServiceKind::Gemini),
    ] {
        match parse_provider(&generic_fixture(service)).expect("accepted service fixture") {
            LlmProviderConfig::Generic(config) => assert_eq!(config.service, expected),
            other => panic!("expected generic config for {service}, got {other:?}"),
        }
    }

    let unknown = generic_fixture("anthropci");
    let error = parse_provider(&unknown).expect_err("unknown service must fail");
    let message = error.to_string();
    assert!(
        message.contains("anthropci"),
        "error should echo the rejected service: {message}"
    );
    assert!(
        message.contains("anthropic") && message.contains("gemini"),
        "error should list supported services: {message}"
    );
}

#[test]
fn generic_schema_required_strings_match_runtime() {
    let schema = load_schema();

    for service in ["anthropic", "gemini"] {
        parse_provider(&generic_fixture(service))
            .unwrap_or_else(|error| panic!("valid {service} fixture must deserialize: {error}"));
    }

    for field in ["type", "service", "api_key", "default_model"] {
        let mut fixture = generic_fixture("anthropic");
        fixture
            .as_object_mut()
            .expect("fixture object")
            .remove(field);
        assert!(
            parse_provider(&fixture).is_err(),
            "omitting required field {field} must fail runtime deserialization"
        );
    }

    for field in ["api_key", "default_model"] {
        let mut fixture = generic_fixture("gemini");
        fixture[field] = json!("   ");
        let provider = parse_provider(&fixture).expect("blank strings still deserialize");
        let error = global_config(provider)
            .validate()
            .expect_err("blank required content must fail global validation");
        assert!(
            error.to_string().contains(field),
            "validation diagnostic should name {field}: {error}"
        );
    }

    for field in ["api_key", "default_model"] {
        let property = &schema["properties"][field];
        assert_eq!(
            property["type"].as_str(),
            Some("string"),
            "{field} must be a string"
        );
        assert_eq!(
            property["pattern"].as_str(),
            Some("\\S"),
            "{field} pattern must declare a non-whitespace regex (single decoded backslash)"
        );
    }
}

#[test]
fn generic_schema_policy_fields_match_runtime() {
    let fixture = json!({
        "type": "generic",
        "service": "anthropic",
        "api_key": "test-key",
        "default_model": "explicit-model",
        "timeout_secs": 7,
        "max_concurrency": 2,
        "network_retry": {
            "enabled": true,
            "max_attempts": 3,
            "initial_delay_ms": 1,
            "max_delay_ms": 5,
            "multiplier": 2.0,
            "jitter_factor": 0.0,
        },
        "pricing": {
            "input_per_1k_tokens": 0.01,
            "output_per_1k_tokens": 0.02,
        },
        "cache_ttl_secs": 9,
        "cache_max_entries": 4,
    });

    let provider = parse_provider(&fixture).expect("policy fixture deserializes");
    let LlmProviderConfig::Generic(config) = &provider else {
        panic!("expected generic config, got {provider:?}");
    };
    assert_eq!(config.service, LlmServiceKind::Anthropic);
    assert_eq!(provider.timeout_secs(), Some(7));
    assert_eq!(provider.max_concurrency(), Some(2));
    assert_eq!(provider.cache_ttl_secs(), Some(9));
    assert_eq!(provider.cache_max_entries(), Some(4));

    let retry = provider.network_retry().expect("retry policy");
    assert!(retry.enabled);
    assert_eq!(retry.max_attempts, 3);
    assert_eq!(retry.max_attempts_absolute, None);
    assert_eq!(retry.initial_delay, Duration::from_millis(1));
    assert_eq!(retry.max_delay, Duration::from_millis(5));
    assert_eq!(retry.multiplier, 2.0);
    assert_eq!(retry.jitter_factor, 0.0);

    let pricing = provider.pricing().expect("pricing table");
    assert!((pricing.input_per_1k_tokens - 0.01).abs() < 1e-12);
    assert!((pricing.output_per_1k_tokens - 0.02).abs() < 1e-12);

    let schema = load_schema();
    let properties = &schema["properties"];
    for field in [
        "timeout_secs",
        "max_concurrency",
        "cache_ttl_secs",
        "cache_max_entries",
    ] {
        assert_eq!(
            properties[field]["type"].as_str(),
            Some("integer"),
            "{field} must be an integer"
        );
        assert_eq!(
            properties[field]["minimum"].as_i64(),
            Some(1),
            "{field} must be positive"
        );
    }
    assert_eq!(
        properties["network_retry"]["type"].as_str(),
        Some("object"),
        "network_retry must be an object"
    );
    assert_eq!(
        properties["pricing"]["type"].as_str(),
        Some("object"),
        "pricing must be an object"
    );

    let retry_properties = &properties["network_retry"]["properties"];
    for field in [
        "enabled",
        "max_attempts",
        "max_attempts_absolute",
        "initial_delay_ms",
        "max_delay_ms",
        "multiplier",
        "jitter_factor",
    ] {
        assert!(
            retry_properties.get(field).is_some(),
            "network_retry must document serialized field {field}"
        );
    }
    assert_eq!(
        retry_properties["enabled"]["type"].as_str(),
        Some("boolean")
    );
    for field in ["max_attempts", "initial_delay_ms", "max_delay_ms"] {
        assert_eq!(
            retry_properties[field]["type"].as_str(),
            Some("integer"),
            "{field} must be an integer"
        );
        assert_eq!(
            retry_properties[field]["minimum"].as_i64(),
            Some(0),
            "{field} must be nonnegative"
        );
    }
    let absolute_types = sorted_strings(&retry_properties["max_attempts_absolute"]["type"]);
    assert_eq!(
        absolute_types,
        vec!["integer", "null"],
        "max_attempts_absolute must be a nullable integer"
    );
    assert_eq!(
        retry_properties["max_attempts_absolute"]["minimum"].as_i64(),
        Some(0),
        "max_attempts_absolute must be nonnegative"
    );
    for field in ["multiplier", "jitter_factor"] {
        assert_eq!(
            retry_properties[field]["type"].as_str(),
            Some("number"),
            "{field} must be a number"
        );
    }

    let pricing = &properties["pricing"];
    assert_eq!(
        sorted_strings(&pricing["required"]),
        vec!["input_per_1k_tokens", "output_per_1k_tokens"],
        "pricing must require both prices"
    );
    for field in ["input_per_1k_tokens", "output_per_1k_tokens"] {
        assert_eq!(
            pricing["properties"][field]["type"].as_str(),
            Some("number"),
            "{field} must be a number"
        );
        assert_eq!(
            pricing["properties"][field]["minimum"].as_f64(),
            Some(0.0),
            "{field} must be nonnegative"
        );
    }

    for field in [
        "timeout_secs",
        "max_concurrency",
        "cache_ttl_secs",
        "cache_max_entries",
    ] {
        let mut invalid = fixture.clone();
        invalid[field] = json!(0);
        let provider = parse_provider(&invalid).expect("zero limit still deserializes");
        let error = global_config(provider)
            .validate()
            .expect_err("zero limits must fail global validation");
        assert!(
            error.to_string().contains(field),
            "zero-limit diagnostic should name {field}: {error}"
        );
    }

    for field in ["input_per_1k_tokens", "output_per_1k_tokens"] {
        let mut invalid = fixture.clone();
        invalid["pricing"][field] = json!(-1.0);
        let provider = parse_provider(&invalid).expect("negative price still deserializes");
        let error = global_config(provider)
            .validate()
            .expect_err("negative pricing must fail global validation");
        assert!(
            error.to_string().contains("pricing"),
            "negative-pricing diagnostic should name pricing: {error}"
        );
    }
}

const LLM_GUIDE: &str = "../../../docs/src/components/llm.md";

fn guide_subsection<'a>(guide: &'a str, heading: &str) -> &'a str {
    let marker = format!("### {heading}");
    let start = guide
        .find(&marker)
        .unwrap_or_else(|| panic!("LLM guide is missing heading '{marker}'"));
    let after = &guide[start + marker.len()..];
    let end = [after.find("\n## "), after.find("\n### ")]
        .into_iter()
        .flatten()
        .min()
        .unwrap_or(after.len());
    &after[..end]
}

fn first_toml_block(section: &str) -> &str {
    let marker = "```toml\n";
    let start = section
        .find(marker)
        .unwrap_or_else(|| panic!("subsection has no fenced TOML example: {section}"));
    let after = &section[start + marker.len()..];
    let end = after
        .find("\n```")
        .unwrap_or_else(|| panic!("TOML example is not fenced: {after}"));
    &after[..end]
}

const LLM_README: &str = "README.md";

fn readme_quick_start_section(readme: &str) -> &str {
    let marker = "## Quick Start";
    let start = readme
        .find(marker)
        .unwrap_or_else(|| panic!("LLM README is missing heading '{marker}'"));
    let after = &readme[start + marker.len()..];
    let end = after.find("\n## ").unwrap_or(after.len());
    &after[..end]
}

fn readme_quick_start_toml(readme: &str) -> &str {
    first_toml_block(readme_quick_start_section(readme))
}

#[test]
fn readme_and_guide_use_bundle_config_prefix() {
    let readme_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(LLM_README);
    let readme = std::fs::read_to_string(&readme_path)
        .unwrap_or_else(|error| panic!("read LLM README {}: {error}", readme_path.display()));
    let quick_start = readme_quick_start_section(&readme);
    assert!(
        quick_start.contains("[components.llm]"),
        "README Quick Start must use the [components.llm] table read by the bundle loader"
    );
    assert!(
        !quick_start.contains("[llm]"),
        "README Quick Start must not use a top-level [llm] table"
    );

    let guide_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(LLM_GUIDE);
    let guide = std::fs::read_to_string(&guide_path)
        .unwrap_or_else(|error| panic!("read LLM guide {}: {error}", guide_path.display()));
    assert!(
        guide.contains("[components.llm.providers."),
        "LLM guide must show the [components.llm.providers.<name>] prefix used in Camel.toml"
    );
    assert!(
        !guide.contains("[llm.providers."),
        "LLM guide must not show a top-level [llm.providers...] table"
    );
}

fn collect_bare_placeholders(value: &toml::Value, path: &str, found: &mut Vec<String>) {
    match value {
        toml::Value::String(text) => {
            let mut rest = text.as_str();
            while let Some(index) = rest.find("${") {
                let token = &rest[index + 2..];
                if !(token.starts_with("env:") || token.starts_with("arg:")) {
                    found.push(format!("{path} = {text:?}"));
                }
                rest = &rest[index + 2..];
            }
        }
        toml::Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                collect_bare_placeholders(item, &format!("{path}[{index}]"), found);
            }
        }
        toml::Value::Table(table) => {
            for (key, item) in table {
                let child = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };
                collect_bare_placeholders(item, &child, found);
            }
        }
        _ => {}
    }
}

#[test]
fn readme_openai_example_has_no_bare_placeholder_expansion_claim() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(LLM_README);
    let readme = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read LLM README {}: {error}", path.display()));

    assert!(
        !readme.contains("${OPENAI_API_KEY}"),
        "README must not present a bare ${{VAR}} token as an expandable API key"
    );

    let block = readme_quick_start_toml(&readme);
    let value: toml::Value = toml::from_str(block).expect("README Quick Start TOML must parse");

    let mut bare = Vec::new();
    collect_bare_placeholders(&value, "components.llm", &mut bare);
    assert!(
        bare.is_empty(),
        "Quick Start TOML must not contain bare placeholders: {bare:?}"
    );
    assert_eq!(
        value["components"]["llm"]["providers"]["my-openai"]["api_key"].as_str(),
        Some("${env:OPENAI_API_KEY}"),
        "OpenAI example must use the ${{env:NAME}} grammar that Camel.toml resolves"
    );

    let section = readme_quick_start_section(&readme);
    assert!(
        section.contains("Camel.toml"),
        "Quick Start must name the surface that performs interpolation"
    );
    assert!(
        section.contains("does not expand placeholders"),
        "Quick Start must not imply that deserialization expands placeholders"
    );
}

#[test]
fn generic_documentation_examples_parse() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(LLM_GUIDE);
    let guide = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read LLM guide {}: {error}", path.display()));

    for (heading, expected) in [
        ("Anthropic generic provider", LlmServiceKind::Anthropic),
        ("Gemini generic provider", LlmServiceKind::Gemini),
    ] {
        let section = guide_subsection(&guide, heading);
        let block = first_toml_block(section);
        let config: LlmGlobalConfig = toml::from_str(block)
            .unwrap_or_else(|error| panic!("{heading} TOML example must parse: {error}"));
        config
            .validate()
            .unwrap_or_else(|error| panic!("{heading} TOML example must validate: {error}"));

        assert_eq!(
            config.providers.len(),
            1,
            "{heading} must show exactly one provider entry"
        );
        let (name, provider) = config
            .providers
            .iter()
            .next()
            .expect("one provider entry exists");
        assert!(!name.trim().is_empty(), "{heading} provider name is blank");

        let LlmProviderConfig::Generic(generic) = provider else {
            panic!("{heading} must use type = \"generic\", got {provider:?}");
        };
        assert_eq!(
            generic.service, expected,
            "{heading} must select service {:?}",
            expected
        );
        assert!(
            !generic.api_key.trim().is_empty(),
            "{heading} must show a nonblank explicit API key"
        );
        assert!(
            !generic.default_model.trim().is_empty(),
            "{heading} must show a nonblank explicit default model"
        );
        assert!(
            !block.contains("${"),
            "{heading} example must not hide environment expansion in the TOML"
        );
    }
}
