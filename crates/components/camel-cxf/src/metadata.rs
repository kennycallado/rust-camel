use camel_component_api::UriConfig;

#[allow(dead_code)]
#[derive(UriConfig)]
#[uri_scheme = "cxf"]
#[uri_config(
    skip_impl,
    metadata(
        scheme = "cxf",
        description = "CXF/SOAP WebService consumer/producer",
        producer,
        consumer
    ),
    crate = "camel_component_api"
)]
pub(super) struct CxfMetadataDescriptor {
    #[uri_param(name = "wsdl", required)]
    pub _wsdl: String,

    #[uri_param(name = "service", required)]
    pub _service: String,

    #[uri_param(name = "port", required)]
    pub _port: String,

    #[uri_param(name = "operation")]
    pub _operation: String,

    #[uri_param(name = "profile")]
    pub _profile: Option<String>,

    #[uri_param(name = "timeout_ms")]
    pub _timeout_ms: Option<u64>,

    #[uri_param(name = "mtom_enabled", default = "false")]
    pub _mtom_enabled: bool,

    #[uri_param(name = "attachment_content_type")]
    pub _attachment_content_type: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_component_api::ComponentMetadata;

    #[test]
    fn cxf_metadata_uri_options_parity() {
        let meta: ComponentMetadata = CxfMetadataDescriptor::metadata();
        let mut names: Vec<&str> = meta.uri_options.iter().map(|o| o.name.as_str()).collect();
        names.sort();

        let mut expected = vec![
            "attachment_content_type",
            "mtom_enabled",
            "operation",
            "port",
            "profile",
            "service",
            "timeout_ms",
            "wsdl",
        ];
        expected.sort();

        assert_eq!(
            names, expected,
            "metadata uri_options names must match parser keys"
        );

        // Verify required flags
        let wsdl = meta.uri_options.iter().find(|o| o.name == "wsdl").unwrap();
        assert!(wsdl.required, "wsdl must be required");

        let service = meta
            .uri_options
            .iter()
            .find(|o| o.name == "service")
            .unwrap();
        assert!(service.required, "service must be required");

        let port = meta.uri_options.iter().find(|o| o.name == "port").unwrap();
        assert!(port.required, "port must be required");
    }
}
