use camel_component_api::UriConfig;

#[allow(dead_code)]
#[derive(UriConfig)]
#[uri_scheme = "llm"]
#[uri_config(
    skip_impl,
    metadata(scheme = "llm", description = "LLM chat / embed producer", producer),
    crate = "camel_component_api"
)]
pub(super) struct LlmMetadataDescriptor {
    #[uri_param(name = "stream", default = "true")]
    pub _stream: bool,
    #[uri_param(name = "provider")]
    pub _provider: Option<String>,
    #[uri_param(name = "model")]
    pub _model: Option<String>,
    #[uri_param(name = "temperature")]
    pub _temperature: Option<f64>,
    #[uri_param(name = "max_tokens")]
    pub _max_tokens: Option<u32>,
    #[uri_param(name = "system_prompt")]
    pub _system_prompt: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_component_api::ComponentMetadata;

    #[test]
    fn llm_metadata_uri_options_parity() {
        let meta: ComponentMetadata = LlmMetadataDescriptor::metadata();
        let names: std::collections::BTreeSet<&str> =
            meta.uri_options.iter().map(|o| o.name.as_str()).collect();
        let expected: std::collections::BTreeSet<&str> = [
            "stream",
            "provider",
            "model",
            "temperature",
            "max_tokens",
            "system_prompt",
        ]
        .into_iter()
        .collect();
        assert_eq!(names, expected, "URI option names must match parser keys");
    }
}
