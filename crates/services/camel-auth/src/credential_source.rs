use std::collections::HashMap;

/// Source from which a token can be extracted.
#[derive(Clone, PartialEq, Eq)]
pub enum CredentialSource {
    /// Extract from the `Authorization` header (Bearer scheme).
    AuthorizationHeader,
    /// Extract from a query parameter with the given name.
    QueryParam { param: String },
    /// Extract from a cookie with the given name.
    Cookie { name: String },
}

impl CredentialSource {
    /// Returns the variant name without exposing sensitive values.
    pub fn variant_name(&self) -> &'static str {
        match self {
            CredentialSource::AuthorizationHeader => "AuthorizationHeader",
            CredentialSource::QueryParam { .. } => "QueryParam",
            CredentialSource::Cookie { .. } => "Cookie",
        }
    }
}

impl std::fmt::Debug for CredentialSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CredentialSource::AuthorizationHeader => f.write_str("AuthorizationHeader"),
            CredentialSource::QueryParam { param } => {
                write!(f, "QueryParam {{ param: {:?} }}", param) // allow-secret
            }
            CredentialSource::Cookie { name } => {
                write!(f, "Cookie {{ name: {:?} }}", name) // allow-secret
            }
        }
    }
}

/// A token extracted from a specific source.
#[derive(Clone)]
pub struct ExtractedToken {
    pub token: String,
    pub source: CredentialSource,
}

impl std::fmt::Debug for ExtractedToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExtractedToken")
            .field("token", &"[REDACTED]")
            .field("source", &self.source)
            .finish()
    }
}

/// Percent-decode a string, returning the original on failure.
fn percent_decode_str(s: &str) -> String {
    percent_encoding::percent_decode_str(s)
        .decode_utf8()
        .unwrap_or_else(|_| s.into())
        .into_owned()
}

/// Extract a bearer token from the `Authorization` header.
///
/// Delegates to `crate::extract_bearer_token`. Returns `None` if the header
/// is absent; propagates errors as `None` (the caller should log separately).
pub fn extract_token_from_header(headers: &http::HeaderMap) -> Option<String> {
    let value = headers.get(http::header::AUTHORIZATION)?;
    crate::extract_bearer_token(value.to_str().ok()?)
        .ok()
        .flatten()
}

/// Extract a token from the query string of a URI.
///
/// Parses the query portion of `uri`, looks for `param`, and percent-decodes
/// the value.
pub fn extract_token_from_query(uri: &http::Uri, param: &str) -> Option<String> {
    let query = uri.query()?;
    let pairs = parse_query_string(query);
    pairs.get(param).map(|v| percent_decode_str(v))
}

/// Extract a token from a named cookie in the `Cookie` header.
///
/// Parses the `Cookie` header value (semicolon-separated `name=value` pairs)
/// and returns the value for `cookie_name`, percent-decoded.
pub fn extract_token_from_cookie(headers: &http::HeaderMap, cookie_name: &str) -> Option<String> {
    let cookie_header = headers.get(http::header::COOKIE)?;
    let cookie_str = cookie_header.to_str().ok()?;
    parse_cookie_header(cookie_str)
        .get(cookie_name)
        .map(|v| percent_decode_str(v))
}

/// Try each source in order, returning the first successful extraction.
pub fn extract_token_multi(
    headers: &http::HeaderMap,
    uri: &http::Uri,
    sources: &[CredentialSource],
) -> Option<ExtractedToken> {
    for source in sources {
        match source {
            CredentialSource::AuthorizationHeader => {
                if let Some(token) = extract_token_from_header(headers) {
                    return Some(ExtractedToken {
                        token,
                        source: source.clone(),
                    });
                }
            }
            CredentialSource::QueryParam { param } => {
                if let Some(token) = extract_token_from_query(uri, param) {
                    return Some(ExtractedToken {
                        token,
                        source: source.clone(),
                    });
                }
            }
            CredentialSource::Cookie { name } => {
                if let Some(token) = extract_token_from_cookie(headers, name) {
                    return Some(ExtractedToken {
                        token,
                        source: source.clone(),
                    });
                }
            }
        }
    }
    None
}

/// Replace sensitive query parameter values with `[REDACTED]`.
///
/// Returns the full URI as a string with matching param values replaced.
pub fn redact_query_params(uri: &http::Uri, sensitive_params: &[&str]) -> String {
    let query = match uri.query() {
        Some(q) => q,
        None => return uri.to_string(),
    };

    let redacted: Vec<String> = query
        .split('&')
        .map(|pair| {
            if let Some((key, _value)) = pair.split_once('=') {
                if sensitive_params.iter().any(|s| s == &key) {
                    format!("{}=[REDACTED]", key)
                } else {
                    pair.to_string()
                }
            } else {
                pair.to_string()
            }
        })
        .collect();

    let base = uri.path();
    if redacted.is_empty() {
        base.to_string()
    } else {
        format!("{}?{}", base, redacted.join("&"))
    }
}

// --- Internal helpers ---

fn parse_query_string(query: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for pair in query.split('&') {
        if let Some((key, value)) = pair.split_once('=') {
            map.insert(key.to_string(), value.to_string());
        }
    }
    map
}

fn parse_cookie_header(cookie_str: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for pair in cookie_str.split(';') {
        let pair = pair.trim();
        if let Some((key, value)) = pair.split_once('=') {
            map.insert(key.trim().to_string(), value.trim().to_string());
        }
    }
    map
}

// --- Tests ---

#[cfg(test)]
mod tests {
    use super::*;

    // --- CredentialSource Debug ---

    #[test]
    fn debug_authorization_header_shows_variant_name() {
        let source = CredentialSource::AuthorizationHeader;
        assert_eq!(format!("{:?}", source), "AuthorizationHeader");
    }

    #[test]
    fn debug_query_param_shows_param_name() {
        let source = CredentialSource::QueryParam {
            param: "token".to_string(),
        };
        assert_eq!(format!("{:?}", source), "QueryParam { param: \"token\" }"); // allow-secret
    }

    #[test]
    fn debug_cookie_shows_cookie_name() {
        let source = CredentialSource::Cookie {
            name: "session".to_string(),
        };
        assert_eq!(format!("{:?}", source), "Cookie { name: \"session\" }");
    }

    #[test]
    fn credential_source_clone() {
        let original = CredentialSource::QueryParam {
            param: "access_token".to_string(),
        };
        let cloned = original.clone();
        assert_eq!(format!("{:?}", original), format!("{:?}", cloned));
    }

    // --- extract_token_from_header ---

    #[test]
    fn extract_header_valid_bearer() {
        let mut headers = http::HeaderMap::new();
        headers.insert(
            http::header::AUTHORIZATION,
            "Bearer mytoken123".parse().unwrap(),
        );
        let token = extract_token_from_header(&headers);
        assert_eq!(token, Some("mytoken123".to_string()));
    }

    #[test]
    fn extract_header_missing_returns_none() {
        let headers = http::HeaderMap::new();
        let token = extract_token_from_header(&headers);
        assert!(token.is_none());
    }

    #[test]
    fn extract_header_non_bearer_returns_none() {
        let mut headers = http::HeaderMap::new();
        headers.insert(http::header::AUTHORIZATION, "Basic abc123".parse().unwrap());
        let token = extract_token_from_header(&headers);
        assert!(token.is_none());
    }

    // --- extract_token_from_query ---

    #[test]
    fn extract_query_valid_token() {
        let uri: http::Uri = "/ws?token=abc123".parse().unwrap();
        let token = extract_token_from_query(&uri, "token");
        assert_eq!(token, Some("abc123".to_string()));
    }

    #[test]
    fn extract_query_missing_param_returns_none() {
        let uri: http::Uri = "/ws?other=value".parse().unwrap();
        let token = extract_token_from_query(&uri, "token");
        assert!(token.is_none());
    }

    #[test]
    fn extract_query_no_query_string_returns_none() {
        let uri: http::Uri = "/ws".parse().unwrap();
        let token = extract_token_from_query(&uri, "token");
        assert!(token.is_none());
    }

    #[test]
    fn extract_query_percent_encoded() {
        let uri: http::Uri = "/ws?token=abc%2Bdef".parse().unwrap();
        let token = extract_token_from_query(&uri, "token");
        assert_eq!(token, Some("abc+def".to_string()));
    }

    #[test]
    fn extract_query_multiple_params() {
        let uri: http::Uri = "/ws?foo=bar&token=secret&baz=qux".parse().unwrap();
        let token = extract_token_from_query(&uri, "token");
        assert_eq!(token, Some("secret".to_string()));
    }

    // --- extract_token_from_cookie ---

    #[test]
    fn extract_cookie_valid() {
        let mut headers = http::HeaderMap::new();
        headers.insert(
            http::header::COOKIE,
            "session=cookie_token_123".parse().unwrap(),
        );
        let token = extract_token_from_cookie(&headers, "session");
        assert_eq!(token, Some("cookie_token_123".to_string()));
    }

    #[test]
    fn extract_cookie_missing_returns_none() {
        let mut headers = http::HeaderMap::new();
        headers.insert(http::header::COOKIE, "other=value".parse().unwrap());
        let token = extract_token_from_cookie(&headers, "session");
        assert!(token.is_none());
    }

    #[test]
    fn extract_cookie_no_cookie_header_returns_none() {
        let headers = http::HeaderMap::new();
        let token = extract_token_from_cookie(&headers, "session");
        assert!(token.is_none());
    }

    #[test]
    fn extract_cookie_multiple_cookies() {
        let mut headers = http::HeaderMap::new();
        headers.insert(
            http::header::COOKIE,
            "foo=bar; auth_token=mycookie; baz=qux".parse().unwrap(),
        );
        let token = extract_token_from_cookie(&headers, "auth_token");
        assert_eq!(token, Some("mycookie".to_string()));
    }

    #[test]
    fn extract_cookie_with_spaces() {
        let mut headers = http::HeaderMap::new();
        headers.insert(
            http::header::COOKIE,
            "foo=bar;  auth_token=spaced_token  ; baz=qux"
                .parse()
                .unwrap(),
        );
        let token = extract_token_from_cookie(&headers, "auth_token");
        assert_eq!(token, Some("spaced_token".to_string()));
    }

    // --- extract_token_multi ---

    #[test]
    fn multi_falls_back_from_header_to_query() {
        let headers = http::HeaderMap::new();
        let uri: http::Uri = "/ws?token=query_token".parse().unwrap();
        let sources = vec![
            CredentialSource::AuthorizationHeader,
            CredentialSource::QueryParam {
                param: "token".to_string(),
            },
        ];
        let result = extract_token_multi(&headers, &uri, &sources);
        assert!(result.is_some());
        let extracted = result.unwrap();
        assert_eq!(extracted.token, "query_token");
        assert!(matches!(
            extracted.source,
            CredentialSource::QueryParam { .. }
        ));
    }

    #[test]
    fn multi_prefers_first_matching_source() {
        let mut headers = http::HeaderMap::new();
        headers.insert(
            http::header::AUTHORIZATION,
            "Bearer header_token".parse().unwrap(),
        );
        let uri: http::Uri = "/ws?token=query_token".parse().unwrap();
        let sources = vec![
            CredentialSource::AuthorizationHeader,
            CredentialSource::QueryParam {
                param: "token".to_string(),
            },
        ];
        let result = extract_token_multi(&headers, &uri, &sources);
        assert!(result.is_some());
        let extracted = result.unwrap();
        assert_eq!(extracted.token, "header_token");
        assert!(matches!(
            extracted.source,
            CredentialSource::AuthorizationHeader
        ));
    }

    #[test]
    fn multi_falls_back_to_cookie() {
        let mut headers = http::HeaderMap::new();
        headers.insert(
            http::header::COOKIE,
            "session=cookie_token".parse().unwrap(),
        );
        let uri: http::Uri = "/ws".parse().unwrap();
        let sources = vec![
            CredentialSource::AuthorizationHeader,
            CredentialSource::QueryParam {
                param: "token".to_string(),
            },
            CredentialSource::Cookie {
                name: "session".to_string(),
            },
        ];
        let result = extract_token_multi(&headers, &uri, &sources);
        assert!(result.is_some());
        let extracted = result.unwrap();
        assert_eq!(extracted.token, "cookie_token");
        assert!(matches!(extracted.source, CredentialSource::Cookie { .. }));
    }

    #[test]
    fn multi_returns_none_when_all_fail() {
        let headers = http::HeaderMap::new();
        let uri: http::Uri = "/ws".parse().unwrap();
        let sources = vec![
            CredentialSource::AuthorizationHeader,
            CredentialSource::QueryParam {
                param: "token".to_string(),
            },
            CredentialSource::Cookie {
                name: "session".to_string(),
            },
        ];
        let result = extract_token_multi(&headers, &uri, &sources);
        assert!(result.is_none());
    }

    // --- redact_query_params ---

    #[test]
    fn redact_single_sensitive_param() {
        let uri: http::Uri = "/ws?token=secret&foo=bar".parse().unwrap();
        let redacted = redact_query_params(&uri, &["token"]);
        assert_eq!(redacted, "/ws?token=[REDACTED]&foo=bar");
    }

    #[test]
    fn redact_multiple_sensitive_params() {
        let uri: http::Uri = "/ws?token=secret&password=pass123&foo=bar".parse().unwrap();
        let redacted = redact_query_params(&uri, &["token", "password"]);
        assert_eq!(redacted, "/ws?token=[REDACTED]&password=[REDACTED]&foo=bar");
    }

    #[test]
    fn redact_no_sensitive_params_in_uri() {
        let uri: http::Uri = "/ws?foo=bar&baz=qux".parse().unwrap();
        let redacted = redact_query_params(&uri, &["token"]);
        assert_eq!(redacted, "/ws?foo=bar&baz=qux");
    }

    #[test]
    fn redact_no_query_string_returns_uri_as_is() {
        let uri: http::Uri = "/ws".parse().unwrap();
        let redacted = redact_query_params(&uri, &["token"]);
        assert_eq!(redacted, "/ws");
    }

    // --- percent_decode_str ---

    #[test]
    fn percent_decode_plus_sign() {
        assert_eq!(percent_decode_str("hello%2Bworld"), "hello+world");
    }

    #[test]
    fn percent_decode_space() {
        assert_eq!(percent_decode_str("hello%20world"), "hello world");
    }

    #[test]
    fn percent_decode_no_encoding_returns_original() {
        assert_eq!(percent_decode_str("plaintext"), "plaintext");
    }

    #[test]
    fn extracted_token_debug_redacts_token() {
        let token = ExtractedToken {
            token: "super-secret-jwt-value".to_string(),
            source: CredentialSource::AuthorizationHeader,
        };
        let debug = format!("{token:?}"); // allow-secret
        assert!(!debug.contains("super-secret-jwt-value"));
        assert!(debug.contains("[REDACTED]"));
    }
}
