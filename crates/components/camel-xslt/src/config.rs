use camel_component_api::CamelError;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct XsltComponentConfig {
    pub bridge_binary_path: Option<PathBuf>,
    pub bridge_start_timeout_ms: u64,
    pub bridge_version: String,
    pub bridge_cache_dir: PathBuf,
}

impl Default for XsltComponentConfig {
    fn default() -> Self {
        Self {
            bridge_binary_path: None,
            bridge_start_timeout_ms: 30_000,
            bridge_version: crate::BRIDGE_VERSION.to_string(),
            bridge_cache_dir: camel_bridge::download::default_cache_dir(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct XsltEndpointConfig {
    pub stylesheet_uri: String,
    pub params: Vec<(String, String)>,
    pub output_method: Option<String>,
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

        let mut params = Vec::new();
        let mut output_method = None;

        if let Some(q) = query {
            for (key, value) in url::form_urlencoded::parse(q.as_bytes()) {
                if key == "output" {
                    output_method = Some(value.into_owned());
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
            stylesheet_uri: stylesheet_uri.to_string(),
            params,
            output_method,
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
    }
}
