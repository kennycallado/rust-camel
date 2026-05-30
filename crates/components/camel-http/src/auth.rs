use camel_component_api::CamelError;

pub fn extract_bearer_token(headers: &http::HeaderMap) -> Result<Option<String>, CamelError> {
    let auth_header = headers
        .get(http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    camel_auth::extract_bearer_token(auth_header)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_headers(auth: Option<&str>) -> http::HeaderMap {
        let mut headers = http::HeaderMap::new();
        if let Some(val) = auth {
            headers.insert(http::header::AUTHORIZATION, val.parse().unwrap());
        }
        headers
    }

    #[test]
    fn test_extract_bearer_token_valid() {
        let headers = make_headers(Some("Bearer abc123"));
        let result = extract_bearer_token(&headers).unwrap();
        assert_eq!(result, Some("abc123".to_string()));
    }

    #[test]
    fn test_extract_bearer_token_missing() {
        let headers = make_headers(None);
        let result = extract_bearer_token(&headers).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_extract_bearer_token_empty_bearer() {
        let headers = make_headers(Some("Bearer "));
        let result = extract_bearer_token(&headers);
        assert!(matches!(result, Err(CamelError::Unauthenticated(_))));
    }

    #[test]
    fn test_extract_bearer_token_wrong_scheme() {
        let headers = make_headers(Some("Basic abc123"));
        let result = extract_bearer_token(&headers);
        assert!(matches!(result, Err(CamelError::Unauthenticated(_))));
    }

    #[test]
    fn test_extract_bearer_token_case_insensitive() {
        let headers = make_headers(Some("bearer abc123"));
        let result = extract_bearer_token(&headers).unwrap();
        assert_eq!(result, Some("abc123".to_string()));
    }

    #[test]
    fn test_extract_bearer_token_uppercase_bearer() {
        let headers = make_headers(Some("BEARER xyz789"));
        let result = extract_bearer_token(&headers).unwrap();
        assert_eq!(result, Some("xyz789".to_string()));
    }

    #[test]
    fn test_extract_bearer_token_trims_whitespace() {
        let headers = make_headers(Some("Bearer  token-with-spaces  "));
        let result = extract_bearer_token(&headers).unwrap();
        assert_eq!(result, Some("token-with-spaces".to_string()));
    }

    #[test]
    fn test_extract_bearer_token_error_messages() {
        // "Bearer " trims to "Bearer" (6 chars) < prefix (7 chars) → "expected Bearer scheme"
        let headers = make_headers(Some("Bearer "));
        let err = extract_bearer_token(&headers).unwrap_err();
        match err {
            CamelError::Unauthenticated(msg) => assert!(msg.contains("expected Bearer")),
            _ => panic!("expected Unauthenticated"),
        }

        let headers = make_headers(Some("Digest abc"));
        let err = extract_bearer_token(&headers).unwrap_err();
        match err {
            CamelError::Unauthenticated(msg) => assert!(msg.contains("expected Bearer")),
            _ => panic!("expected Unauthenticated"),
        }
    }
}
