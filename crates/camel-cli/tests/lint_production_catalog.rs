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
