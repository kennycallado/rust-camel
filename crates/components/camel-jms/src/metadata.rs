use camel_component_api::UriConfig;

#[allow(dead_code)]
#[derive(UriConfig)]
#[uri_scheme = "jms"]
#[uri_config(
    skip_impl,
    descriptor,
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

    #[uri_param(name = "acknowledgementMode", default = "Auto")]
    pub _acknowledgement_mode: String,

    #[uri_param(name = "messageSelector")]
    pub _message_selector: Option<String>,

    #[uri_param(name = "concurrentConsumers", default = "1")]
    pub _concurrent_consumers: u32,

    #[uri_param(name = "transactionMode", default = "None")]
    pub _transaction_mode: String,

    #[uri_param(name = "timeToLive")]
    pub _time_to_live: Option<u64>,

    #[uri_param(name = "priority")]
    pub _priority: Option<u8>,

    #[uri_param(name = "persistentDelivery", default = "true")]
    pub _persistent_delivery: bool,

    #[uri_param(name = "mapJmsHeaders", default = "true")]
    pub _map_jms_headers: bool,

    #[uri_param(name = "exchangePattern", default = "InOnly")]
    pub _exchange_pattern: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_component_api::ComponentMetadata;

    fn find<'a>(
        meta: &'a [camel_component_api::UriOption],
        name: &str,
    ) -> &'a camel_component_api::UriOption {
        meta.iter()
            .find(|o| o.name == name)
            .unwrap_or_else(|| panic!("uri_option '{}' not found", name))
    }

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

        // ── required + default_value assertions ──────────────────────────────

        // Option<String> fields — never required
        assert!(!find(&meta.uri_options, "broker").required);
        assert!(!find(&meta.uri_options, "messageSelector").required);
        assert!(!find(&meta.uri_options, "timeToLive").required);
        assert!(!find(&meta.uri_options, "priority").required);

        // descriptor-defaulted fields — not required, carry runtime default
        let ack = find(&meta.uri_options, "acknowledgementMode");
        assert_eq!(ack.default_value.as_deref(), Some("Auto"));
        assert!(!ack.required);

        let tx = find(&meta.uri_options, "transactionMode");
        assert_eq!(tx.default_value.as_deref(), Some("None"));
        assert!(!tx.required);

        let ep = find(&meta.uri_options, "exchangePattern");
        assert_eq!(ep.default_value.as_deref(), Some("InOnly"));
        assert!(!ep.required);

        // Existing defaults — keep
        assert_eq!(
            find(&meta.uri_options, "concurrentConsumers")
                .default_value
                .as_deref(),
            Some("1")
        );
        assert!(!find(&meta.uri_options, "concurrentConsumers").required);
        assert_eq!(
            find(&meta.uri_options, "persistentDelivery")
                .default_value
                .as_deref(),
            Some("true")
        );
        assert!(!find(&meta.uri_options, "persistentDelivery").required);
        assert_eq!(
            find(&meta.uri_options, "mapJmsHeaders")
                .default_value
                .as_deref(),
            Some("true")
        );
        assert!(!find(&meta.uri_options, "mapJmsHeaders").required);
    }
}
