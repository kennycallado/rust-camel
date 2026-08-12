use camel_component_api::UriConfig;

#[allow(dead_code)]
#[derive(UriConfig)]
#[uri_scheme = "kafka"]
#[uri_config(
    skip_impl,
    descriptor,
    metadata(
        scheme = "kafka",
        description = "Apache Kafka consumer/producer",
        producer,
        consumer
    ),
    crate = "camel_component_api"
)]
pub(super) struct KafkaMetadataDescriptor {
    #[uri_param(name = "brokers", required)]
    pub _brokers: String,

    #[uri_param(name = "groupId")]
    pub _group_id: String,

    #[uri_param(name = "autoOffsetReset")]
    pub _auto_offset_reset: String,

    #[uri_param(name = "sessionTimeoutMs")]
    pub _session_timeout_ms: Option<u32>,

    #[uri_param(name = "heartbeatIntervalMs")]
    pub _heartbeat_interval_ms: Option<u32>,

    #[uri_param(name = "pollTimeoutMs", default = "5000")]
    pub _poll_timeout_ms: u32,

    #[uri_param(name = "maxPollRecords", default = "500")]
    pub _max_poll_records: u32,

    #[uri_param(name = "acks", default = "all")]
    pub _acks: String,

    #[uri_param(name = "requestTimeoutMs")]
    pub _request_timeout_ms: Option<u32>,

    #[uri_param(name = "securityProtocol")]
    pub _security_protocol: Option<String>,

    #[uri_param(name = "saslAuthType", default = "NONE")]
    pub _sasl_auth_type: String,

    #[uri_param(name = "saslUsername")]
    pub _sasl_username: String,

    #[uri_param(name = "saslPassword", secret)]
    pub _sasl_password: String,

    #[uri_param(name = "sslKeystoreLocation")]
    pub _ssl_keystore_location: String,

    #[uri_param(name = "sslKeystorePassword", secret)]
    pub _ssl_keystore_password: String,

    #[uri_param(name = "sslTruststoreLocation")]
    pub _ssl_truststore_location: String,

    #[uri_param(name = "sslTruststorePassword", secret)]
    pub _ssl_truststore_password: String,

    #[uri_param(name = "allowManualCommit", default = "false")]
    pub _allow_manual_commit: bool,

    #[uri_param(name = "partitionAssignmentStrategy", default = "range")]
    pub _partition_assignment_strategy: String,

    #[uri_param(name = "clientId")]
    pub _client_id: String,

    #[uri_param(name = "brokerName")]
    pub _broker_name: String,

    #[uri_param(name = "commitTimeoutMs", default = "10000")]
    pub _commit_timeout_ms: u32,

    #[uri_param(name = "isolationLevel")]
    pub _isolation_level: String,

    #[uri_param(name = "dlqTopic")]
    pub _dlq_topic: String,

    #[uri_param(name = "dlqMaxRetries", default = "3")]
    pub _dlq_max_retries: u32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_component_api::ComponentMetadata;

    #[test]
    fn kafka_metadata_uri_options_parity() {
        let meta: ComponentMetadata = KafkaMetadataDescriptor::metadata();
        let mut names: Vec<&str> = meta.uri_options.iter().map(|o| o.name.as_str()).collect();
        names.sort();

        let mut expected = vec![
            "acks",
            "allowManualCommit",
            "autoOffsetReset",
            "brokerName",
            "brokers",
            "clientId",
            "commitTimeoutMs",
            "dlqMaxRetries",
            "dlqTopic",
            "groupId",
            "heartbeatIntervalMs",
            "isolationLevel",
            "maxPollRecords",
            "partitionAssignmentStrategy",
            "pollTimeoutMs",
            "requestTimeoutMs",
            "saslAuthType",
            "saslPassword",
            "saslUsername",
            "securityProtocol",
            "sessionTimeoutMs",
            "sslKeystoreLocation",
            "sslKeystorePassword",
            "sslTruststoreLocation",
            "sslTruststorePassword",
        ];
        expected.sort();

        assert_eq!(
            names, expected,
            "metadata uri_options names must match parser keys"
        );

        // Verify required flags
        let brokers = meta
            .uri_options
            .iter()
            .find(|o| o.name == "brokers")
            .unwrap();
        assert!(brokers.required, "brokers must be required");

        // Verify secret flags
        let sasl_password = meta
            .uri_options
            .iter()
            .find(|o| o.name == "saslPassword")
            .unwrap();
        assert!(sasl_password.secret, "saslPassword must be secret");

        let ssl_keystore_password = meta
            .uri_options
            .iter()
            .find(|o| o.name == "sslKeystorePassword")
            .unwrap();
        assert!(
            ssl_keystore_password.secret,
            "sslKeystorePassword must be secret"
        );

        let ssl_truststore_password = meta
            .uri_options
            .iter()
            .find(|o| o.name == "sslTruststorePassword")
            .unwrap();
        assert!(
            ssl_truststore_password.secret,
            "sslTruststorePassword must be secret"
        );
    }
}
