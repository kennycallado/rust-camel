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

        if let Some(q) = query {
            for (key, value) in url::form_urlencoded::parse(q.as_bytes()) {
                if key == "direction" {
                    direction = Some(Direction::from_query(value.as_ref())?);
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
}
