// Integration tests for the `descriptor` attribute on `#[uri_config(..)]`.
// These live in `camel-endpoint/tests/` to avoid a publish-cycle between
// `camel-endpoint-macros` and `camel-endpoint`.

use camel_endpoint::{UriConfig, UriOption};

fn find<'a>(options: &'a [UriOption], name: &str) -> &'a UriOption {
    options
        .iter()
        .find(|o| o.name == name)
        .unwrap_or_else(|| panic!("uri_option not found: {name}"))
}

#[derive(UriConfig)]
#[uri_scheme = "x"]
#[uri_config(skip_impl, descriptor, metadata(scheme = "x"))]
struct DescriptorBareNonOption {
    #[uri_param(name = "foo")]
    pub _foo: String,
}

#[test]
fn descriptor_with_bare_non_option_field_is_not_required() {
    let options = DescriptorBareNonOption::uri_options();
    assert!(!find(&options, "foo").required);
}

#[derive(UriConfig)]
#[uri_scheme = "x"]
#[uri_config(skip_impl, descriptor, metadata(scheme = "x"))]
struct DescriptorExplicitRequired {
    #[uri_param(name = "foo", required)]
    pub _foo: String,
}

#[test]
fn descriptor_with_explicit_required_stays_required() {
    let options = DescriptorExplicitRequired::uri_options();
    assert!(find(&options, "foo").required);
}

#[derive(UriConfig)]
#[uri_scheme = "x"]
#[uri_config(skip_impl, metadata(scheme = "x"))]
#[allow(dead_code)]
struct RuntimeConfigNoDescriptor {
    #[uri_param(name = "foo")]
    pub sample: String,
}

#[test]
fn runtime_config_without_descriptor_retains_shape_inference() {
    let options = RuntimeConfigNoDescriptor::uri_options();
    assert!(find(&options, "foo").required);
}

#[derive(UriConfig)]
#[uri_scheme = "x"]
#[uri_config(skip_impl, descriptor, metadata(scheme = "x"))]
struct DescriptorPattern {
    #[uri_param(pattern = "param.")]
    pub _params: Vec<(String, String)>,
}

#[test]
fn descriptor_with_pattern_field_is_not_required() {
    let options = DescriptorPattern::uri_options();
    assert!(!find(&options, "param").required);
}

#[derive(UriConfig)]
#[uri_scheme = "x"]
#[uri_config(skip_impl, descriptor, metadata(scheme = "x"))]
struct DescriptorDefault {
    #[uri_param(name = "period", default = "1000")]
    pub _period: u64,
}

#[test]
fn descriptor_with_default_field_is_not_required() {
    let options = DescriptorDefault::uri_options();
    assert!(!find(&options, "period").required);
}

#[derive(UriConfig)]
#[uri_scheme = "x"]
#[uri_config(skip_impl, descriptor, metadata(scheme = "x"))]
struct DescriptorOption {
    #[uri_param(name = "password")]
    pub _password: Option<String>,
}

#[test]
fn descriptor_with_option_field_is_not_required() {
    let options = DescriptorOption::uri_options();
    assert!(!find(&options, "password").required);
}
