/// Value type alias for dynamic header/property values.
pub type Value = serde_json::Value;

/// Headers type alias.
pub type Headers = std::collections::HashMap<String, Value>;
