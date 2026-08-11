use camel_component_api::UriConfig;

#[allow(dead_code)]
#[derive(UriConfig)]
#[uri_scheme = "grpc"]
#[uri_config(
    skip_impl,
    metadata(
        scheme = "grpc",
        description = "gRPC client/server",
        producer,
        consumer
    ),
    crate = "camel_component_api"
)]
pub(super) struct GrpcMetadataDescriptor {
    #[uri_param(name = "transport", required)]
    pub _transport: String,

    #[uri_param(name = "protoFile", required)]
    pub _proto_file: String,

    #[uri_param(name = "service")]
    pub _service: String,

    #[uri_param(name = "method")]
    pub _method: String,

    #[uri_param(name = "metadata")]
    pub _metadata: String,

    #[uri_param(name = "reflection", default = "false")]
    pub _reflection: bool,

    #[uri_param(name = "caCertPath")]
    pub _ca_cert_path: String,

    #[uri_param(name = "clientCertPath")]
    pub _client_cert_path: String,

    #[uri_param(name = "clientKeyPath")]
    pub _client_key_path: String,

    #[uri_param(name = "serverName")]
    pub _server_name: String,

    #[uri_param(name = "serverCertPath")]
    pub _server_cert_path: String,

    #[uri_param(name = "serverKeyPath")]
    pub _server_key_path: String,

    #[uri_param(name = "clientCaPath")]
    pub _client_ca_path: String,

    #[uri_param(name = "max_receive_message_length", default = "4194304")]
    pub _max_receive_message_length: u64,

    #[uri_param(name = "deadline_ms")]
    pub _deadline_ms: Option<u64>,

    #[uri_param(name = "connectTimeoutMs", default = "10000")]
    pub _connect_timeout_ms: u64,

    #[uri_param(name = "defaultDeadlineMs", default = "30000")]
    pub _default_deadline_ms: u64,

    #[uri_param(name = "bearerToken", secret)]
    pub _bearer_token: String,

    #[uri_param(name = "googleServiceAccount", secret)]
    pub _google_service_account: String,

    #[uri_param(name = "consumerStrategy")]
    pub _consumer_strategy: String,

    #[uri_param(name = "producerStrategy")]
    pub _producer_strategy: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_component_api::ComponentMetadata;

    #[test]
    fn grpc_metadata_uri_options_parity() {
        let meta: ComponentMetadata = GrpcMetadataDescriptor::metadata();
        let mut names: Vec<&str> = meta.uri_options.iter().map(|o| o.name.as_str()).collect();
        names.sort();

        let mut expected = vec![
            "bearerToken",
            "caCertPath",
            "clientCaPath",
            "clientCertPath",
            "clientKeyPath",
            "connectTimeoutMs",
            "consumerStrategy",
            "deadline_ms",
            "defaultDeadlineMs",
            "googleServiceAccount",
            "max_receive_message_length",
            "metadata",
            "method",
            "producerStrategy",
            "protoFile",
            "reflection",
            "serverCertPath",
            "serverKeyPath",
            "serverName",
            "service",
            "transport",
        ];
        expected.sort();

        assert_eq!(
            names, expected,
            "metadata uri_options names must match parser keys"
        );

        // Verify required flags
        let transport = meta
            .uri_options
            .iter()
            .find(|o| o.name == "transport")
            .unwrap();
        assert!(transport.required, "transport must be required");

        let proto_file = meta
            .uri_options
            .iter()
            .find(|o| o.name == "protoFile")
            .unwrap();
        assert!(proto_file.required, "protoFile must be required");

        // Verify tls is absent (legacy param removed per ADR-0033)
        assert!(
            meta.uri_options.iter().all(|o| o.name != "tls"),
            "legacy 'tls' param must not appear in metadata"
        );

        // Verify secret flags
        let bearer_token = meta
            .uri_options
            .iter()
            .find(|o| o.name == "bearerToken")
            .unwrap();
        assert!(bearer_token.secret, "bearerToken must be secret");

        let google_service_account = meta
            .uri_options
            .iter()
            .find(|o| o.name == "googleServiceAccount")
            .unwrap();
        assert!(
            google_service_account.secret,
            "googleServiceAccount must be secret"
        );
    }
}
