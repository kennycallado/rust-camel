//! Slim-profile gate: bridge schemes stay out of the lint catalog.
//!
//! With the per-bridge features off (`--no-default-features`), the lint
//! catalog must not register the gated bridge bundles, and the lint engine
//! must degrade gracefully: an endpoint on a gated scheme surfaces as a
//! single informational `unverified-scheme` note (the same by-design class
//! as `wasm`/`exec`), never an error. The file is compiled only when the
//! gates its assertions depend on are off (`jms`, `xslt`, `sql`) — the
//! default (full) build and any sql/xslt-enabled build have nothing to
//! assert here.

#![cfg(not(any(feature = "jms", feature = "xslt", feature = "sql")))]

use camel_cli::commands::lint::production_engine;
use camel_lint::{DiagnosticCode, Severity, UriKnownSubCode};

/// Route document whose endpoint sits on the gated `jms` bridge scheme.
const JMS_ROUTE: &str = "routes:
  - id: \"jms-bridge\"
    from: \"jms:queue\"
    steps:
      - log: \"message=hello\"
";

/// Count `unverified-scheme` diagnostics among `diags`.
fn unverified_scheme_count(diags: &[camel_lint::Diagnostic]) -> usize {
    diags
        .iter()
        .filter(|d| {
            matches!(
                d.code,
                DiagnosticCode::RUriKnown(UriKnownSubCode::UnverifiedScheme)
            )
        })
        .count()
}

#[tokio::test]
async fn slim_lint_catalog_omits_gated_bridges() {
    let mut ctx = camel_core::CamelContext::builder()
        .build()
        .await
        .unwrap_or_else(|e| panic!("fresh CamelContext builds for lint gate: {e}"));
    camel_cli::register_builtin_components_for_lint(&mut ctx);

    let registry = ctx.registry();
    assert!(
        registry.get("jms").is_none(),
        "jms bridge must be absent from the slim lint catalog"
    );
    assert!(
        registry.get("xslt").is_none(),
        "xslt bridge must be absent from the slim lint catalog"
    );
    assert!(
        registry.get("sql").is_none(),
        "sql bridge must be absent from the slim lint catalog"
    );
    assert!(
        registry.get("http").is_some(),
        "http must stay registered in every profile"
    );
    assert!(
        registry.get("timer").is_some(),
        "timer must stay registered in every profile"
    );
    assert!(
        registry.get("stream").is_some(),
        "stream must stay registered in every profile (ADR-0080 always-on)"
    );
}

#[tokio::test]
async fn slim_lint_flags_gated_bridge_scheme() {
    let engine = production_engine()
        .await
        .expect("production engine builds with gated bridges omitted");

    let diags = engine.lint(JMS_ROUTE);
    let count = unverified_scheme_count(&diags);
    assert_eq!(
        count, 1,
        "exactly one unverified-scheme note expected; got {diags:?}"
    );

    let note = diags
        .iter()
        .find(|d| {
            matches!(
                d.code,
                DiagnosticCode::RUriKnown(UriKnownSubCode::UnverifiedScheme)
            )
        })
        .expect("unverified-scheme diagnostic present");
    assert_eq!(
        note.severity,
        Severity::Info,
        "gated bridge degradation is informational, like wasm/exec"
    );
    // Naming check: the note must sit exactly on the `jms` endpoint's
    // scheme token. Since rc-bsx4t trims the YAML quote pair at
    // endpoint-span construction, the span slices the bare token.
    let uri_pos = JMS_ROUTE
        .find("\"jms:queue\"")
        .expect("fixture contains the jms endpoint URI");
    let scheme_start = uri_pos + "\"".len();
    let scheme_end = scheme_start + "jms".len();
    assert_eq!(
        (note.span.start, note.span.end),
        (scheme_start, scheme_end),
        "note must sit on the exact jms scheme token; span {:?}, scheme at {scheme_start}..{scheme_end}",
        note.span
    );
}
