use camel_component_api::UriConfig;

#[allow(dead_code)]
#[derive(UriConfig)]
#[uri_scheme = "xslt"]
#[uri_config(
    skip_impl,
    metadata(
        scheme = "xslt",
        description = "XSLT 3.0 transformation via xml-bridge",
        producer
    ),
    crate = "camel_component_api"
)]
pub(super) struct XsltMetadataDescriptor {
    #[uri_param(name = "output", desc = "Output method: xml, html, or text")]
    pub _output_method: Option<String>,

    #[uri_param(
        name = "transformerCacheSize",
        desc = "Max compiled stylesheets to keep in cache"
    )]
    pub _transformer_cache_size: Option<usize>,

    #[uri_param(
        name = "failOnNullBody",
        default = "false",
        desc = "Fail if input body is null or empty"
    )]
    pub _fail_on_null_body: bool,

    #[uri_param(name = "maxPayloadBytes", desc = "Max payload size in bytes")]
    pub _max_payload_bytes: Option<usize>,

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
    fn xslt_metadata_uri_options_names() {
        let meta: ComponentMetadata = XsltMetadataDescriptor::metadata();
        let mut names: Vec<&str> = meta.uri_options.iter().map(|o| o.name.as_str()).collect();
        names.sort();

        let mut expected = vec![
            "failOnNullBody",
            "maxPayloadBytes",
            "output",
            "param",
            "transformerCacheSize",
        ];
        expected.sort();

        assert_eq!(
            names, expected,
            "metadata uri_options names must match expected set"
        );
    }

    #[test]
    fn xslt_metadata_param_option_has_prefix_pattern() {
        let meta: ComponentMetadata = XsltMetadataDescriptor::metadata();
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
    fn xslt_metadata_numeric_options_derive_int_kind() {
        let meta: ComponentMetadata = XsltMetadataDescriptor::metadata();
        let numeric_options = ["transformerCacheSize", "maxPayloadBytes"];
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
    fn xslt_metadata_no_required_options() {
        let meta: ComponentMetadata = XsltMetadataDescriptor::metadata();
        for option in &meta.uri_options {
            assert!(!option.required, "{} must not be required", option.name);
        }
    }
}
