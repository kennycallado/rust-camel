use std::collections::HashMap;
use std::sync::Arc;

use crate::config::{LlmGlobalConfig, LlmProviderConfig};
use crate::error::LlmError;
use crate::provider::LlmProvider;
use crate::provider::mock::{MockMode, MockProvider};

pub type ProviderMap = HashMap<String, Arc<dyn LlmProvider>>;

pub fn build_provider_map(config: &LlmGlobalConfig) -> Result<ProviderMap, LlmError> {
    let mut map = HashMap::new();
    for (name, provider_config) in &config.providers {
        let provider = build_single(name, provider_config).map_err(|e| {
            // log-policy: system-broken
            tracing::error!(provider = %name, error = %e, "failed to build llm provider");
            e
        })?;
        map.insert(name.clone(), provider);
    }
    Ok(map)
}

fn build_single(name: &str, config: &LlmProviderConfig) -> Result<Arc<dyn LlmProvider>, LlmError> {
    match config {
        LlmProviderConfig::Mock(c) => {
            let mode = if let Some(ref msg) = c.error_message {
                MockMode::Error(LlmError::provider(msg))
            } else {
                parse_mock_mode(&c.response)
            };
            Ok(Arc::new(
                MockProvider::new(name, mode).with_model(&c.default_model),
            ))
        }
        #[cfg(any(feature = "openai", feature = "all-providers"))]
        LlmProviderConfig::Openai(c) => crate::provider::siumai_adapter::build_openai(name, c)
            .map(|p| p as Arc<dyn LlmProvider>),
        #[cfg(not(any(feature = "openai", feature = "all-providers")))]
        LlmProviderConfig::Openai(_) => Err(LlmError::UnsupportedCapability(
            "OpenAI provider requires the 'openai' feature flag".into(),
        )),
        #[cfg(any(feature = "ollama", feature = "all-providers"))]
        LlmProviderConfig::Ollama(c) => crate::provider::siumai_adapter::build_ollama(name, c)
            .map(|p| p as Arc<dyn LlmProvider>),
        #[cfg(not(any(feature = "ollama", feature = "all-providers")))]
        LlmProviderConfig::Ollama(_) => Err(LlmError::UnsupportedCapability(
            "Ollama provider requires the 'ollama' feature flag".into(),
        )),
    }
}

fn parse_mock_mode(response: &str) -> MockMode {
    if response == "echo" {
        MockMode::Echo
    } else if let Some(fixed) = response.strip_prefix("fixed:") {
        MockMode::Fixed(fixed.into())
    } else {
        MockMode::Fixed(response.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::MockProviderConfig;

    #[test]
    fn builds_mock_provider() {
        let mut providers = HashMap::new();
        providers.insert(
            "test".into(),
            LlmProviderConfig::Mock(MockProviderConfig {
                response: "echo".into(),
                default_model: "mock-model".into(),
                error_message: None,
            }),
        );
        let global = LlmGlobalConfig {
            providers,
            ..Default::default()
        };

        let map = build_provider_map(&global).expect("build ok");
        assert_eq!(map.len(), 1);
        assert!(map.contains_key("test"));
    }

    #[test]
    fn build_with_no_providers_returns_empty_map() {
        let global = LlmGlobalConfig::default();
        let map = build_provider_map(&global).expect("build ok");
        assert!(map.is_empty());
    }

    #[test]
    fn parse_mock_mode_echo() {
        let mode = parse_mock_mode("echo");
        assert!(matches!(mode, MockMode::Echo));
    }

    #[test]
    fn parse_mock_mode_fixed_prefix() {
        let mode = parse_mock_mode("fixed:canned response");
        assert!(matches!(mode, MockMode::Fixed(ref t) if t == "canned response"));
    }

    #[test]
    fn parse_mock_mode_fallback() {
        let mode = parse_mock_mode("some random text");
        assert!(matches!(mode, MockMode::Fixed(ref t) if t == "some random text"));
    }

    #[test]
    fn build_single_with_error_message_creates_error_mode() {
        let config = LlmProviderConfig::Mock(MockProviderConfig {
            response: "echo".into(),
            default_model: "mock-model".into(),
            error_message: Some("boom".into()),
        });
        let provider = build_single("test", &config).expect("build ok");
        assert_eq!(provider.id(), "test");
    }
}
