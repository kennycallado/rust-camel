use camel_component_api::UriConfig;

#[allow(dead_code)]
#[derive(UriConfig)]
#[uri_scheme = "cxf"]
#[uri_config(
    skip_impl,
    descriptor,
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

    #[uri_param(name = "profile", required)]
    pub _profile: String,

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

    fn find<'a>(
        meta: &'a [camel_component_api::UriOption],
        name: &str,
    ) -> &'a camel_component_api::UriOption {
        meta.iter()
            .find(|o| o.name == name)
            .unwrap_or_else(|| panic!("uri_option '{}' not found", name))
    }

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

        // ── required assertions ──────────────────────────────────────────────

        // Explicitly required fields
        assert!(
            find(&meta.uri_options, "wsdl").required,
            "wsdl must be required"
        );
        assert!(
            find(&meta.uri_options, "service").required,
            "service must be required"
        );
        assert!(
            find(&meta.uri_options, "port").required,
            "port must be required"
        );
        assert!(
            find(&meta.uri_options, "profile").required,
            "profile must be required"
        );

        // descriptor-defaulted fields (not required)
        assert!(
            !find(&meta.uri_options, "operation").required,
            "operation must not be required"
        );
        assert!(
            !find(&meta.uri_options, "timeout_ms").required,
            "timeout_ms must not be required"
        );
        assert!(
            !find(&meta.uri_options, "mtom_enabled").required,
            "mtom_enabled must not be required"
        );
        assert!(
            !find(&meta.uri_options, "attachment_content_type").required,
            "attachment_content_type must not be required"
        );
    }
}
