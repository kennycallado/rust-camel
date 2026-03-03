pub mod hash;
pub mod key;
pub mod list;
pub mod other;
pub mod pubsub;
pub mod set;
pub mod string;
pub mod zset;

use camel_api::{CamelError, Exchange};

// ── Header extraction helpers ────────────────────────────────────────────────

pub fn get_str_header<'a>(exchange: &'a Exchange, key: &str) -> Option<&'a str> {
    exchange.input.header(key).and_then(|v| v.as_str())
}

pub fn get_u64_header(exchange: &Exchange, key: &str) -> Option<u64> {
    exchange.input.header(key).and_then(|v| v.as_u64())
}

pub fn get_i64_header(exchange: &Exchange, key: &str) -> Option<i64> {
    exchange.input.header(key).and_then(|v| v.as_i64())
}

pub fn get_f64_header(exchange: &Exchange, key: &str) -> Option<f64> {
    exchange.input.header(key).and_then(|v| v.as_f64())
}

pub fn get_bool_header(exchange: &Exchange, key: &str) -> Option<bool> {
    exchange.input.header(key).and_then(|v| v.as_bool())
}

pub fn get_str_vec_header(exchange: &Exchange, key: &str) -> Option<Vec<String>> {
    exchange.input.header(key).and_then(|v| {
        v.as_array().map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_str().map(|s| s.to_string()))
                .collect()
        })
    })
}

pub fn get_value_header(exchange: &Exchange, key: &str) -> Option<serde_json::Value> {
    exchange.input.header(key).cloned()
}

pub fn require_str_header<'a>(exchange: &'a Exchange, key: &str) -> Result<&'a str, CamelError> {
    get_str_header(exchange, key)
        .ok_or_else(|| CamelError::ProcessorError(format!("Missing required header: {}", key)))
}

pub fn require_key(exchange: &Exchange) -> Result<String, CamelError> {
    require_str_header(exchange, "CamelRedis.Key").map(|s| s.to_string())
}

pub fn require_value(exchange: &Exchange) -> Result<serde_json::Value, CamelError> {
    get_value_header(exchange, "CamelRedis.Value").ok_or_else(|| {
        CamelError::ProcessorError("Missing required header: CamelRedis.Value".into())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_api::{Exchange, Message};

    fn make_exchange_with_header(key: &str, val: serde_json::Value) -> Exchange {
        let mut msg = Message::default();
        msg.set_header(key, val);
        Exchange::new(msg)
    }

    #[test]
    fn test_get_str_header_found() {
        let ex =
            make_exchange_with_header("CamelRedis.Key", serde_json::Value::String("mykey".into()));
        assert_eq!(get_str_header(&ex, "CamelRedis.Key"), Some("mykey"));
    }

    #[test]
    fn test_get_str_header_missing() {
        let ex = Exchange::new(Message::default());
        assert_eq!(get_str_header(&ex, "CamelRedis.Key"), None);
    }

    #[test]
    fn test_get_u64_header() {
        let ex = make_exchange_with_header("CamelRedis.Timeout", serde_json::json!(30u64));
        assert_eq!(get_u64_header(&ex, "CamelRedis.Timeout"), Some(30));
    }

    #[test]
    fn test_get_f64_header() {
        let ex = make_exchange_with_header("CamelRedis.Score", serde_json::json!(3.15f64));
        assert_eq!(get_f64_header(&ex, "CamelRedis.Score"), Some(3.15));
    }

    #[test]
    fn test_get_i64_header() {
        let ex = make_exchange_with_header("CamelRedis.Start", serde_json::json!(-1i64));
        assert_eq!(get_i64_header(&ex, "CamelRedis.Start"), Some(-1));
    }

    #[test]
    fn test_get_bool_header() {
        let ex = make_exchange_with_header("CamelRedis.WithScore", serde_json::json!(true));
        assert_eq!(get_bool_header(&ex, "CamelRedis.WithScore"), Some(true));
    }

    #[test]
    fn test_get_str_vec_header() {
        let ex = make_exchange_with_header("CamelRedis.Keys", serde_json::json!(["a", "b", "c"]));
        assert_eq!(
            get_str_vec_header(&ex, "CamelRedis.Keys"),
            Some(vec!["a".to_string(), "b".to_string(), "c".to_string()])
        );
    }

    #[test]
    fn test_require_str_header_ok() {
        let ex = make_exchange_with_header("CamelRedis.Key", serde_json::Value::String("k".into()));
        assert_eq!(require_str_header(&ex, "CamelRedis.Key").unwrap(), "k");
    }

    #[test]
    fn test_require_str_header_missing_returns_err() {
        let ex = Exchange::new(Message::default());
        assert!(require_str_header(&ex, "CamelRedis.Key").is_err());
    }
}
