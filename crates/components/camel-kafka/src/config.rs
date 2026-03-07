use camel_api::CamelError;
use camel_endpoint::parse_uri;

#[derive(Debug, Clone)]
pub struct KafkaConfig {
    pub topic: String,
    pub brokers: String,
    pub group_id: String,
    pub auto_offset_reset: String,
    pub session_timeout_ms: u32,
    pub poll_timeout_ms: u32,
    pub max_poll_records: u32,
    pub acks: String,
    pub request_timeout_ms: u32,
}

impl KafkaConfig {
    pub fn from_uri(uri: &str) -> Result<Self, CamelError> {
        let parts = parse_uri(uri)?;

        if parts.scheme != "kafka" {
            return Err(CamelError::InvalidUri(format!(
                "expected scheme 'kafka', got '{}'",
                parts.scheme
            )));
        }

        // Topic is the path component (e.g., "kafka:orders" → path = "orders")
        let topic = parts.path.trim_start_matches('/').to_string();
        if topic.is_empty() {
            return Err(CamelError::InvalidUri(
                "Kafka URI must specify a topic (e.g. kafka:my-topic)".to_string(),
            ));
        }

        let brokers = parts
            .params
            .get("brokers")
            .cloned()
            .unwrap_or_else(|| "localhost:9092".to_string());

        let group_id = parts
            .params
            .get("groupId")
            .cloned()
            .unwrap_or_else(|| "camel".to_string());

        let auto_offset_reset = parts
            .params
            .get("autoOffsetReset")
            .cloned()
            .unwrap_or_else(|| "latest".to_string());

        let session_timeout_ms = parts
            .params
            .get("sessionTimeoutMs")
            .and_then(|s| s.parse().ok())
            .unwrap_or(45000u32);

        let poll_timeout_ms = parts
            .params
            .get("pollTimeoutMs")
            .and_then(|s| s.parse().ok())
            .unwrap_or(5000u32);

        let max_poll_records = parts
            .params
            .get("maxPollRecords")
            .and_then(|s| s.parse().ok())
            .unwrap_or(500u32);

        let acks = parts
            .params
            .get("acks")
            .cloned()
            .unwrap_or_else(|| "all".to_string());

        if !matches!(acks.as_str(), "0" | "1" | "all") {
            return Err(CamelError::InvalidUri(format!(
                "acks must be '0', '1', or 'all', got '{acks}'"
            )));
        }

        let auto_offset_reset_val = auto_offset_reset.as_str();
        if !matches!(auto_offset_reset_val, "earliest" | "latest" | "none") {
            return Err(CamelError::InvalidUri(format!(
                "autoOffsetReset must be 'earliest', 'latest', or 'none', got '{auto_offset_reset}'"
            )));
        }

        let request_timeout_ms = parts
            .params
            .get("requestTimeoutMs")
            .and_then(|s| s.parse().ok())
            .unwrap_or(30000u32);

        Ok(Self {
            topic,
            brokers,
            group_id,
            auto_offset_reset,
            session_timeout_ms,
            poll_timeout_ms,
            max_poll_records,
            acks,
            request_timeout_ms,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_defaults() {
        let c = KafkaConfig::from_uri("kafka:orders").unwrap();
        assert_eq!(c.topic, "orders");
        assert_eq!(c.brokers, "localhost:9092");
        assert_eq!(c.group_id, "camel");
        assert_eq!(c.auto_offset_reset, "latest");
        assert_eq!(c.session_timeout_ms, 45000);
        assert_eq!(c.poll_timeout_ms, 5000);
        assert_eq!(c.max_poll_records, 500);
        assert_eq!(c.acks, "all");
        assert_eq!(c.request_timeout_ms, 30000);
    }

    #[test]
    fn test_config_custom_params() {
        let c = KafkaConfig::from_uri(
            "kafka:events?brokers=kafka:9092&groupId=svc&autoOffsetReset=earliest\
             &sessionTimeoutMs=10000&pollTimeoutMs=1000&maxPollRecords=100\
             &acks=1&requestTimeoutMs=5000",
        )
        .unwrap();
        assert_eq!(c.topic, "events");
        assert_eq!(c.brokers, "kafka:9092");
        assert_eq!(c.group_id, "svc");
        assert_eq!(c.auto_offset_reset, "earliest");
        assert_eq!(c.session_timeout_ms, 10000);
        assert_eq!(c.poll_timeout_ms, 1000);
        assert_eq!(c.max_poll_records, 100);
        assert_eq!(c.acks, "1");
        assert_eq!(c.request_timeout_ms, 5000);
    }

    #[test]
    fn test_config_missing_topic_fails() {
        // Empty topic in path should fail
        let result = KafkaConfig::from_uri("kafka:?brokers=localhost:9092");
        assert!(result.is_err());
    }

    #[test]
    fn test_config_invalid_scheme_fails() {
        let result = KafkaConfig::from_uri("redis:orders");
        assert!(result.is_err());
    }

    #[test]
    fn test_config_invalid_acks_fails() {
        let result = KafkaConfig::from_uri("kafka:orders?acks=invalid");
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("acks"), "error should mention 'acks': {msg}");
    }

    #[test]
    fn test_config_invalid_auto_offset_reset_fails() {
        let result = KafkaConfig::from_uri("kafka:orders?autoOffsetReset=bad");
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("autoOffsetReset"),
            "error should mention 'autoOffsetReset': {msg}"
        );
    }
}
