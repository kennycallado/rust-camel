//! Builds the named decision provider map from [`LlmGlobalConfig`].

use std::collections::HashMap;
use std::sync::Arc;

#[cfg(feature = "decisions")]
use camel_api::SsrfPolicy;

use crate::config::LlmGlobalConfig;
use crate::decision::DecisionProvider;
use crate::decision::config::DecisionProviderConfig;
use crate::decision::mock::MockDecisionProvider;
#[cfg(feature = "decisions")]
use crate::decision::openai::OpenaiDecisionProvider;
#[cfg(feature = "decisions")]
use crate::decision::system_one::SystemOneDecisionProvider;
use crate::error::LlmError;
#[cfg(feature = "decisions")]
use crate::provider_factory;

/// Decision providers keyed by their configured name.
pub type DecisionProviderMap = HashMap<String, Arc<dyn DecisionProvider>>;

/// Builds one decision provider per `[decision_providers.*]` entry.
pub fn build_decision_provider_map(
    config: &LlmGlobalConfig,
) -> Result<DecisionProviderMap, LlmError> {
    let mut map = HashMap::new();
    for (name, provider_config) in &config.decision_providers {
        let provider = build_single(name, provider_config, config).map_err(|e| {
            // log-policy: system-broken
            tracing::error!(provider = %name, error = %e, "failed to build decision provider");
            e
        })?;
        map.insert(name.clone(), provider);
    }
    Ok(map)
}

/// SSRF policy for HTTP decision providers, derived from `allow_internal`.
#[cfg(feature = "decisions")]
fn ssrf_policy(config: &LlmGlobalConfig) -> SsrfPolicy {
    if config.allow_internal {
        SsrfPolicy::AllowInternal
    } else {
        SsrfPolicy::PublicHttpsOnly
    }
}

fn build_single(
    name: &str,
    cfg: &DecisionProviderConfig,
    #[cfg_attr(not(feature = "decisions"), allow(unused_variables))] config: &LlmGlobalConfig,
) -> Result<Arc<dyn DecisionProvider>, LlmError> {
    match cfg {
        DecisionProviderConfig::Mock(mock) => {
            Ok(Arc::new(MockDecisionProvider::new(name, mock.clone())))
        }
        #[cfg(feature = "decisions")]
        DecisionProviderConfig::Openai(c) => {
            let policy = ssrf_policy(config);
            if let Some(base) = &c.base_url {
                provider_factory::validate_llm_url(base, policy)?;
            }
            Ok(Arc::new(OpenaiDecisionProvider::new(
                name,
                c,
                policy,
                config.timeout_secs,
            )))
        }
        #[cfg(feature = "decisions")]
        DecisionProviderConfig::Typesafe(c) => {
            let policy = ssrf_policy(config);
            if let Some(base) = &c.base_url {
                provider_factory::validate_llm_url(base, policy)?;
            }
            Ok(Arc::new(SystemOneDecisionProvider::new_typesafe(
                name,
                c,
                policy,
                config.timeout_secs,
            )))
        }
        #[cfg(feature = "decisions")]
        DecisionProviderConfig::Ollama(c) => {
            let policy = ssrf_policy(config);
            provider_factory::validate_llm_url(&c.base_url, policy)?;
            Ok(Arc::new(SystemOneDecisionProvider::new_ollama(
                name,
                c,
                policy,
                config.timeout_secs,
            )))
        }
        #[cfg(not(feature = "decisions"))]
        DecisionProviderConfig::Openai(_)
        | DecisionProviderConfig::Typesafe(_)
        | DecisionProviderConfig::Ollama(_) => Err(LlmError::UnsupportedCapability(format!(
            "decision provider '{name}' of type '{}' requires the 'decisions' feature",
            cfg.type_name()
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decision::config::DecisionProviderConfig;
    use crate::decision::config::MockDecisionConfig;

    fn mock_config() -> DecisionProviderConfig {
        DecisionProviderConfig::Mock(MockDecisionConfig {
            default_model: "mock-decision".to_string(),
            predicate_probability: 1.0,
            refuse: vec![],
            error_message: None,
        })
    }

    #[test]
    fn factory_builds_mock() {
        let mut config = LlmGlobalConfig::default();
        config
            .decision_providers
            .insert("triage-mock".to_string(), mock_config());
        let map = build_decision_provider_map(&config).expect("mock should build");
        let key = "triage-mock";
        assert!(map.contains_key(key), "map should contain {key}");
        assert_eq!(map[key].id(), key);
    }

    #[cfg(not(feature = "decisions"))]
    #[test]
    fn factory_rejects_openai_without_feature() {
        use crate::decision::config::OpenaiDecisionConfig;

        let mut config = LlmGlobalConfig::default();
        config.decision_providers.insert(
            "live".to_string(),
            DecisionProviderConfig::Openai(OpenaiDecisionConfig {
                api_key: "k".to_string(),
                default_model: "m".to_string(),
                base_url: None,
                timeout_secs: None,
                max_concurrency: None,
                network_retry: None,
            }),
        );
        match build_decision_provider_map(&config) {
            Err(LlmError::UnsupportedCapability(m)) => {
                assert!(m.contains("decisions"), "message was {m}")
            }
            Err(other) => panic!("expected UnsupportedCapability, got {other:?}"),
            Ok(_) => panic!("expected an error for openai without the decisions feature"),
        }
    }

    #[cfg(feature = "decisions")]
    #[test]
    fn factory_rejects_plain_http_public_typesafe() {
        use crate::decision::config::TypesafeDecisionConfig;

        let mut config = LlmGlobalConfig {
            allow_internal: false,
            ..Default::default()
        };
        config.decision_providers.insert(
            "triage".to_string(),
            DecisionProviderConfig::Typesafe(TypesafeDecisionConfig {
                api_key: "k".to_string(),
                default_model: "m".to_string(),
                base_url: Some("http://api.typesafe.ai".to_string()),
                timeout_secs: None,
                max_concurrency: None,
                network_retry: None,
            }),
        );
        match build_decision_provider_map(&config) {
            Err(LlmError::InvalidRequest(_)) => {}
            Err(other) => panic!("expected InvalidRequest, got {other:?}"),
            Ok(_) => panic!("plain http public typesafe must be rejected"),
        }
    }

    #[cfg(feature = "decisions")]
    #[test]
    fn factory_rejects_ollama_loopback_without_allow_internal() {
        use crate::decision::config::OllamaDecisionConfig;

        let mut config = LlmGlobalConfig {
            allow_internal: false,
            ..Default::default()
        };
        config.decision_providers.insert(
            "local".to_string(),
            DecisionProviderConfig::Ollama(OllamaDecisionConfig {
                base_url: "http://127.0.0.1:11434".to_string(),
                default_model: "m".to_string(),
                timeout_secs: None,
                max_concurrency: None,
                network_retry: None,
            }),
        );
        match build_decision_provider_map(&config) {
            Err(LlmError::InvalidRequest(_)) => {}
            Err(other) => panic!("expected InvalidRequest, got {other:?}"),
            Ok(_) => panic!("ollama loopback without allow_internal must be rejected"),
        }
    }

    #[cfg(feature = "decisions")]
    #[tokio::test]
    async fn factory_builds_all_types_with_allow_internal() {
        use crate::decision::config::OllamaDecisionConfig;
        use crate::decision::config::OpenaiDecisionConfig;
        use crate::decision::config::TypesafeDecisionConfig;

        let server = wiremock::MockServer::start().await;
        let base = server.uri();

        let mut config = LlmGlobalConfig {
            allow_internal: true,
            ..Default::default()
        };
        config.decision_providers.insert(
            "oai".to_string(),
            DecisionProviderConfig::Openai(OpenaiDecisionConfig {
                api_key: "k".to_string(),
                default_model: "m".to_string(),
                base_url: Some(base.clone()),
                timeout_secs: None,
                max_concurrency: None,
                network_retry: None,
            }),
        );
        config.decision_providers.insert(
            "ts".to_string(),
            DecisionProviderConfig::Typesafe(TypesafeDecisionConfig {
                api_key: "k".to_string(),
                default_model: "m".to_string(),
                base_url: Some(base.clone()),
                timeout_secs: None,
                max_concurrency: None,
                network_retry: None,
            }),
        );
        config.decision_providers.insert(
            "ol".to_string(),
            DecisionProviderConfig::Ollama(OllamaDecisionConfig {
                base_url: base.clone(),
                default_model: "m".to_string(),
                timeout_secs: None,
                max_concurrency: None,
                network_retry: None,
            }),
        );

        let map = build_decision_provider_map(&config)
            .expect("all decision types must build with allow_internal");
        assert_eq!(map.len(), 3, "expected 3 providers, got {}", map.len());
        assert_eq!(map["oai"].capabilities().max_questions, 400);
        assert_eq!(map["ts"].capabilities().max_questions, 400);
        assert_eq!(map["ol"].capabilities().max_questions, 64);

        let received = server.received_requests().await.unwrap_or_default();
        assert!(
            received.is_empty(),
            "build must be lazy, but wiremock got {} request(s)",
            received.len()
        );
    }
}
