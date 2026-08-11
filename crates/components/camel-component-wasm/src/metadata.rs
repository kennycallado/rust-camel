use camel_component_api::UriConfig;

#[allow(dead_code)]
#[derive(UriConfig)]
#[uri_scheme = "wasm"]
#[uri_config(
    skip_impl,
    metadata(
        scheme = "wasm",
        description = "WebAssembly component",
        producer,
        consumer
    ),
    crate = "camel_component_api"
)]
pub(super) struct WasmMetadataDescriptor {
    #[uri_param(name = "timeout", default = "30")]
    pub _timeout: u64,

    #[uri_param(name = "max-memory", default = "52428800")]
    pub _max_memory: u64,

    #[uri_param(name = "max-concurrent-calls", default = "4")]
    pub _max_concurrent_calls: u64,

    #[uri_param(name = "max-wasm-size", default = "10485760")]
    pub _max_wasm_size: u64,

    #[uri_param(name = "allow-call", default = "")]
    pub _allow_call: String,

    #[uri_param(name = "max-stream-bytes", default = "10485760")]
    pub _max_stream_bytes: u64,

    #[uri_param(name = "max-instances", default = "10000")]
    pub _max_instances: u64,

    #[uri_param(name = "max-tables", default = "10000")]
    pub _max_tables: u64,

    #[uri_param(name = "max-table-elements")]
    pub _max_table_elements: Option<u64>,

    #[uri_param(name = "max-kv-entries", default = "256")]
    pub _max_kv_entries: u64,

    #[uri_param(name = "max-key-bytes", default = "1024")]
    pub _max_key_bytes: u64,

    #[uri_param(name = "max-value-bytes", default = "65536")]
    pub _max_value_bytes: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_component_api::ComponentMetadata;

    #[test]
    fn wasm_metadata_uri_options_parity() {
        let meta: ComponentMetadata = WasmMetadataDescriptor::metadata();
        let mut names: Vec<&str> = meta.uri_options.iter().map(|o| o.name.as_str()).collect();
        names.sort();

        let mut expected = vec![
            "allow-call",
            "max-concurrent-calls",
            "max-instances",
            "max-key-bytes",
            "max-kv-entries",
            "max-memory",
            "max-stream-bytes",
            "max-table-elements",
            "max-tables",
            "max-value-bytes",
            "max-wasm-size",
            "timeout",
        ];
        expected.sort();

        assert_eq!(
            names, expected,
            "metadata uri_options names must match parser keys"
        );
    }
}
