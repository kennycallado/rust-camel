//! Cross-crate corpus test pinning the two path-template parsers together
//! (restpath decision 4).
//!
//! `camel-http` does not depend on `camel-dsl`, so the segment grammar is
//! implemented twice: once for HTTP dispatch in
//! `camel_component_http::rest_match` and once for route loading in
//! `camel_dsl::rest`. Agreement is pinned here rather than by code sharing:
//! both parsers must classify every corpus template identically, and the
//! classification must match the hardcoded expectations below. A divergence
//! is a defect in whichever side moved.

use camel_component_http::rest_match as http_rest;
use camel_dsl::rest as dsl_rest;

/// Normalized segment representation shared by both parser adapters.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Part {
    Lit(String),
    Param(String),
}

/// Normalized parse-failure kind shared by both parser adapters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ErrorKind {
    Unclosed,
    StrayClose,
    EmptyName,
}

/// Templates both parsers MUST accept, with the classification asserted
/// against the hardcoded expectations in [`expected_parts`].
const VALID: [&str; 11] = [
    "/users/{id}",
    "/files/{id}.xml",
    "/report-{date}.json",
    "/{a}-{b}",
    "/{a}{b}",
    "/a{y}bc",
    "/users/{user-id}",
    "/f/{x.y}.dat",
    "/files/%7Bid%7D",
    "/x/{id}",
    "/a/b/c",
];

/// Templates both parsers MUST reject, with the error kind asserted against
/// [`assert_expected_error_kind`].
const INVALID: [&str; 5] = [
    "/files/{id.xml",
    "/files/id}.xml",
    "/files/{}.xml",
    "/{{id}}",
    "/x/{a{b}",
];

/// Adapt the HTTP dispatch parser into the normalized representation.
fn classify_http(template: &str) -> Result<Vec<Vec<Part>>, ErrorKind> {
    http_rest::parse_path_template(template)
        .map(|segments| {
            segments
                .into_iter()
                .map(|segment| {
                    segment
                        .parts
                        .into_iter()
                        .map(|part| match part {
                            http_rest::SegmentPart::Lit(text) => Part::Lit(text),
                            http_rest::SegmentPart::Param(name) => Part::Param(name),
                        })
                        .collect()
                })
                .collect()
        })
        .map_err(|err| match err {
            http_rest::TemplateParseError::UnclosedPlaceholder { .. } => ErrorKind::Unclosed,
            http_rest::TemplateParseError::StrayCloseBrace { .. } => ErrorKind::StrayClose,
            http_rest::TemplateParseError::EmptyPlaceholder { .. } => ErrorKind::EmptyName,
        })
}

/// Adapt the DSL parser into the normalized representation.
fn classify_dsl(template: &str) -> Result<Vec<Vec<Part>>, ErrorKind> {
    dsl_rest::parse_path_template(template)
        .map(|segments| {
            segments
                .into_iter()
                .map(|segment| {
                    segment
                        .parts
                        .into_iter()
                        .map(|part| match part {
                            dsl_rest::SegmentPart::Lit(text) => Part::Lit(text),
                            dsl_rest::SegmentPart::Param(name) => Part::Param(name),
                        })
                        .collect()
                })
                .collect()
        })
        .map_err(|err| match err {
            dsl_rest::TemplateParseError::UnclosedPlaceholder { .. } => ErrorKind::Unclosed,
            dsl_rest::TemplateParseError::StrayCloseBrace { .. } => ErrorKind::StrayClose,
            dsl_rest::TemplateParseError::EmptyPlaceholder { .. } => ErrorKind::EmptyName,
        })
}

/// A literal piece of a template segment.
fn lit(text: &str) -> Part {
    Part::Lit(text.to_string())
}

/// A `{name}` placeholder piece of a template segment.
fn param(name: &str) -> Part {
    Part::Param(name.to_string())
}

/// The blessed classification of every valid corpus template, written out
/// segment by segment from the grammar rather than derived from either
/// implementation.
fn expected_parts(template: &str) -> Vec<Vec<Part>> {
    match template {
        "/users/{id}" => vec![vec![lit("users")], vec![param("id")]],
        "/files/{id}.xml" => vec![vec![lit("files")], vec![param("id"), lit(".xml")]],
        "/report-{date}.json" => vec![vec![lit("report-"), param("date"), lit(".json")]],
        "/{a}-{b}" => vec![vec![param("a"), lit("-"), param("b")]],
        "/{a}{b}" => vec![vec![param("a"), param("b")]],
        "/a{y}bc" => vec![vec![lit("a"), param("y"), lit("bc")]],
        "/users/{user-id}" => vec![vec![lit("users")], vec![param("user-id")]],
        "/f/{x.y}.dat" => vec![vec![lit("f")], vec![param("x.y"), lit(".dat")]],
        "/files/%7Bid%7D" => vec![vec![lit("files")], vec![lit("%7Bid%7D")]],
        "/x/{id}" => vec![vec![lit("x")], vec![param("id")]],
        "/a/b/c" => vec![vec![lit("a")], vec![lit("b")], vec![lit("c")]],
        other => panic!("no hardcoded expectation for valid template '{other}'"),
    }
}

/// Assert the blessed error kind for an invalid corpus template.
///
/// Nested braces (`{{id}}`, `a{b`) have no single pinned malformation: the
/// grammar only requires a failure, so either kind is accepted as long as
/// both parsers agree on it.
fn assert_expected_error_kind(template: &str, kind: ErrorKind) {
    match template {
        "/files/{id.xml" => assert_eq!(
            kind,
            ErrorKind::Unclosed,
            "wrong error kind for '{template}'"
        ),
        "/files/id}.xml" => assert_eq!(
            kind,
            ErrorKind::StrayClose,
            "wrong error kind for '{template}'"
        ),
        "/files/{}.xml" => assert_eq!(
            kind,
            ErrorKind::EmptyName,
            "wrong error kind for '{template}'"
        ),
        "/{{id}}" | "/x/{a{b}" => assert!(
            matches!(kind, ErrorKind::Unclosed | ErrorKind::StrayClose),
            "unexpected error kind {kind:?} for '{template}'"
        ),
        other => panic!("no hardcoded error expectation for invalid template '{other}'"),
    }
}

#[test]
fn both_parsers_classify_valid_corpus_identically() {
    assert_eq!(VALID.len(), 11, "valid corpus must cover all 11 templates");

    for template in VALID {
        let dsl = classify_dsl(template);
        let http = classify_http(template);

        assert_eq!(dsl, http, "parsers disagree on '{template}'");
        assert_eq!(
            dsl,
            Ok(expected_parts(template)),
            "unexpected classification for '{template}'"
        );
    }
}

#[test]
fn both_parsers_reject_invalid_corpus_identically() {
    assert_eq!(
        INVALID.len(),
        5,
        "invalid corpus must cover all 5 templates"
    );

    for template in INVALID {
        let dsl = match classify_dsl(template) {
            Ok(parts) => panic!("DSL parser accepted invalid template '{template}' as {parts:?}"),
            Err(kind) => kind,
        };
        let http = match classify_http(template) {
            Ok(parts) => {
                panic!("HTTP parser accepted invalid template '{template}' as {parts:?}")
            }
            Err(kind) => kind,
        };

        assert_eq!(dsl, http, "parsers disagree on error kind for '{template}'");
        assert_expected_error_kind(template, dsl);
    }
}
