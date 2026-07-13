use std::collections::HashMap;

use camel_api::CamelError;

/// Parsed components of a Camel URI.
///
/// Format: `scheme:path?key1=value1&key2=value2`
#[derive(Clone, PartialEq)]
pub struct UriComponents {
    /// The scheme (component name), e.g. "timer", "log".
    pub scheme: String,
    /// The path portion after the scheme, e.g. "tick" in "timer:tick".
    pub path: String,
    /// Query parameters as key-value pairs.
    pub params: HashMap<String, String>,
}

const SENSITIVE_KEYS: &[&str] = &[
    "password",
    "secret",
    "token",
    "credential",
    "apikey",
    "accesskey",
    "privatekey",
];

fn is_sensitive_key(key: &str) -> bool {
    SENSITIVE_KEYS.contains(&key.to_lowercase().as_str())
}

fn unwrap_raw(value: &str) -> &str {
    if value.starts_with("RAW(") && value.ends_with(')') {
        &value[4..value.len() - 1]
    } else {
        value
    }
}

fn is_raw_value(value: &str) -> bool {
    value.starts_with("RAW(") && value.ends_with(')')
}

/// Percent-decode a string per RFC 3986.
///
/// `%XX` sequences are replaced by the byte represented by the hex digits.
/// `+` is NOT treated as space (Camel URIs are not form-encoded).
/// Returns an error for incomplete or invalid `%XX` sequences, or if the
/// resulting bytes are not valid UTF-8.
fn percent_decode(s: &str) -> Result<String, CamelError> {
    let bytes = s.as_bytes();
    let mut result = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if i + 2 >= bytes.len() {
                return Err(CamelError::InvalidUri(format!(
                    "incomplete percent-encoding at position {i} in '{s}'"
                )));
            }
            let hi = char::from(bytes[i + 1]);
            let lo = char::from(bytes[i + 2]);
            let byte = u8::from_str_radix(&format!("{hi}{lo}"), 16).map_err(|_| {
                CamelError::InvalidUri(format!("invalid percent-encoding '%{hi}{lo}' in '{s}'"))
            })?;
            result.push(byte);
            i += 3;
        } else {
            result.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(result).map_err(|e| {
        CamelError::InvalidUri(format!("percent-decoded bytes are not valid UTF-8: {e}"))
    })
}

impl std::fmt::Debug for UriComponents {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut redacted_params = std::collections::HashMap::new();
        for (k, v) in &self.params {
            if is_sensitive_key(k) {
                redacted_params.insert(k.clone(), "***".to_string());
            } else {
                redacted_params.insert(k.clone(), v.clone());
            }
        }
        f.debug_struct("UriComponents")
            .field("scheme", &self.scheme)
            .field("path", &self.path)
            .field("params", &redacted_params)
            .finish()
    }
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

    // EP-005: Validate scheme characters — only alphanumeric and hyphens allowed.
    if !scheme
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-')
    {
        return Err(CamelError::InvalidUri(format!(
            "invalid scheme '{scheme}': must contain only alphanumeric characters and hyphens"
        )));
    }

    let (path, params) = match rest.split_once('?') {
        Some((path, query)) => (path, parse_query(query)?),
        None => (rest, HashMap::new()),
    };

    Ok(UriComponents {
        scheme: scheme.to_string(),
        path: percent_decode(path)?,
        params,
    })
}

fn parse_query(query: &str) -> Result<HashMap<String, String>, CamelError> {
    let mut params = HashMap::new();

    for pair in split_query_pairs(query)
        .into_iter()
        .filter(|s| !s.is_empty())
    {
        let Some((key, value)) = pair.split_once('=') else {
            return Err(CamelError::InvalidUri(format!(
                "query parameter '{}' has no value",
                pair
            )));
        };

        let decoded_key = percent_decode(key)?;

        if params.contains_key(&decoded_key) {
            return Err(CamelError::InvalidUri(format!(
                "duplicate query parameter: {}",
                decoded_key
            )));
        }

        let parsed_value = if is_raw_value(value) {
            // RAW(...) signals "treat this value literally, no further processing".
            // For sensitive keys: unwrap the RAW(...) wrapper so the stored value is
            // the plain secret (consistent with pre-existing sensitive key handling).
            // For non-sensitive keys: preserve the full `RAW(...)` string intact so
            // downstream consumers can detect it and handle it explicitly (e.g., avoid
            // encoding it again). This is intentional, not an oversight.
            if is_sensitive_key(&decoded_key) {
                unwrap_raw(value).to_string()
            } else {
                value.to_string()
            }
        } else if is_sensitive_key(&decoded_key) {
            // Sensitive non-RAW: preserve literally (no decode)
            value.to_string()
        } else {
            // Non-sensitive non-RAW: percent-decode
            percent_decode(value)?
        };

        params.insert(decoded_key, parsed_value);
    }

    Ok(params)
}

fn split_query_pairs(query: &str) -> Vec<&str> {
    let mut pairs = Vec::new();
    let mut start = 0usize;
    let mut i = 0usize;
    let mut raw_depth = 0usize;

    while i < query.len() {
        let rest = &query[i..];

        if rest.starts_with("RAW(") {
            raw_depth += 1;
            i += 4;
            continue;
        }

        let ch = rest.as_bytes()[0] as char;
        match ch {
            ')' if raw_depth > 0 => raw_depth -= 1,
            '&' if raw_depth == 0 => {
                pairs.push(&query[start..i]);
                i += 1;
                start = i;
                continue;
            }
            _ => {}
        }

        i += 1;
    }

    pairs.push(&query[start..]);
    pairs
}

/// Parse a boolean parameter from a string, case-insensitively.
///
/// Accepts: "true"/"True"/"TRUE"/"1"/"yes" as true,
///          "false"/"False"/"FALSE"/"0"/"no" as false.
pub fn parse_bool_param(s: &str) -> Result<bool, String> {
    match s.to_lowercase().as_str() {
        "true" | "1" | "yes" => Ok(true),
        "false" | "0" | "no" => Ok(false),
        _ => Err(format!("invalid boolean value: '{}'", s)),
    }
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

    #[test]
    fn test_uri_components_debug_redacts_sensitive_params() {
        let uri = parse_uri("timer:tick?password=secret&token=abc123&name=hello").unwrap();
        let debug_output = format!("{:?}", uri);
        assert!(
            !debug_output.contains("secret"),
            "Debug must not contain password value"
        );
        assert!(
            !debug_output.contains("abc123"),
            "Debug must not contain token value"
        );
        assert!(
            debug_output.contains("hello"),
            "Debug should contain non-sensitive param values"
        );
        assert!(
            debug_output.contains("password"),
            "Debug should show param key 'password'"
        );
    }

    #[test]
    fn test_uri_components_debug_redacts_case_insensitive() {
        let uri = parse_uri("timer:tick?Password=secret&TOKEN=abc123").unwrap();
        let debug_output = format!("{:?}", uri);
        assert!(
            !debug_output.contains("secret"),
            "Debug must redact 'Password' (capitalized)"
        );
        assert!(
            !debug_output.contains("abc123"),
            "Debug must redact 'TOKEN' (uppercase)"
        );
    }

    #[test]
    fn test_parse_bool_param_true_variants() {
        for val in &["true", "True", "TRUE", "1", "yes", "Yes", "YES"] {
            assert_eq!(
                parse_bool_param(val),
                Ok(true),
                "parse_bool_param('{}') should be Ok(true)",
                val
            );
        }
    }

    #[test]
    fn test_parse_bool_param_false_variants() {
        for val in &["false", "False", "FALSE", "0", "no", "No", "NO"] {
            assert_eq!(
                parse_bool_param(val),
                Ok(false),
                "parse_bool_param('{}') should be Ok(false)",
                val
            );
        }
    }

    #[test]
    fn test_parse_bool_param_invalid() {
        for val in &["maybe", "yes ", " true", "2", "-1", ""] {
            assert!(
                parse_bool_param(val).is_err(),
                "parse_bool_param('{}') should be Err",
                val
            );
        }
    }

    #[test]
    fn test_raw_token_extracts_value() {
        assert_eq!(unwrap_raw("RAW(p@ss!)"), "p@ss!");
        assert_eq!(unwrap_raw("RAW(user:pass@host)"), "user:pass@host");
    }

    #[test]
    fn test_non_raw_value_unchanged() {
        assert_eq!(unwrap_raw("plainvalue"), "plainvalue");
        assert_eq!(unwrap_raw("RAW(unclosed"), "RAW(unclosed");
    }

    #[test]
    fn test_uri_with_raw_password_parses_correctly() {
        let result = parse_uri("redis://localhost?password=RAW(p@ss!)").unwrap();
        assert_eq!(result.params.get("password"), Some(&"p@ss!".to_string()));
    }

    #[test]
    fn test_uri_with_raw_password_containing_ampersand_parses_correctly() {
        let result = parse_uri("redis://localhost?password=RAW(a&b)&db=0").unwrap();
        assert_eq!(result.params.get("password"), Some(&"a&b".to_string()));
        assert_eq!(result.params.get("db"), Some(&"0".to_string()));
    }

    #[test]
    fn test_uri_with_non_sensitive_raw_value_is_unchanged() {
        let result = parse_uri("timer:tick?name=RAW(p@ss!)").unwrap();
        assert_eq!(result.params.get("name"), Some(&"RAW(p@ss!)".to_string()));
    }

    #[test]
    fn test_parse_uri_duplicate_query_key_returns_error() {
        let result = parse_uri("foo:bar?key=a&key=b");
        assert!(result.is_err());
        match result {
            Err(CamelError::InvalidUri(msg)) => {
                assert_eq!(msg, "duplicate query parameter: key");
            }
            _ => panic!("Expected InvalidUri for duplicate key"),
        }
    }

    #[test]
    fn test_parse_uri_bare_query_param_returns_error() {
        let result = parse_uri("foo:bar?flag");
        assert!(result.is_err());
        match result {
            Err(CamelError::InvalidUri(msg)) => {
                assert_eq!(msg, "query parameter 'flag' has no value");
            }
            _ => panic!("Expected InvalidUri for bare query parameter"),
        }
    }

    #[test]
    fn test_parse_uri_duplicate_key_with_raw_ampersand_returns_error() {
        let result = parse_uri("foo:bar?password=RAW(a&b)&password=RAW(c&d)");
        assert!(result.is_err());
        match result {
            Err(CamelError::InvalidUri(msg)) => {
                assert_eq!(msg, "duplicate query parameter: password");
            }
            _ => panic!("Expected InvalidUri for duplicate key with RAW value"),
        }
    }

    // EP-005: scheme validation tests

    #[test]
    fn test_valid_scheme_alphanumeric() {
        let result = parse_uri("timer:tick").unwrap();
        assert_eq!(result.scheme, "timer");
    }

    #[test]
    fn test_valid_scheme_with_hyphen() {
        let result = parse_uri("my-component:path").unwrap();
        assert_eq!(result.scheme, "my-component");
    }

    #[test]
    fn test_valid_scheme_alphanumeric_only() {
        let result = parse_uri("opensearchs://host:9200/idx").unwrap();
        assert_eq!(result.scheme, "opensearchs");
    }

    #[test]
    fn test_invalid_scheme_with_space() {
        let result = parse_uri("bad scheme:path");
        assert!(result.is_err());
        match result {
            Err(CamelError::InvalidUri(msg)) => {
                assert!(msg.contains("invalid scheme"), "got: {msg}");
            }
            _ => panic!("Expected InvalidUri for scheme with space"),
        }
    }

    #[test]
    fn test_invalid_scheme_with_dot() {
        let result = parse_uri("bad.scheme:path");
        assert!(result.is_err());
        match result {
            Err(CamelError::InvalidUri(msg)) => {
                assert!(msg.contains("invalid scheme"), "got: {msg}");
            }
            _ => panic!("Expected InvalidUri for scheme with dot"),
        }
    }

    #[test]
    fn test_invalid_scheme_with_underscore() {
        let result = parse_uri("bad_scheme:path");
        assert!(result.is_err());
    }

    // ENDPOINT-002: percent-decoding tests

    #[test]
    fn test_parse_uri_percent_encoded_path() {
        let result = parse_uri("timer:my%20timer").unwrap();
        assert_eq!(result.path, "my timer");
    }

    #[test]
    fn test_parse_uri_percent_encoded_query_value() {
        let result = parse_uri("log:info?description=hello%20world").unwrap();
        assert_eq!(
            result.params.get("description"),
            Some(&"hello world".to_string())
        );
    }

    #[test]
    fn test_parse_uri_percent_encoded_special_chars() {
        // %2F = '/', %3A = ':', %40 = '@'
        let result = parse_uri("http://host/path?user=foo%40bar.com&redirect=%2Fhome").unwrap();
        assert_eq!(result.params.get("user"), Some(&"foo@bar.com".to_string()));
        assert_eq!(result.params.get("redirect"), Some(&"/home".to_string()));
    }

    #[test]
    fn test_parse_uri_percent_encoded_path_with_slash() {
        let result = parse_uri("file:my%2Fpath%2Fhere").unwrap();
        assert_eq!(result.path, "my/path/here");
    }

    #[test]
    fn test_raw_value_not_percent_decoded() {
        // RAW(...) values bypass percent-decoding — they are already raw
        let result = parse_uri("redis://localhost?password=RAW(%40secret)").unwrap();
        assert_eq!(
            result.params.get("password"),
            Some(&"%40secret".to_string())
        );
    }

    #[test]
    fn test_percent_encoded_key_decoded() {
        let result = parse_uri("foo:bar?my%20key=value").unwrap();
        assert_eq!(result.params.get("my key"), Some(&"value".to_string()));
    }

    #[test]
    fn test_invalid_percent_sequence_returns_error() {
        let result = parse_uri("foo:bar?key=%ZZ");
        assert!(
            result.is_err(),
            "Expected error for invalid percent sequence %ZZ"
        );
    }

    #[test]
    fn test_incomplete_percent_sequence_returns_error() {
        let result = parse_uri("foo:bar?key=val%");
        assert!(
            result.is_err(),
            "Expected error for incomplete percent sequence"
        );
        let result2 = parse_uri("foo:bar?key=val%2");
        assert!(
            result2.is_err(),
            "Expected error for truncated percent sequence"
        );
    }

    #[test]
    fn test_percent_encoded_plus_is_not_space() {
        // Camel URIs are NOT form-encoded; '+' is a literal plus, not space
        let result = parse_uri("foo:bar?key=a+b").unwrap();
        assert_eq!(result.params.get("key"), Some(&"a+b".to_string()));
    }

    #[test]
    fn test_percent_encoded_plus_decodes_to_plus() {
        // %2B must decode to literal '+' in both path and query value
        let result = parse_uri("file:a%2Bb?key=c%2Bd").unwrap();
        assert_eq!(result.path, "a+b");
        assert_eq!(result.params.get("key"), Some(&"c+d".to_string()));
    }

    #[test]
    fn test_percent_encoded_multibyte_utf8() {
        // %C3%A9 = U+00E9 LATIN SMALL LETTER E WITH ACUTE ('é')
        let result = parse_uri("file:caf%C3%A9?name=r%C3%A9sum%C3%A9").unwrap();
        assert_eq!(result.path, "café");
        assert_eq!(result.params.get("name"), Some(&"résumé".to_string()));
    }

    #[test]
    fn test_percent_encoded_null_byte_allowed() {
        // %00 decodes to NUL byte — behavior is pinned: decoder allows it, result contains '\0'
        let result = parse_uri("foo:bar?key=val%00end").unwrap();
        assert_eq!(result.params.get("key"), Some(&"val\0end".to_string()));
    }

    #[test]
    fn test_sensitive_key_percent_encoded() {
        // Key is percent-decoded before sensitivity check; sensitive value is NOT percent-decoded
        let result = parse_uri("db:conn?pass%77ord=abc%20def").unwrap();
        // "pass%77ord" decodes key to "password" → sensitive → value stored literal
        assert_eq!(
            result.params.get("password"),
            Some(&"abc%20def".to_string())
        );
    }

    // R4-L2 (revised): `#` is NOT stripped — Camel endpoint URIs use their own
    // grammar (`scheme:path?params`), not RFC 3986. `#` is an established
    // placeholder character in SQL (`:#name`, positional `#`) and in other
    // component path/query languages. Operators who need a literal `#` in an
    // RFC-3986 context can percent-encode as `%23` (handled by percent_decode).

    #[test]
    fn parse_uri_preserves_hash_in_path() {
        let uri = parse_uri("direct://a/b#part").unwrap();
        assert_eq!(uri.path, "//a/b#part");
    }

    #[test]
    fn parse_uri_preserves_hash_in_query_value() {
        let uri = parse_uri("x:p?key=a#b").unwrap();
        assert_eq!(uri.params.get("key"), Some(&"a#b".to_string()));
    }

    #[test]
    fn parse_uri_percent_encoded_hash_decodes() {
        let uri = parse_uri("x:p%23q?key=a%23b").unwrap();
        assert_eq!(uri.path, "p#q");
        assert_eq!(uri.params.get("key"), Some(&"a#b".to_string()));
    }
}
