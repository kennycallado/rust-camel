use std::{fs, path::PathBuf};

#[test]
fn guide_yaml_route_uses_the_real_parser() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/config-basic/routes/hello.yaml");
    let yaml = fs::read_to_string(path).expect("guide YAML example must be readable");

    let routes = camel_dsl::parse_yaml(&yaml).expect("guide YAML example must parse");

    assert_eq!(routes.len(), 1);
    assert_eq!(routes[0].route_id(), "hello-timer");
}
