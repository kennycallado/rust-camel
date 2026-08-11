use camel_component_api::UriConfig;

#[allow(dead_code)]
#[derive(UriConfig)]
#[uri_scheme = "keycloak"]
#[uri_config(
    skip_impl,
    metadata(
        scheme = "keycloak",
        description = "Keycloak admin + event polling",
        producer,
        consumer
    ),
    crate = "camel_component_api"
)]
pub(super) struct KeycloakMetadataDescriptor {
    #[uri_param(name = "operation", required)]
    pub _operation: String,
    #[uri_param(name = "realm")]
    pub _realm: Option<String>,
    #[uri_param(name = "userId")]
    pub _user_id: Option<String>,
    #[uri_param(name = "eventType", required)]
    pub _event_type: String,
    #[uri_param(name = "pollDelay", default = "5000")]
    pub _poll_delay: u64,
    #[uri_param(name = "maxResults", default = "100")]
    pub _max_results: u32,
    #[uri_param(name = "lookbackWindow", default = "300000")]
    pub _lookback_window: u64,
    #[uri_param(name = "dedupCapacity", default = "10000")]
    pub _dedup_capacity: usize,
    #[uri_param(name = "maxAuthErrors", default = "3")]
    pub _max_auth_errors: u32,
    #[uri_param(name = "type")]
    pub _type_filter: Option<String>,
    #[uri_param(name = "client")]
    pub _client_filter: Option<String>,
    #[uri_param(name = "operationTypes")]
    pub _operation_types: Option<String>,
    #[uri_param(name = "resourcePath")]
    pub _resource_path: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keycloak_metadata_uri_options_parity() {
        let meta = KeycloakMetadataDescriptor::metadata();
        let mut names: Vec<&str> = meta.uri_options.iter().map(|o| o.name.as_str()).collect();
        names.sort();

        let mut expected = vec![
            "operation",
            "realm",
            "userId",
            "eventType",
            "pollDelay",
            "maxResults",
            "lookbackWindow",
            "dedupCapacity",
            "maxAuthErrors",
            "type",
            "client",
            "operationTypes",
            "resourcePath",
        ];
        expected.sort();

        assert_eq!(
            names, expected,
            "uri_options names must match union of admin + events keys"
        );

        // Verify required flags
        for opt in &meta.uri_options {
            match opt.name.as_str() {
                "operation" => assert!(opt.required, "operation must be required"),
                "eventType" => assert!(opt.required, "eventType must be required"),
                _ => assert!(!opt.required, "{} must not be required", opt.name),
            }
        }
    }
}
