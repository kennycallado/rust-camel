//! Keyspace helpers for Redis-backed repositories.
//!
//! Keys are namespaced hierarchically as `{prefix}:{repo}:{key}` so multiple
//! repositories can share one Redis deployment without collisions.

use camel_api::CamelError;

/// Builds a hierarchical Redis key: `{prefix}:{repo}:{key}`.
pub fn namespaced(prefix: &str, repo: &str, key: &str) -> String {
    format!("{prefix}:{repo}:{key}")
}

/// Validates a namespace token (key prefix or repository name).
///
/// Rejects empty values and any character outside `[A-Za-z0-9:_-]`, so glob
/// metacharacters can never leak into key patterns.
pub fn validate_namespace_token(kind: &str, value: &str) -> Result<(), CamelError> {
    let valid = !value.is_empty()
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, ':' | '_' | '-'));

    if valid {
        Ok(())
    } else {
        Err(CamelError::Config(format!(
            "{kind} '{value}': must be non-empty and use only [A-Za-z0-9:_-] \
             (glob metacharacters are forbidden)"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::namespaced;
    use super::validate_namespace_token;
    use camel_api::CamelError;

    #[test]
    fn namespaced_builds_hierarchical_key() {
        assert_eq!(
            namespaced("camel:cache", "default", "k"),
            "camel:cache:default:k"
        );
    }

    #[test]
    fn validate_rejects_glob_metacharacters() {
        for bad in ["my*cache", "my?cache", "my[cache", "my]cache"] {
            let err = validate_namespace_token("repository name", bad).unwrap_err();
            assert!(
                matches!(&err, CamelError::Config(msg) if msg.contains("[A-Za-z0-9:_-]")),
                "expected Config error with charset text for {bad:?}, got: {err}"
            );
        }
    }

    #[test]
    fn validate_rejects_empty_token() {
        assert!(validate_namespace_token("key_prefix", "").is_err());
    }

    #[test]
    fn validate_accepts_colon_tokens() {
        validate_namespace_token("key_prefix", "camel:cache").unwrap();
    }
}
