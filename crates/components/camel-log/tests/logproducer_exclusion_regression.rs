//! Regression: camel-log LogProducer error! at LogLevel::Error is a user-output
//! mechanism, NOT framework telemetry. Per oracle ruling ses_16262b201ffeCmO67e3T6qa73b,
//! it is EXCLUDED from ADR-0012 scope. This test asserts:
//! 1. The site has NO `// log-policy:` annotation
//! 2. The site is inside `impl Service<Exchange> for LogProducer`
//! 3. The exclusion is symbol-bound (NOT file-bound) — other error! in camel-log
//!    are still flagged

#[test]
fn logproducer_error_has_no_log_policy_annotation() {
    let dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let src = std::fs::read_to_string(format!("{dir}/src/lib.rs")).expect("read lib.rs");

    // Find the LogProducer impl block
    let impl_start = src
        .find("impl Service<Exchange> for LogProducer")
        .or_else(|| src.find("impl Service<Exchange<'_>> for LogProducer"))
        .expect("LogProducer impl block not found");

    // Find the error! inside LogProducer
    let after_impl = &src[impl_start..];
    assert!(
        after_impl.contains("error!"),
        "LogProducer must still have error! (not downgraded)"
    );

    // Verify NO log-policy annotation exists in the LogProducer impl body
    // (the annotation would be on the line before error!)
    let impl_end = after_impl.find('}').unwrap_or(after_impl.len());
    let impl_body = &after_impl[..impl_end];

    // Check that the error! arm does NOT have log-policy annotation
    let lines: Vec<&str> = impl_body.lines().collect();
    for (i, line) in lines.iter().enumerate() {
        if line.contains("error!") && line.contains("msg") {
            // This is the LogProducer error! site
            // The PRECEDING line must NOT have log-policy
            if i > 0 {
                let prev = lines[i - 1].trim();
                assert!(
                    !prev.starts_with("// log-policy:"),
                    "LogProducer error! must NOT have log-policy annotation (excluded from ADR-0012)"
                );
            }
        }
    }
}

#[test]
fn logproducer_exclusion_is_symbol_bound_not_file_bound() {
    // This test documents that the exclusion is symbol-bound.
    // If camel-log/src/lib.rs ever gets OTHER error! sites outside LogProducer,
    // they MUST be flagged by the lint (not excluded).
    // The lint implementation in xtask must check symbol context, not just file path.

    let dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let src = std::fs::read_to_string(format!("{dir}/src/lib.rs")).expect("read lib.rs");

    // Count log-policy annotations in the file — should be 0 (camel-log is excluded)
    let annotation_count = src.matches("// log-policy:").count();
    assert_eq!(
        annotation_count, 0,
        "camel-log must have ZERO log-policy annotations (LogProducer is excluded, not annotated)"
    );
}
