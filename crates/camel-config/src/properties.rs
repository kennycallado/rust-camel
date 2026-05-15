use std::collections::HashMap;
use std::sync::RwLock;

pub struct PropertiesResolver {
    properties: RwLock<HashMap<String, String>>,
}

#[derive(Debug, PartialEq, thiserror::Error)]
pub enum ResolveError {
    #[error("missing property key: {key}")]
    MissingKey { key: String },
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
            .unwrap()
            .insert(key.to_string(), value.to_string());
    }

    pub fn resolve(&self, input: &str) -> Result<String, ResolveError> {
        let props = self.properties.read().unwrap();
        let mut result = input.to_string();
        let mut start = 0;

        while let Some(open) = result[start..].find("{{") {
            let open_abs = start + open;
            if let Some(close) = result[open_abs..].find("}}") {
                let close_abs = open_abs + close + 2;
                let inner = &result[open_abs + 2..open_abs + close];

                let (key, default) = if let Some(colon) = inner.find(':') {
                    (&inner[..colon], Some(&inner[colon + 1..]))
                } else {
                    (inner, None)
                };

                // Treat empty key (e.g. "{{}}") as missing
                if key.trim().is_empty() {
                    return Err(ResolveError::MissingKey {
                        key: key.to_string(),
                    });
                }

                let value = match (props.get(key), default) {
                    (Some(v), _) => v.clone(),
                    (None, Some(d)) => d.to_string(),
                    (None, None) => {
                        return Err(ResolveError::MissingKey {
                            key: key.to_string(),
                        })
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
}
