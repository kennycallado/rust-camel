use std::collections::HashMap;
use std::sync::Arc;
#[cfg(any(feature = "openai", feature = "ollama", feature = "all-providers"))]
use std::time::Duration;

use crate::config::{LlmGlobalConfig, LlmProviderConfig};
use crate::error::LlmError;
use crate::provider::LlmProvider;
use crate::provider::mock::{MockMode, MockProvider};

pub type ProviderMap = HashMap<String, Arc<dyn LlmProvider>>;

pub fn build_provider_map(config: &LlmGlobalConfig) -> Result<ProviderMap, LlmError> {
    let mut map = HashMap::new();
    let global_timeout = config.timeout_secs;
    for (name, provider_config) in &config.providers {
        let provider = build_single(name, provider_config, global_timeout).map_err(|e| {
            // log-policy: system-broken
            tracing::error!(provider = %name, error = %e, "failed to build llm provider");
            e
        })?;
        map.insert(name.clone(), provider);
    }
    Ok(map)
}

/// Validate a provider `base_url` before constructing the underlying client.
///
/// H15: rejects non-http(s) schemes AND resolves DNS to block
/// private/loopback/link-local IPs via the shared `is_ssrf_blocked_ip`
/// helper. Prevents SSRF to cloud metadata endpoints (e.g. 169.254.169.254).
///
/// DNS resolution is **fail-closed**: if the host cannot be resolved,
/// validation fails. An unresolvable host is treated as a configuration
/// error — silently passing would let typos, hijacked DNS, or
/// firewalled-internal names reach the outbound client.
pub fn validate_llm_url(url: &str) -> Result<(), LlmError> {
    let parsed = url::Url::parse(url)
        .map_err(|e| LlmError::InvalidRequest(format!("invalid llm base_url '{url}': {e}")))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(LlmError::InvalidRequest(format!(
            "llm base_url must use http/https, got: {}",
            parsed.scheme()
        )));
    }
    // SSRF: resolve host and reject blocked IPs. Fail-closed on resolution
    // failure so a misconfigured or non-resolvable host never silently
    // bypasses the SSRF guard.
    if let Some(host) = parsed.host_str() {
        use std::net::ToSocketAddrs;
        let addrs = (host, 0u16).to_socket_addrs().map_err(|e| {
            LlmError::InvalidRequest(format!("cannot resolve llm base_url host '{host}': {e}"))
        })?;
        for addr in addrs {
            if camel_api::is_ssrf_blocked_ip(&addr.ip()) {
                return Err(LlmError::InvalidRequest(format!(
                    "llm base_url resolves to blocked SSRF address: {}",
                    addr.ip()
                )));
            }
        }
    }
    Ok(())
}

fn build_single(
    name: &str,
    config: &LlmProviderConfig,
    #[allow(unused_variables)] global_timeout: Option<u64>,
) -> Result<Arc<dyn LlmProvider>, LlmError> {
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
        LlmProviderConfig::Openai(c) => {
            if let Some(ref base_url) = c.base_url {
                validate_llm_url(base_url)?;
            }
            let configured_timeout =
                Duration::from_secs(c.timeout_secs.or(global_timeout).unwrap_or(30));
            crate::provider::siumai_adapter::build_openai(name, c, configured_timeout)
                .map(|p| p as Arc<dyn LlmProvider>)
        }
        #[cfg(not(any(feature = "openai", feature = "all-providers")))]
        LlmProviderConfig::Openai(_) => Err(LlmError::UnsupportedCapability(
            "OpenAI provider requires the 'openai' feature flag".into(),
        )),
        #[cfg(any(feature = "ollama", feature = "all-providers"))]
        LlmProviderConfig::Ollama(c) => {
            validate_llm_url(&c.base_url)?;
            let configured_timeout =
                Duration::from_secs(c.timeout_secs.or(global_timeout).unwrap_or(30));
            crate::provider::siumai_adapter::build_ollama(name, c, configured_timeout)
                .map(|p| p as Arc<dyn LlmProvider>)
        }
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
        let provider = build_single("test", &config, None).expect("build ok");
        assert_eq!(provider.id(), "test");
    }

    // -----------------------------------------------------------------------
    // H15: validate_llm_url
    // -----------------------------------------------------------------------

    #[test]
    fn validate_llm_url_accepts_http() {
        // IP literal so the test is independent of DNS. Public IP — must
        // pass the SSRF guard.
        validate_llm_url("http://1.1.1.1").expect("public http IP literal must pass");
    }

    #[test]
    fn validate_llm_url_accepts_https() {
        validate_llm_url("https://1.1.1.1").expect("public https IP literal must pass");
    }

    #[test]
    fn validate_llm_url_rejects_unresolvable_host() {
        // Fail-closed on DNS resolution failure: a non-resolvable host
        // is treated as a config error, not a silent pass-through.
        let err = validate_llm_url("https://no-such-host.invalid").unwrap_err();
        assert!(
            err.to_string().contains("cannot resolve"),
            "expected DNS error, got: {err}"
        );
    }

    #[test]
    fn validate_llm_url_rejects_ftp_scheme() {
        let err = validate_llm_url("ftp://api.example.com").unwrap_err();
        assert!(matches!(err, LlmError::InvalidRequest(_)), "got: {err}");
        assert!(err.to_string().contains("http/https"), "msg: {err}");
    }

    #[test]
    fn validate_llm_url_rejects_file_scheme() {
        let err = validate_llm_url("file:///etc/passwd").unwrap_err();
        assert!(err.to_string().contains("http/https"), "msg: {err}");
    }

    #[test]
    fn validate_llm_url_rejects_unparseable() {
        let err = validate_llm_url("not a url").unwrap_err();
        assert!(err.to_string().contains("invalid"), "msg: {err}");
    }

    #[cfg(any(feature = "openai", feature = "all-providers"))]
    #[test]
    fn build_openai_rejects_non_http_base_url() {
        use crate::config::OpenaiProviderConfig;
        let config = LlmProviderConfig::Openai(OpenaiProviderConfig {
            api_key: "sk-test".into(),
            base_url: Some("ftp://api.example.com".into()),
            default_model: "gpt-4o".into(),
            timeout_secs: None,
            max_concurrency: None,
            network_retry: None,
            pricing: None,
            cache_ttl_secs: None,
            cache_max_entries: None,
        });
        let result = build_single("oai", &config, None);
        assert!(result.is_err(), "expected Err for ftp:// base_url");
        let err = result.err().expect("is_err above"); // allow-unwrap
        assert!(err.to_string().contains("http/https"), "msg: {err}");
    }

    #[cfg(any(feature = "ollama", feature = "all-providers"))]
    #[test]
    fn build_ollama_rejects_non_http_base_url() {
        use crate::config::OllamaProviderConfig;
        let config = LlmProviderConfig::Ollama(OllamaProviderConfig {
            base_url: "file:///tmp/ollama".into(),
            default_model: "llama3".into(),
            timeout_secs: None,
            max_concurrency: None,
            network_retry: None,
            pricing: None,
            cache_ttl_secs: None,
            cache_max_entries: None,
        });
        let result = build_single("ol", &config, None);
        assert!(result.is_err(), "expected Err for file:// base_url");
        let err = result.err().expect("is_err above"); // allow-unwrap
        assert!(err.to_string().contains("http/https"), "msg: {err}");
    }
}
