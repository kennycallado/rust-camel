use camel_component_api::UriConfig;

/// URI metadata descriptor for scheme `rabbitmq`.
///
/// The descriptor is the frozen P1 option set plus the frozen P2 option set plus
/// the frozen P3 delta plus the P4 delta (`config::P1_OPTIONS ∪
/// config::P2_OPTIONS ∪ config::P3_OPTIONS ∪ config::P4_OPTIONS`). Task 4.1
/// adds the single P4 option (`replyTimeout`); task 4.4 freezes the set.
#[allow(dead_code)]
#[derive(UriConfig)]
#[uri_scheme = "rabbitmq"]
#[uri_config(
    skip_impl,
    descriptor,
    metadata(
        scheme = "rabbitmq",
        description = "RabbitMQ AMQP 0-9-1 messaging",
        producer,
        consumer
    ),
    crate = "camel_component_api"
)]
pub(super) struct RabbitMqMetadataDescriptor {
    #[uri_param(name = "broker")]
    pub _broker: Option<String>,

    #[uri_param(name = "queue")]
    pub _queue: Option<String>,

    #[uri_param(name = "routingKey")]
    pub _routing_key: Option<String>,

    #[uri_param(name = "persistent", default = "true")]
    pub _persistent: bool,

    #[uri_param(name = "contentType")]
    pub _content_type: Option<String>,

    /// Requeue a failed route's delivery instead of rejecting it. WARNING:
    /// without a broker-side delivery limit this forms a hot requeue loop.
    #[uri_param(name = "requeueOnFailure", default = "false")]
    pub _requeue_on_failure: bool,

    /// Maximum unacknowledged deliveries per channel (`basic.qos`).
    #[uri_param(name = "prefetch", default = "10")]
    pub _prefetch: u16,

    /// Number of channel + consumer engines; prefetch applies per engine.
    #[uri_param(name = "concurrentConsumers", default = "1")]
    pub _concurrent_consumers: u32,

    /// Publisher-confirm wait bound in milliseconds (task 3.1).
    #[uri_param(name = "confirmTimeout", default = "5000")]
    pub _confirm_timeout: u64,

    /// InOut direct reply-to wait bound in milliseconds (task 4.1).
    #[uri_param(name = "replyTimeout", default = "30000")]
    pub _reply_timeout: u64,

    /// Return an unroutable publish as an exchange failure instead of letting
    /// the broker drop it (task 3.2).
    #[uri_param(name = "mandatory", default = "false")]
    pub _mandatory: bool,

    /// Opt-in active topology declaration at consumer start. The Camel
    /// spring-rabbitmq consumer defaults to `true`; this component requires the
    /// explicit opt-in because an active declare mutates broker topology
    /// (task 3.4).
    #[uri_param(name = "autoDeclare", default = "false")]
    pub _auto_declare: bool,

    /// Exchange kind for the active declare (task 3.4).
    #[uri_param(name = "exchangeType", default = "direct")]
    pub _exchange_type: String,

    /// Durability of the actively declared queue; the exchange is always
    /// durable (task 3.4).
    #[uri_param(name = "durableQueue", default = "true")]
    pub _durable_queue: bool,

    /// Queue arguments as a JSON object of string values (task 3.4).
    #[uri_param(name = "queueArguments")]
    pub _queue_arguments: Option<String>,
}

#[cfg(test)]
mod tests {
    use camel_component_api::ComponentMetadata;

    fn find<'a>(
        meta: &'a [camel_component_api::UriOption],
        name: &str,
    ) -> &'a camel_component_api::UriOption {
        meta.iter()
            .find(|o| o.name == name)
            .unwrap_or_else(|| panic!("uri_option '{name}' not found"))
    }

    #[test]
    fn metadata_parity_p4_frozen() {
        let meta: ComponentMetadata = super::RabbitMqMetadataDescriptor::metadata();

        let mut names: Vec<&str> = meta.uri_options.iter().map(|o| o.name.as_str()).collect();
        names.sort_unstable();

        // Task 4.4 freezes the complete phase set: the descriptor must expose
        // `P1 ∪ P2 ∪ P3 ∪ P4` exactly, with the P4 delta being the single
        // `replyTimeout` option. The expected set is derived from the phase
        // consts, never restated here.
        let mut expected: Vec<&str> = crate::config::P1_OPTIONS
            .iter()
            .chain(crate::config::P2_OPTIONS.iter())
            .chain(crate::config::P3_OPTIONS.iter())
            .chain(crate::config::P4_OPTIONS.iter())
            .copied()
            .collect();
        expected.sort_unstable();

        assert_eq!(
            names, expected,
            "metadata uri_options names must equal the frozen P1 ∪ P2 ∪ P3 ∪ P4 exactly"
        );

        for option in &meta.uri_options {
            assert!(!option.required, "{} must not be required", option.name);
        }

        let persistent = find(&meta.uri_options, "persistent");
        assert_eq!(persistent.default_value.as_deref(), Some("true"));
        assert!(!persistent.required);

        let requeue = find(&meta.uri_options, "requeueOnFailure");
        assert_eq!(requeue.default_value.as_deref(), Some("false"));
        assert!(!requeue.required);

        let prefetch = find(&meta.uri_options, "prefetch");
        assert_eq!(prefetch.default_value.as_deref(), Some("10"));
        assert!(!prefetch.required);

        let concurrent = find(&meta.uri_options, "concurrentConsumers");
        assert_eq!(concurrent.default_value.as_deref(), Some("1"));
        assert!(!concurrent.required);

        let confirm_timeout = find(&meta.uri_options, "confirmTimeout");
        assert_eq!(confirm_timeout.default_value.as_deref(), Some("5000"));
        assert!(!confirm_timeout.required);

        let mandatory = find(&meta.uri_options, "mandatory");
        assert_eq!(mandatory.default_value.as_deref(), Some("false"));
        assert!(!mandatory.required);

        // Task 3.4 options.
        let auto_declare = find(&meta.uri_options, "autoDeclare");
        assert_eq!(auto_declare.default_value.as_deref(), Some("false"));
        assert!(!auto_declare.required);

        let exchange_type = find(&meta.uri_options, "exchangeType");
        assert_eq!(exchange_type.default_value.as_deref(), Some("direct"));
        assert!(!exchange_type.required);

        let durable_queue = find(&meta.uri_options, "durableQueue");
        assert_eq!(durable_queue.default_value.as_deref(), Some("true"));
        assert!(!durable_queue.required);

        let queue_arguments = find(&meta.uri_options, "queueArguments");
        assert_eq!(queue_arguments.default_value.as_deref(), None);
        assert!(!queue_arguments.required);

        // Task 4.1: the P4 `replyTimeout` option, default 30000 ms.
        let reply_timeout = find(&meta.uri_options, "replyTimeout");
        assert_eq!(reply_timeout.default_value.as_deref(), Some("30000"));
        assert!(!reply_timeout.required);

        assert!(
            meta.capabilities.supports_producer,
            "the descriptor advertises the producer capability"
        );
        assert!(
            meta.capabilities.supports_consumer,
            "task 2.1 added the consumer capability"
        );
    }

    #[test]
    fn metadata_capabilities_gain_consumer() {
        let meta: ComponentMetadata = super::RabbitMqMetadataDescriptor::metadata();

        assert!(
            meta.capabilities.supports_producer,
            "the producer capability must remain advertised"
        );
        assert!(
            meta.capabilities.supports_consumer,
            "the consumer capability must be advertised from task 2.1 on"
        );
    }
}
