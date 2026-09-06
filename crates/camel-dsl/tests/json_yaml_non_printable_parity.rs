//! Production front-end contract for non-printable (DEL U+007F) documents.
//!
//! Pins the spec scenario for `dsl-parity-json-valid`: the JSON front-end
//! accepts a JSON-valid document whose raw text contains a DEL byte, while
//! the YAML front-end rejects it with a format-annotated error. The escaped
//! form (`\u007f` as six ASCII bytes) must stay strict-parity across both
//! front-ends.

use camel_api::CamelError;
use camel_dsl::{json::parse_json_to_declarative, yaml::parse_yaml_to_declarative};

const MINIMIZED_DEL_DOC: &str = "{\"routes\":[{\"id\":\"r1\",\"from\":\"dtart\",\"steps\":[{\"to\":\"di*rect:ewwwwwwwwwwwwww\x7fwwwwwwwwnd\"}]}]}";

const ESCAPED_DEL_DOC: &str = "{\"routes\":[{\"id\":\"r1\",\"from\":\"dtart\",\"steps\":[{\"to\":\"di*rect:ewwwwwwwwwwwwww\\u007fwwwwwwwwnd\"}]}]}";

/// Extract the first route's first step `to` URI, or `None` if the first
/// step is not a `To` step.
fn first_to_uri(routes: &[camel_dsl::DeclarativeRoute]) -> Option<String> {
    let first_route = routes.first()?;
    let first_step = first_route.steps.first()?;
    match first_step {
        camel_dsl::DeclarativeStep::To(to) => Some(to.uri.clone()),
        _ => None,
    }
}

#[test]
fn json_front_end_accepts_raw_del_document() {
    let routes = parse_json_to_declarative(MINIMIZED_DEL_DOC)
        .expect("JSON front-end must accept a JSON-valid document with a raw DEL byte");
    let uri =
        first_to_uri(&routes).expect("first route's first step must be a To step carrying `to`");
    assert!(
        uri.contains('\u{7f}'),
        "first step `to` must contain U+007F, got: {uri:?}"
    );
}

#[test]
fn yaml_front_end_rejects_raw_del_with_format_annotation() {
    let err = parse_yaml_to_declarative(MINIMIZED_DEL_DOC)
        .expect_err("YAML front-end must reject a document with a raw DEL byte");
    let msg = match err {
        CamelError::RouteError(msg) => msg,
        other => panic!("expected CamelError::RouteError, got: {other:?}"),
    };
    assert!(
        msg.starts_with("YAML DSL error:"),
        "YAML error must carry the YAML format annotation, got: {msg}"
    );
}

#[test]
fn escaped_del_document_parity_both_front_ends() {
    let json_routes = parse_json_to_declarative(ESCAPED_DEL_DOC)
        .expect("JSON front-end must accept the escaped-DEL document");
    let yaml_routes = parse_yaml_to_declarative(ESCAPED_DEL_DOC)
        .expect("YAML front-end must accept the escaped-DEL document");

    let json_uri = first_to_uri(&json_routes)
        .expect("JSON first route's first step must be a To step carrying `to`");
    let yaml_uri = first_to_uri(&yaml_routes)
        .expect("YAML first route's first step must be a To step carrying `to`");

    assert_eq!(
        json_uri, yaml_uri,
        "both front-ends must produce identical first-step `to`"
    );
    assert!(
        json_uri.contains('\u{7f}'),
        "escaped DEL must decode to U+007F, got: {json_uri:?}"
    );
}
