use camel_component_api::UriConfig;

#[allow(dead_code)]
#[derive(UriConfig)]
#[uri_scheme = "jms"]
#[uri_config(
    skip_impl,
    metadata(
        scheme = "jms",
        description = "JMS / ActiveMQ / Artemis messaging",
        producer,
        consumer
    ),
    crate = "camel_component_api"
)]
pub(super) struct JmsMetadataDescriptor {
    #[uri_param(name = "broker")]
    pub _broker: Option<String>,

    #[uri_param(name = "acknowledgementMode")]
    pub _acknowledgement_mode: String,

    #[uri_param(name = "messageSelector")]
    pub _message_selector: Option<String>,

    #[uri_param(name = "concurrentConsumers", default = "1")]
    pub _concurrent_consumers: u32,

    #[uri_param(name = "transactionMode")]
    pub _transaction_mode: String,

    #[uri_param(name = "timeToLive")]
    pub _time_to_live: Option<u64>,

    #[uri_param(name = "priority")]
    pub _priority: Option<u8>,

    #[uri_param(name = "persistentDelivery", default = "true")]
    pub _persistent_delivery: bool,

    #[uri_param(name = "mapJmsHeaders", default = "true")]
    pub _map_jms_headers: bool,

    #[uri_param(name = "exchangePattern")]
    pub _exchange_pattern: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_component_api::ComponentMetadata;

    #[test]
    fn jms_metadata_uri_options_parity() {
        let meta: ComponentMetadata = JmsMetadataDescriptor::metadata();
        let mut names: Vec<&str> = meta.uri_options.iter().map(|o| o.name.as_str()).collect();
        names.sort();

        let mut expected = vec![
            "acknowledgementMode",
            "broker",
            "concurrentConsumers",
            "exchangePattern",
            "mapJmsHeaders",
            "messageSelector",
            "persistentDelivery",
            "priority",
            "timeToLive",
            "transactionMode",
        ];
        expected.sort();

        assert_eq!(
            names, expected,
            "metadata uri_options names must match parser keys"
        );

        // Verify defaults
        let concurrent = meta
            .uri_options
            .iter()
            .find(|o| o.name == "concurrentConsumers")
            .unwrap();
        assert_eq!(
            concurrent.default_value.as_deref(),
            Some("1"),
            "concurrentConsumers default must match parser"
        );

        let persistent = meta
            .uri_options
            .iter()
            .find(|o| o.name == "persistentDelivery")
            .unwrap();
        assert_eq!(
            persistent.default_value.as_deref(),
            Some("true"),
            "persistentDelivery default must match parser"
        );

        let map_headers = meta
            .uri_options
            .iter()
            .find(|o| o.name == "mapJmsHeaders")
            .unwrap();
        assert_eq!(
            map_headers.default_value.as_deref(),
            Some("true"),
            "mapJmsHeaders default must match parser"
        );
    }
}
