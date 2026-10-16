//! Production-catalog smoke test for `camel lint` (Task 3.3 of
//! `openspec/changes/add-camel-lint`).
//!
//! Builds the real component catalog the way `camel lint` does and proves the
//! `R-URI-known` rule consults it: an unknown timer option (`bogusOption`)
//! yields an `R-URI-known:unknown-option` Error. If the catalog were empty, or
//! `R-URI-known` skipped it, the diagnostic would not surface as
//! `UnknownOption` (an absent scheme only produces an `UnverifiedScheme` info
//! note).

use camel_cli::commands::lint::production_engine;
use camel_lint::{DiagnosticCode, UriKnownSubCode};

#[tokio::test]
async fn production_catalog_reports_invalid_timer_option() {
    let engine = production_engine()
        .await
        .expect("production engine builds for catalog smoke test");

    // `timer` is a registered scheme; `bogusOption` is not among its options.
    // The option lives on a `to:` step — `endpoints()` exposes the `from` URI
    // with empty options, so R-URI-known only inspects query options on
    // `to`/`uri`/etc. endpoints.
    let source = "id: r1\nfrom: direct:start\nsteps:\n  - to: timer:tick?bogusOption=1\n";
    let diags = engine.lint(source);

    let unknown = diags
        .iter()
        .find(|d| d.code == DiagnosticCode::RUriKnown(UriKnownSubCode::UnknownOption))
        .unwrap_or_else(|| {
            panic!(
                "expected an R-URI-known:unknown-option Error on `bogusOption`; \
                 got: {diags:?}"
            )
        });

    // The diagnostic points at the offending option key.
    assert_eq!(&source[unknown.span.start..unknown.span.end], "bogusOption");
}

/// Regression (rc-twzg9): the `file` catalog entry declared `maxDepth` as a
/// non-`Option` field with no default, so the derive marked it required and
/// every `file:` URI emitted a false `missing-required-option`, even though
/// the runtime `from_uri` treats it as optional (`u64::MAX`). The attribute
/// now carries `default = "18446744073709551615"`; a `file:` URI without
/// `maxDepth` must lint clean.
#[tokio::test]
async fn production_catalog_file_without_max_depth_is_not_required() {
    let engine = production_engine()
        .await
        .expect("production engine builds for catalog smoke test");

    let source = "id: r1\nfrom: direct:start\nsteps:\n  - to: \"file:outbox?fileName=out.txt\"\n";
    let diags = engine.lint(source);

    // Guard against a vacuous pass: if the catalog did not know `file`, the
    // URI would surface as an UnverifiedScheme note instead of being validated.
    assert!(
        !diags.iter().any(|d| matches!(
            d.code,
            DiagnosticCode::RUriKnown(UriKnownSubCode::UnverifiedScheme)
        )),
        "file scheme must be present in the production catalog; got: {diags:?}"
    );

    let missing: Vec<_> = diags
        .iter()
        .filter(|d| d.code == DiagnosticCode::RUriKnown(UriKnownSubCode::MissingRequiredOption))
        .collect();
    assert!(
        missing.is_empty(),
        "`file:` without maxDepth must not report missing-required-option; got: {missing:?}"
    );
}
