use camel_api::component_metadata::UriOptionMatch;
use camel_component_api::{Component, ComponentMetadata};
use camel_xj::XjComponent;

#[test]
fn xj_component_metadata_non_empty_via_override() {
    let component = XjComponent::default();
    let meta: ComponentMetadata = component.metadata();

    assert_eq!(
        meta.uri_options.len(),
        5,
        "xj Component metadata must have exactly 5 uri_options"
    );

    let param_option = meta
        .uri_options
        .iter()
        .find(|o| o.name == "param")
        .expect("param option must exist in xj Component metadata");

    assert_eq!(
        param_option.pattern,
        Some(UriOptionMatch::Prefix {
            separator: "param.".to_string()
        }),
        "param option must have Prefix pattern with separator 'param.'"
    );
}
