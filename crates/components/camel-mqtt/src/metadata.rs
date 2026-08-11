use camel_component_api::UriConfig;

#[allow(dead_code)]
#[derive(UriConfig)]
#[uri_scheme = "mqtt"]
#[uri_config(
    skip_impl,
    metadata(scheme = "mqtt", description = "MQTT messaging", producer, consumer),
    crate = "camel_component_api"
)]
pub(super) struct MqttMetadataDescriptor {
    #[uri_param(name = "topics")]
    pub _topics: String,

    #[uri_param(name = "qos", default = "1")]
    pub _qos: String,

    #[uri_param(name = "ackMode", default = "auto")]
    pub _ack_mode: String,

    #[uri_param(name = "cleanSession", default = "true")]
    pub _clean_session: bool,

    #[uri_param(name = "retain", default = "false")]
    pub _retain: bool,

    #[uri_param(name = "keepAliveSecs", default = "60")]
    pub _keep_alive_secs: u64,

    #[uri_param(name = "maxPayloadBytes", default = "262144")]
    pub _max_payload_bytes: u64,

    #[uri_param(name = "clientId")]
    pub _client_id: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_component_api::ComponentMetadata;

    #[test]
    fn mqtt_metadata_uri_options_parity() {
        let meta: ComponentMetadata = MqttMetadataDescriptor::metadata();
        let mut names: Vec<&str> = meta.uri_options.iter().map(|o| o.name.as_str()).collect();
        names.sort();

        let mut expected = vec![
            "ackMode",
            "cleanSession",
            "clientId",
            "keepAliveSecs",
            "maxPayloadBytes",
            "qos",
            "retain",
            "topics",
        ];
        expected.sort();

        assert_eq!(
            names, expected,
            "metadata uri_options names must match parser keys"
        );

        // Verify qos carries default = "1" (parser: Some("1") | None => AtLeastOnce)
        let qos = meta
            .uri_options
            .iter()
            .find(|o| o.name == "qos")
            .expect("qos must be present");
        assert_eq!(
            qos.default_value.as_deref(),
            Some("1"),
            "qos must carry default = \"1\" (matches parser AtLeastOnce default)"
        );
    }
}
