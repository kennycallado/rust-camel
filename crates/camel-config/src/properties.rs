use std::collections::HashMap;
use std::env;
use std::sync::RwLock;

pub struct PropertiesResolver {
    // TODO(CONFIG-015): Property value caching not implemented.
    // Values are re-read from source on every access.
    // TODO(CONFIG-017): Multi-source property chaining partially implemented.
    // Currently: file + env vars. Missing: explicit priority ordering,
    // per-source encryption, dynamic re-evaluation.
    properties: RwLock<HashMap<String, String>>,
}

// TODO(CONFIG-016): Property value encoding/decoding not supported.
// All values are treated as UTF-8 strings.

#[derive(Debug, PartialEq, thiserror::Error)]
pub enum ResolveError {
    #[error("missing property key: {key}")]
    MissingKey { key: String },
}

impl Default for PropertiesResolver {
    fn default() -> Self {
        Self::new()
    }
}

impl PropertiesResolver {
    pub fn new() -> Self {
        Self {
            properties: RwLock::new(HashMap::new()),
        }
    }

    pub fn set(&self, key: &str, value: &str) {
        self.properties
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(key.to_string(), value.to_string());
    }

    fn lookup(&self, key: &str) -> Option<String> {
        if let Some(rest) = key.strip_prefix("env:") {
            let env_key = rest.trim();
            if env_key.is_empty() {
                return None;
            }
            return env::var(env_key).ok();
        }
        let props = self
            .properties
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        props.get(key).cloned()
    }

    pub fn resolve(&self, input: &str) -> Result<String, ResolveError> {
        let mut result = input.to_string();
        let mut start = 0;

        while let Some(open) = result[start..].find("{{") {
            let open_abs = start + open;
            if let Some(close) = result[open_abs..].find("}}") {
                let close_abs = open_abs + close + 2;
                let inner = &result[open_abs + 2..open_abs + close];

                let (key, default) = if let Some(rest) = inner.strip_prefix("env:") {
                    let env_key = rest.trim();
                    if let Some(colon) = env_key.find(':') {
                        (&inner[..4 + colon], Some(&env_key[colon + 1..]))
                    } else {
                        (inner, None)
                    }
                } else if let Some(colon) = inner.find(':') {
                    (&inner[..colon], Some(&inner[colon + 1..]))
                } else {
                    (inner, None)
                };

                if key.trim().is_empty() {
                    return Err(ResolveError::MissingKey {
                        key: key.to_string(),
                    });
                }

                let value = match (self.lookup(key), default) {
                    (Some(v), _) => v,
                    (None, Some(d)) => d.to_string(),
                    (None, None) => {
                        return Err(ResolveError::MissingKey {
                            key: key.to_string(),
                        });
                    }
                };

                result.replace_range(open_abs..close_abs, &value);
                start = open_abs + value.len();
            } else {
                break;
            }
        }

        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_simple_placeholder() {
        let resolver = PropertiesResolver::new();
        resolver.set("host", "localhost");
        resolver.set("port", "6379");
        assert_eq!(
            resolver.resolve("redis://{{host}}:{{port}}"),
            Ok("redis://localhost:6379".to_string())
        );
    }

    #[test]
    fn test_resolve_missing_key_returns_error() {
        let resolver = PropertiesResolver::new();
        let result = resolver.resolve("redis://{{host}}:6379");
        assert!(result.is_err());
    }

    #[test]
    fn test_resolve_no_placeholders() {
        let resolver = PropertiesResolver::new();
        assert_eq!(
            resolver.resolve("redis://localhost:6379"),
            Ok("redis://localhost:6379".to_string())
        );
    }

    #[test]
    fn test_resolve_default_value() {
        let resolver = PropertiesResolver::new();
        assert_eq!(
            resolver.resolve("redis://{{host:localhost}}:6379"),
            Ok("redis://localhost:6379".to_string())
        );
    }

    #[test]
    fn test_resolve_multiple_same_key() {
        let resolver = PropertiesResolver::new();
        resolver.set("host", "redis.example.com");
        assert_eq!(
            resolver.resolve("{{host}}:{{host}}"),
            Ok("redis.example.com:redis.example.com".to_string())
        );
    }

    #[test]
    fn test_resolve_unclosed_placeholder_passthrough() {
        let resolver = PropertiesResolver::new();
        assert_eq!(
            resolver.resolve("redis://{{host:localhost}:6379"),
            Ok("redis://{{host:localhost}:6379".to_string())
        );
    }

    #[test]
    fn test_resolve_env_placeholder() {
        unsafe {
            env::set_var("RUST_CAMEL_TEST_HOST", "redis.prod.example.com");
        }
        let resolver = PropertiesResolver::new();
        assert_eq!(
            resolver.resolve("redis://{{env:RUST_CAMEL_TEST_HOST}}:6379"),
            Ok("redis://redis.prod.example.com:6379".to_string())
        );
        unsafe {
            env::remove_var("RUST_CAMEL_TEST_HOST");
        }
    }

    #[test]
    fn test_resolve_env_missing_without_default_returns_error() {
        unsafe {
            env::remove_var("RUST_CAMEL_TEST_DEFINITELY_MISSING");
        }
        let resolver = PropertiesResolver::new();
        let result = resolver.resolve("{{env:RUST_CAMEL_TEST_DEFINITELY_MISSING}}");
        assert!(result.is_err());
    }

    #[test]
    fn test_resolve_env_with_default_uses_default_when_missing() {
        unsafe {
            env::remove_var("RUST_CAMEL_TEST_MISSING_HOST");
        }
        let resolver = PropertiesResolver::new();
        assert_eq!(
            resolver.resolve("redis://{{env:RUST_CAMEL_TEST_MISSING_HOST:localhost}}:6379"),
            Ok("redis://localhost:6379".to_string())
        );
    }

    #[test]
    fn test_resolve_env_with_default_uses_env_when_present() {
        unsafe {
            env::set_var("RUST_CAMEL_TEST_DB_HOST", "db.prod.example.com");
        }
        let resolver = PropertiesResolver::new();
        assert_eq!(
            resolver.resolve("{{env:RUST_CAMEL_TEST_DB_HOST:localhost}}"),
            Ok("db.prod.example.com".to_string())
        );
        unsafe {
            env::remove_var("RUST_CAMEL_TEST_DB_HOST");
        }
    }

    #[test]
    fn test_resolve_mixed_env_and_property() {
        unsafe {
            env::set_var("RUST_CAMEL_TEST_PORT", "6380");
        }
        let resolver = PropertiesResolver::new();
        resolver.set("host", "localhost");
        assert_eq!(
            resolver.resolve("redis://{{host}}:{{env:RUST_CAMEL_TEST_PORT}}"),
            Ok("redis://localhost:6380".to_string())
        );
        unsafe {
            env::remove_var("RUST_CAMEL_TEST_PORT");
        }
    }

    #[test]
    fn test_resolve_env_empty_key_returns_error() {
        let resolver = PropertiesResolver::new();
        let result = resolver.resolve("{{env:}}");
        assert!(result.is_err());
    }

    #[test]
    fn test_resolve_env_value_with_colons() {
        unsafe {
            env::set_var("RUST_CAMEL_TEST_CONN", "user:pass@host:5432");
        }
        let resolver = PropertiesResolver::new();
        assert_eq!(
            resolver.resolve("{{env:RUST_CAMEL_TEST_CONN}}"),
            Ok("user:pass@host:5432".to_string())
        );
        unsafe {
            env::remove_var("RUST_CAMEL_TEST_CONN");
        }
    }
}
