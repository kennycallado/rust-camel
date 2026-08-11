use camel_component_api::UriConfig;

#[allow(dead_code)]
#[derive(UriConfig)]
#[uri_scheme = "validator"]
#[uri_config(
    skip_impl,
    metadata(
        scheme = "validator",
        description = "Schema validation producer (XSD, JSON Schema, YAML)",
        producer
    ),
    crate = "camel_component_api"
)]
pub(super) struct ValidatorMetadataDescriptor {
    #[uri_param(name = "type")]
    pub _type: String,

    #[uri_param(name = "maxPayloadBytes")]
    pub _max_payload_bytes: Option<u64>,

    #[uri_param(name = "schemaCacheMaxEntries", default = "256")]
    pub _schema_cache_max_entries: u64,

    #[uri_param(name = "failOnNullBody", default = "true")]
    pub _fail_on_null_body: bool,

    #[uri_param(name = "headerName")]
    pub _header_name: String,

    #[uri_param(name = "failOnNullHeader", default = "true")]
    pub _fail_on_null_header: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_component_api::ComponentMetadata;

    #[test]
    fn validator_metadata_uri_options_parity() {
        let meta: ComponentMetadata = ValidatorMetadataDescriptor::metadata();
        let mut names: Vec<&str> = meta.uri_options.iter().map(|o| o.name.as_str()).collect();
        names.sort();

        let mut expected = vec![
            "failOnNullBody",
            "failOnNullHeader",
            "headerName",
            "maxPayloadBytes",
            "schemaCacheMaxEntries",
            "type",
        ];
        expected.sort();

        assert_eq!(
            names, expected,
            "metadata uri_options names must match parser keys"
        );

        // Verify defaults
        let cache = meta
            .uri_options
            .iter()
            .find(|o| o.name == "schemaCacheMaxEntries")
            .unwrap();
        assert_eq!(
            cache.default_value.as_deref(),
            Some("256"),
            "schemaCacheMaxEntries default must match parser"
        );

        let fail_body = meta
            .uri_options
            .iter()
            .find(|o| o.name == "failOnNullBody")
            .unwrap();
        assert_eq!(
            fail_body.default_value.as_deref(),
            Some("true"),
            "failOnNullBody default must match parser"
        );

        let fail_header = meta
            .uri_options
            .iter()
            .find(|o| o.name == "failOnNullHeader")
            .unwrap();
        assert_eq!(
            fail_header.default_value.as_deref(),
            Some("true"),
            "failOnNullHeader default must match parser"
        );
    }
}
