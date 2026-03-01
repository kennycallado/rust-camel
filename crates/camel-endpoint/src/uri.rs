use std::collections::HashMap;

use camel_api::CamelError;

/// Parsed components of a Camel URI.
///
/// Format: `scheme:path?key1=value1&key2=value2`
#[derive(Debug, Clone, PartialEq)]
pub struct UriComponents {
    /// The scheme (component name), e.g. "timer", "log".
    pub scheme: String,
    /// The path portion after the scheme, e.g. "tick" in "timer:tick".
    pub path: String,
    /// Query parameters as key-value pairs.
    pub params: HashMap<String, String>,
}

/// Parse a Camel-style URI into its components.
///
/// Format: `scheme:path?key1=value1&key2=value2`
pub fn parse_uri(uri: &str) -> Result<UriComponents, CamelError> {
    let (scheme, rest) = uri.split_once(':').ok_or_else(|| {
        CamelError::InvalidUri(format!("missing scheme separator ':' in '{uri}'"))
    })?;

    if scheme.is_empty() {
        return Err(CamelError::InvalidUri(format!("empty scheme in '{uri}'")));
    }

    let (path, params) = match rest.split_once('?') {
        Some((path, query)) => (path, parse_query(query)),
        None => (rest, HashMap::new()),
    };

    Ok(UriComponents {
        scheme: scheme.to_string(),
        path: path.to_string(),
        params,
    })
}

fn parse_query(query: &str) -> HashMap<String, String> {
    query
        .split('&')
        .filter(|s| !s.is_empty())
        .filter_map(|pair| {
            let (key, value) = pair.split_once('=')?;
            Some((key.to_string(), value.to_string()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_uri() {
        let result = parse_uri("timer:tick").unwrap();
        assert_eq!(result.scheme, "timer");
        assert_eq!(result.path, "tick");
        assert!(result.params.is_empty());
    }

    #[test]
    fn test_parse_uri_with_params() {
        let result = parse_uri("timer:tick?period=1000&delay=500").unwrap();
        assert_eq!(result.scheme, "timer");
        assert_eq!(result.path, "tick");
        assert_eq!(result.params.get("period"), Some(&"1000".to_string()));
        assert_eq!(result.params.get("delay"), Some(&"500".to_string()));
    }

    #[test]
    fn test_parse_uri_with_single_param() {
        let result = parse_uri("log:info?level=debug").unwrap();
        assert_eq!(result.scheme, "log");
        assert_eq!(result.path, "info");
        assert_eq!(result.params.get("level"), Some(&"debug".to_string()));
    }

    #[test]
    fn test_parse_uri_no_scheme() {
        let result = parse_uri("noscheme");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_uri_empty_scheme() {
        let result = parse_uri(":path");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_direct_uri() {
        let result = parse_uri("direct:myRoute").unwrap();
        assert_eq!(result.scheme, "direct");
        assert_eq!(result.path, "myRoute");
        assert!(result.params.is_empty());
    }

    #[test]
    fn test_parse_mock_uri() {
        let result = parse_uri("mock:result").unwrap();
        assert_eq!(result.scheme, "mock");
        assert_eq!(result.path, "result");
    }

    #[test]
    fn test_parse_http_uri_simple() {
        let result = parse_uri("http://localhost:8080/api/users").unwrap();
        assert_eq!(result.scheme, "http");
        assert_eq!(result.path, "//localhost:8080/api/users");
        assert!(result.params.is_empty());
    }

    #[test]
    fn test_parse_https_uri_with_camel_params() {
        let result = parse_uri(
            "https://api.example.com/v1/data?httpMethod=POST&throwExceptionOnFailure=false",
        )
        .unwrap();
        assert_eq!(result.scheme, "https");
        assert_eq!(result.path, "//api.example.com/v1/data");
        assert_eq!(result.params.get("httpMethod"), Some(&"POST".to_string()));
        assert_eq!(
            result.params.get("throwExceptionOnFailure"),
            Some(&"false".to_string())
        );
    }

    #[test]
    fn test_parse_http_uri_no_path() {
        let result = parse_uri("http://localhost:8080").unwrap();
        assert_eq!(result.scheme, "http");
        assert_eq!(result.path, "//localhost:8080");
        assert!(result.params.is_empty());
    }

    #[test]
    fn test_parse_http_uri_with_port_and_query() {
        let result = parse_uri("http://example.com:3000/api?connectTimeout=5000").unwrap();
        assert_eq!(result.scheme, "http");
        assert_eq!(result.path, "//example.com:3000/api");
        assert_eq!(
            result.params.get("connectTimeout"),
            Some(&"5000".to_string())
        );
    }
}
