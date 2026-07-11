use camel_bridge::spec::XML_BRIDGE;
use camel_component_api::{CamelError, NetworkRetryPolicy};
use std::path::PathBuf;
use std::time::Duration;

fn xslt_reconnect_default() -> NetworkRetryPolicy {
    NetworkRetryPolicy {
        enabled: true,
        max_attempts: 2, // old max_retries=1 → 1 initial + 1 retry = 2 total
        initial_delay: Duration::from_millis(100),
        multiplier: 2.0,
        max_delay: Duration::from_secs(30),
        jitter_factor: 0.0,
    }
}

#[derive(Debug, Clone)]
pub struct XsltComponentConfig {
    pub bridge_binary_path: Option<PathBuf>,
    pub bridge_start_timeout_ms: u64,
    pub bridge_version: String,
    pub bridge_cache_dir: PathBuf,
    pub reconnect: NetworkRetryPolicy,
}

impl Default for XsltComponentConfig {
    fn default() -> Self {
        Self {
            bridge_binary_path: None,
            bridge_start_timeout_ms: 30_000,
            bridge_version: crate::BRIDGE_VERSION.to_string(),
            bridge_cache_dir: camel_bridge::download::default_cache_dir_for_spec(&XML_BRIDGE),
            reconnect: xslt_reconnect_default(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct XsltEndpointConfig {
    pub stylesheet_uri: String,
    /// Stylesheet parameters passed to the XSLT transformer at evaluation time.
    ///
    /// Parameters are specified in the endpoint URI as `param.<name>=<value>` pairs
    /// (e.g. `xslt:/transform.xslt?param.mode=debug&param.lang=en`). They are
    /// forwarded as string key-value pairs to the XSLT engine, where they become
    /// available as global `<xsl:param>` values inside the stylesheet.
    ///
    /// # Example
    ///
    /// URI: `xslt:/my.xslt?param.title=Hello&param.version=2`
    ///
    /// In the stylesheet:
    /// ```xml
    /// <xsl:param name="title"/>
    /// <xsl:param name="version"/>
    /// <output title="{$title}" version="{$version}"/>
    /// ```
    pub params: Vec<(String, String)>,
    pub output_method: Option<String>,
    /// Maximum number of compiled stylesheets to keep in the transformer cache.
    /// A value of `None` means unlimited (use bridge default), while `Some(0)`
    /// disables caching entirely.
    pub transformer_cache_size: Option<usize>,
    /// When `true`, the producer returns an error if the incoming exchange body
    /// is empty (no XML payload). When `false` (default), an empty body is
    /// forwarded to the bridge as-is.
    pub fail_on_null_body: bool,
    /// Maximum payload size in bytes accepted from the incoming exchange body.
    /// `None` uses the global default (`DEFAULT_MATERIALIZE_LIMIT`). `Some(0)` is rejected.
    pub max_payload_bytes: Option<usize>,
}

impl XsltEndpointConfig {
    pub fn from_uri(uri: &str) -> Result<Self, CamelError> {
        let rest = uri.strip_prefix("xslt:").ok_or_else(|| {
            CamelError::EndpointCreationFailed("expected 'xslt:' URI scheme".to_string())
        })?;

        let (stylesheet_uri, query) = match rest.split_once('?') {
            Some((path, q)) => (path, Some(q)),
            None => (rest, None),
        };

        if stylesheet_uri.is_empty() {
            return Err(CamelError::EndpointCreationFailed(
                "stylesheet path cannot be empty".to_string(),
            ));
        }

        // Strip file:// prefix so read_stylesheet receives a plain path
        let stylesheet_path = if stylesheet_uri.starts_with("file://") {
            let stripped = stylesheet_uri
                .strip_prefix("file://")
                .expect("prefix checked above"); // allow-unwrap
            if stripped.is_empty() {
                return Err(CamelError::EndpointCreationFailed(
                    "stylesheet path is empty after stripping file:// prefix".to_string(),
                ));
            }
            stripped.to_string()
        } else {
            stylesheet_uri.to_string()
        };

        let mut params = Vec::new();
        let mut output_method = None;
        let mut transformer_cache_size = None;
        let mut fail_on_null_body = false;
        let mut max_payload_bytes = None;

        if let Some(q) = query {
            for (key, value) in url::form_urlencoded::parse(q.as_bytes()) {
                if key == "output" {
                    output_method = Some(value.into_owned());
                    continue;
                }

                if key == "transformerCacheSize" {
                    match value.parse::<usize>() {
                        Ok(n) => transformer_cache_size = Some(n),
                        Err(_) => {
                            return Err(CamelError::EndpointCreationFailed(format!(
                                "invalid transformerCacheSize value: {value}"
                            )));
                        }
                    }
                    continue;
                }

                if key == "failOnNullBody" {
                    fail_on_null_body = value == "true";
                    continue;
                }

                if key == "maxPayloadBytes" {
                    match value.parse::<usize>() {
                        Ok(0) => {
                            return Err(CamelError::EndpointCreationFailed(
                                "maxPayloadBytes must be greater than 0".to_string(),
                            ));
                        }
                        Ok(n) => max_payload_bytes = Some(n),
                        Err(_) => {
                            return Err(CamelError::EndpointCreationFailed(format!(
                                "invalid maxPayloadBytes value: {value}"
                            )));
                        }
                    }
                    continue;
                }

                if let Some(param_key) = key.strip_prefix("param.")
                    && !param_key.is_empty()
                {
                    params.push((param_key.to_string(), value.into_owned()));
                }
            }
        }

        Ok(Self {
            stylesheet_uri: stylesheet_path,
            params,
            output_method,
            transformer_cache_size,
            fail_on_null_body,
            max_payload_bytes,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_uri_with_output_and_params() {
        let cfg = XsltEndpointConfig::from_uri("xslt:/tmp/a.xslt?output=xml&param.a=1").unwrap();
        assert_eq!(cfg.stylesheet_uri, "/tmp/a.xslt");
        assert_eq!(cfg.output_method, Some("xml".to_string()));
        assert_eq!(cfg.params, vec![("a".to_string(), "1".to_string())]);
        assert_eq!(cfg.transformer_cache_size, None);
        assert!(!cfg.fail_on_null_body);
    }

    #[test]
    fn parses_uri_options() {
        let cfg = XsltEndpointConfig::from_uri(
            "xslt:/tmp/a.xslt?transformerCacheSize=64&failOnNullBody=true",
        )
        .unwrap();
        assert_eq!(cfg.transformer_cache_size, Some(64));
        assert!(cfg.fail_on_null_body);
    }

    #[test]
    fn strips_file_prefix() {
        let cfg = XsltEndpointConfig::from_uri("xslt:file:///tmp/a.xslt").unwrap();
        assert_eq!(cfg.stylesheet_uri, "/tmp/a.xslt");
    }

    #[test]
    fn rejects_empty_path_after_file_prefix() {
        let err = XsltEndpointConfig::from_uri("xslt:file://").unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("empty"),
            "expected empty-path error, got: {msg}"
        );
    }

    #[test]
    fn rejects_invalid_cache_size() {
        let err =
            XsltEndpointConfig::from_uri("xslt:/tmp/a.xslt?transformerCacheSize=abc").unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("transformerCacheSize"),
            "expected cache-size error, got: {msg}"
        );
    }

    #[test]
    fn fail_on_null_body_defaults_false() {
        let cfg = XsltEndpointConfig::from_uri("xslt:/tmp/a.xslt").unwrap();
        assert!(!cfg.fail_on_null_body);
    }

    #[test]
    fn parses_max_payload_bytes() {
        let cfg = XsltEndpointConfig::from_uri("xslt:/tmp/a.xslt?maxPayloadBytes=2048").unwrap();
        assert_eq!(cfg.max_payload_bytes, Some(2048));
    }

    #[test]
    fn max_payload_bytes_defaults_none() {
        let cfg = XsltEndpointConfig::from_uri("xslt:/tmp/a.xslt").unwrap();
        assert_eq!(cfg.max_payload_bytes, None);
    }

    #[test]
    fn rejects_zero_max_payload_bytes() {
        let err = XsltEndpointConfig::from_uri("xslt:/tmp/a.xslt?maxPayloadBytes=0").unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("maxPayloadBytes"),
            "expected maxPayloadBytes error, got: {msg}"
        );
    }

    #[test]
    fn rejects_invalid_max_payload_bytes() {
        let err = XsltEndpointConfig::from_uri("xslt:/tmp/a.xslt?maxPayloadBytes=abc").unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("maxPayloadBytes"),
            "expected maxPayloadBytes error, got: {msg}"
        );
    }

    #[test]
    fn default_component_cache_dir_uses_xml_bridge() {
        let cfg = XsltComponentConfig::default();
        assert!(
            cfg.bridge_cache_dir.ends_with("xml-bridge"),
            "expected xml bridge cache dir, got {}",
            cfg.bridge_cache_dir.display()
        );
        assert!(
            !cfg.bridge_cache_dir.ends_with("jms-bridge"),
            "XSLT must not use JMS bridge cache dir"
        );
    }

    #[test]
    fn xslt_config_has_reconnect_policy() {
        let cfg = XsltComponentConfig::default();
        // Old max_retries=1 mapped to max_attempts=2 (1 initial + 1 retry).
        assert!(cfg.reconnect.enabled);
        assert_eq!(cfg.reconnect.max_attempts, 2);
        assert_eq!(cfg.reconnect.initial_delay, Duration::from_millis(100));
        assert_eq!(cfg.reconnect.multiplier, 2.0f64);
        assert_eq!(cfg.reconnect.max_delay, Duration::from_secs(30));
        assert_eq!(cfg.reconnect.jitter_factor, 0.0f64);
    }
}
