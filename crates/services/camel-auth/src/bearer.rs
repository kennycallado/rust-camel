use camel_api::CamelError;

const BEARER_PREFIX: &str = "Bearer ";

pub fn extract_bearer_token(header_value: &str) -> Result<Option<String>, CamelError> {
    let trimmed = header_value.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }

    if trimmed.len() < BEARER_PREFIX.len()
        || !trimmed[..BEARER_PREFIX.len()].eq_ignore_ascii_case(BEARER_PREFIX)
    {
        return Err(CamelError::Unauthenticated(
            "invalid Authorization header: expected Bearer scheme".into(),
        ));
    }

    let token = trimmed[BEARER_PREFIX.len()..].trim();
    if token.is_empty() {
        return Err(CamelError::Unauthenticated(
            "invalid Authorization header: empty Bearer token".into(),
        ));
    }

    Ok(Some(token.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_bearer() {
        let result = extract_bearer_token("Bearer abc123").unwrap();
        assert_eq!(result, Some("abc123".to_string()));
    }

    #[test]
    fn test_empty_string_returns_none() {
        let result = extract_bearer_token("").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_whitespace_only_returns_none() {
        let result = extract_bearer_token("   ").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_empty_bearer_token() {
        // "Bearer " trims to "Bearer" (6 chars) which is shorter than prefix (7 chars),
        // so it hits the "expected Bearer scheme" error path.
        let err = extract_bearer_token("Bearer ").unwrap_err();
        match err {
            CamelError::Unauthenticated(msg) => assert!(msg.contains("expected Bearer")),
            _ => panic!("expected Unauthenticated, got: {err:?}"),
        }
    }

    #[test]
    fn test_wrong_scheme() {
        let err = extract_bearer_token("Basic abc123").unwrap_err();
        match err {
            CamelError::Unauthenticated(msg) => assert!(msg.contains("expected Bearer")),
            _ => panic!("expected Unauthenticated, got: {err:?}"),
        }
    }

    #[test]
    fn test_case_insensitive_bearer() {
        let result = extract_bearer_token("bearer abc123").unwrap();
        assert_eq!(result, Some("abc123".to_string()));
    }

    #[test]
    fn test_uppercase_bearer() {
        let result = extract_bearer_token("BEARER xyz789").unwrap();
        assert_eq!(result, Some("xyz789".to_string()));
    }

    #[test]
    fn test_trims_whitespace_around_token() {
        let result = extract_bearer_token("Bearer  token-with-spaces  ").unwrap();
        assert_eq!(result, Some("token-with-spaces".to_string()));
    }

    #[test]
    fn test_header_shorter_than_prefix() {
        let err = extract_bearer_token("Bea").unwrap_err();
        match err {
            CamelError::Unauthenticated(msg) => assert!(msg.contains("expected Bearer")),
            _ => panic!("expected Unauthenticated, got: {err:?}"),
        }
    }
}
