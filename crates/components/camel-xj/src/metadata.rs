use camel_component_api::UriConfig;

#[allow(dead_code)]
#[derive(UriConfig)]
#[uri_scheme = "xj"]
#[uri_config(
    skip_impl,
    metadata(
        scheme = "xj",
        description = "XJ XML<->JSON transformation via xml-bridge",
        producer
    ),
    crate = "camel_component_api"
)]
pub(super) struct XjMetadataDescriptor {
    #[uri_param(name = "direction", desc = "Transform direction: xml2json or json2xml")]
    pub _direction: String,

    #[uri_param(
        name = "maxPayloadBytes",
        desc = "Max payload size in bytes before rejecting"
    )]
    pub _max_payload_bytes: Option<usize>,

    #[uri_param(
        name = "retryCount",
        default = "3",
        desc = "Retry count for bridge operations"
    )]
    pub _retry_count: u32,

    #[uri_param(
        name = "retryDelayMs",
        default = "500",
        desc = "Retry delay in milliseconds"
    )]
    pub _retry_delay_ms: u64,

    #[uri_param(
        pattern = "param.",
        desc = "Open namespace: param.<name>=<value> stylesheet parameters"
    )]
    pub _params: Vec<(String, String)>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_api::component_metadata::UriOptionMatch;
    use camel_component_api::{ComponentMetadata, OptionKind};

    #[test]
    fn xj_metadata_uri_options_names() {
        let meta: ComponentMetadata = XjMetadataDescriptor::metadata();
        let mut names: Vec<&str> = meta.uri_options.iter().map(|o| o.name.as_str()).collect();
        names.sort();

        let mut expected = vec![
            "direction",
            "maxPayloadBytes",
            "param",
            "retryCount",
            "retryDelayMs",
        ];
        expected.sort();

        assert_eq!(
            names, expected,
            "metadata uri_options names must match expected set"
        );
    }

    #[test]
    fn xj_metadata_param_option_has_prefix_pattern() {
        let meta: ComponentMetadata = XjMetadataDescriptor::metadata();
        let param = meta
            .uri_options
            .iter()
            .find(|o| o.name == "param")
            .expect("param option must exist");
        assert_eq!(
            param.pattern,
            Some(UriOptionMatch::Prefix {
                separator: "param.".to_string()
            }),
            "param option must have Prefix pattern with separator 'param.'"
        );
        assert_eq!(
            param.kind,
            OptionKind::String,
            "param option kind must be String"
        );
    }

    #[test]
    fn xj_metadata_numeric_options_derive_int_kind() {
        let meta: ComponentMetadata = XjMetadataDescriptor::metadata();
        let numeric_options = ["maxPayloadBytes", "retryCount", "retryDelayMs"];
        for name in &numeric_options {
            let option = meta
                .uri_options
                .iter()
                .find(|o| o.name == *name)
                .unwrap_or_else(|| panic!("{} option must exist", name));
            assert_eq!(option.kind, OptionKind::Int, "{} must have kind Int", name);
        }
    }

    #[test]
    fn xj_metadata_direction_is_required() {
        let meta: ComponentMetadata = XjMetadataDescriptor::metadata();
        let direction = meta
            .uri_options
            .iter()
            .find(|o| o.name == "direction")
            .expect("direction option must exist");
        assert!(direction.required, "direction must be required");
    }
}
