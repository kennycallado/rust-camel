use camel_component_api::UriConfig;

#[allow(dead_code)]
#[derive(UriConfig)]
#[uri_scheme = "controlbus"]
#[uri_config(
    skip_impl,
    metadata(
        scheme = "controlbus",
        description = "Route lifecycle control producer",
        producer
    ),
    crate = "camel_component_api"
)]
pub(super) struct ControlBusMetadataDescriptor {
    #[uri_param(name = "routeId", required)]
    pub _route_id: String,

    #[uri_param(name = "action", required)]
    pub _action: String,

    #[uri_param(name = "authorizedRoutes", required)]
    pub _authorized_routes: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_component_api::ComponentMetadata;

    #[test]
    fn controlbus_metadata_uri_options_parity() {
        let meta: ComponentMetadata = ControlBusMetadataDescriptor::metadata();
        let mut names: Vec<&str> = meta.uri_options.iter().map(|o| o.name.as_str()).collect();
        names.sort();

        let mut expected = vec!["routeId", "action", "authorizedRoutes"];
        expected.sort();

        assert_eq!(
            names, expected,
            "metadata uri_options names must match parser keys"
        );

        // Verify all three carry required
        for name in &["routeId", "action", "authorizedRoutes"] {
            let opt = meta
                .uri_options
                .iter()
                .find(|o| o.name == *name)
                .unwrap_or_else(|| panic!("{name} not found in uri_options"));
            assert!(opt.required, "{name} must be required");
        }
    }
}
