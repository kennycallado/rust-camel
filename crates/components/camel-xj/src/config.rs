use camel_component_api::CamelError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    XmlToJson,
    JsonToXml,
}

impl Direction {
    fn from_query(value: &str) -> Result<Self, CamelError> {
        match value {
            "xml2json" => Ok(Self::XmlToJson),
            "json2xml" => Ok(Self::JsonToXml),
            other => Err(CamelError::EndpointCreationFailed(format!(
                "unsupported direction '{other}', expected xml2json or json2xml"
            ))),
        }
    }
}

#[derive(Debug, Clone)]
pub struct XjEndpointConfig {
    pub stylesheet_uri: String,
    pub direction: Direction,
    pub params: Vec<(String, String)>,
    /// Maximum allowed payload size in bytes before sending to the bridge process.
    /// Requests exceeding this limit are rejected immediately.
    pub max_payload_bytes: Option<usize>,
    /// Transformation direction override: "XML2JSON" or "JSON2XML".
    /// When set, takes precedence over `direction`. Default: auto-detect from content.
    pub transform_direction: Option<String>,
    /// Additional XSLT resource URI for transformation.
    /// Default: None (no additional stylesheet applied beyond the primary one).
    pub resource_uri: Option<String>,
    pub retry_count: u32,
    pub retry_delay_ms: u64,
}

impl XjEndpointConfig {
    pub fn from_uri(uri: &str) -> Result<Self, CamelError> {
        let rest = uri.strip_prefix("xj:").ok_or_else(|| {
            CamelError::EndpointCreationFailed("expected 'xj:' URI scheme".to_string())
        })?;

        let (stylesheet_uri, query) = match rest.split_once('?') {
            Some((path, q)) => (path, Some(q)),
            None => (rest, None),
        };

        if stylesheet_uri.is_empty() {
            return Err(CamelError::EndpointCreationFailed(
                "stylesheet URI cannot be empty".to_string(),
            ));
        }

        let mut direction = None;
        let mut params = Vec::new();
        let mut max_payload_bytes = None;
        let mut transform_direction = None;
        let mut resource_uri = None;
        let mut retry_count = 3u32;
        let mut retry_delay_ms = 500u64;

        if let Some(q) = query {
            for (key, value) in url::form_urlencoded::parse(q.as_bytes()) {
                if key == "direction" {
                    direction = Some(Direction::from_query(value.as_ref())?);
                    continue;
                }

                if key == "transformDirection" {
                    transform_direction = Some(value.into_owned());
                    continue;
                }

                if key == "resourceUri" {
                    resource_uri = Some(value.into_owned());
                    continue;
                }

                if key == "maxPayloadBytes" {
                    max_payload_bytes = Some(value.parse::<usize>().map_err(|e| {
                        CamelError::EndpointCreationFailed(format!(
                            "invalid maxPayloadBytes '{value}': {e}"
                        ))
                    })?);
                    continue;
                }

                if key == "retryCount" {
                    retry_count = value.parse::<u32>().map_err(|e| {
                        CamelError::EndpointCreationFailed(format!(
                            "invalid retryCount '{value}': {e}"
                        ))
                    })?;
                    continue;
                }

                if key == "retryDelayMs" {
                    retry_delay_ms = value.parse::<u64>().map_err(|e| {
                        CamelError::EndpointCreationFailed(format!(
                            "invalid retryDelayMs '{value}': {e}"
                        ))
                    })?;
                    continue;
                }

                if let Some(param_key) = key.strip_prefix("param.")
                    && !param_key.is_empty()
                {
                    params.push((param_key.to_string(), value.into_owned()));
                }
            }
        }

        let direction = direction.ok_or_else(|| {
            CamelError::EndpointCreationFailed(
                "missing required query parameter 'direction'".to_string(),
            )
        })?;

        Ok(Self {
            stylesheet_uri: stylesheet_uri.to_string(),
            direction,
            params,
            max_payload_bytes,
            transform_direction,
            resource_uri,
            retry_count,
            retry_delay_ms,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_xj_uri() {
        let cfg = XjEndpointConfig::from_uri(
            "xj:file:///tmp/a.xslt?direction=xml2json&param.foo=bar&param.a=1",
        )
        .unwrap();

        assert_eq!(cfg.stylesheet_uri, "file:///tmp/a.xslt");
        assert_eq!(cfg.direction, Direction::XmlToJson);
        assert_eq!(
            cfg.params,
            vec![
                ("foo".to_string(), "bar".to_string()),
                ("a".to_string(), "1".to_string())
            ]
        );
    }

    #[test]
    fn rejects_missing_direction() {
        let err = XjEndpointConfig::from_uri("xj:classpath:identity").unwrap_err();
        assert!(
            err.to_string()
                .contains("missing required query parameter 'direction'")
        );
    }

    #[test]
    fn parses_transform_direction_and_resource_uri() {
        let cfg = XjEndpointConfig::from_uri(
            "xj:file:///tmp/a.xslt?direction=xml2json&transformDirection=XML2JSON&resourceUri=classpath:extra.xslt",
        )
        .unwrap();

        assert_eq!(cfg.stylesheet_uri, "file:///tmp/a.xslt");
        assert_eq!(cfg.direction, Direction::XmlToJson);
        assert_eq!(cfg.transform_direction, Some("XML2JSON".to_string()));
        assert_eq!(cfg.resource_uri, Some("classpath:extra.xslt".to_string()));
        assert_eq!(cfg.retry_count, 3);
        assert_eq!(cfg.retry_delay_ms, 500);
    }

    #[test]
    fn new_options_default_to_none() {
        let cfg = XjEndpointConfig::from_uri("xj:classpath:identity?direction=json2xml").unwrap();

        assert_eq!(cfg.transform_direction, None);
        assert_eq!(cfg.resource_uri, None);
        assert_eq!(cfg.retry_count, 3);
        assert_eq!(cfg.retry_delay_ms, 500);
    }

    #[test]
    fn parses_retry_options() {
        let cfg = XjEndpointConfig::from_uri(
            "xj:classpath:identity?direction=xml2json&retryCount=7&retryDelayMs=1200",
        )
        .unwrap();

        assert_eq!(cfg.retry_count, 7);
        assert_eq!(cfg.retry_delay_ms, 1200);
    }

    #[test]
    fn rejects_empty_direction_value() {
        let err = XjEndpointConfig::from_uri("xj:classpath:identity?direction=").unwrap_err();
        assert!(
            err.to_string().contains("unsupported direction ''"),
            "expected unsupported direction error, got: {err}"
        );
    }

    #[test]
    fn rejects_invalid_direction_value() {
        let err = XjEndpointConfig::from_uri("xj:classpath:identity?direction=foobar").unwrap_err();
        assert!(
            err.to_string().contains("unsupported direction 'foobar'"),
            "expected unsupported direction error, got: {err}"
        );
    }
}
