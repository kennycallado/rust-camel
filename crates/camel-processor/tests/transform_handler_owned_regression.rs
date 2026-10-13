//! Regression: per ADR-0012 Phase C, processor `aggregator` / `wire_tap`
//! sites MUST be downgraded to `warn!` with a handler-owned annotation.
//!
//! The `log` step is deliberately NOT handler-owned: the route selects the
//! level, so `level: error` must emit an ERROR event — that emission is the
//! product's output, not an operational failure signal. `log.rs` is therefore
//! a structural exclusion bound to its impl blocks via `STRUCTURAL_EXCLUSIONS`
//! in `scripts/xtask/src/main.rs` (lint-log-levels). See ADR-0012,
//! "Structural exclusions (user-output mechanisms)".

#[test]
fn processor_sites_use_warn_not_error() {
    let dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    for file in &["aggregator.rs", "wire_tap.rs"] {
        let src = std::fs::read_to_string(format!("{dir}/src/{file}"))
            .unwrap_or_else(|_| panic!("read {file}"));
        assert!(
            src.contains("// log-policy: handler-owned"),
            "{file} must have log-policy: handler-owned annotation"
        );
    }
}

#[test]
fn log_error_exclusion_is_bound_in_xtask() {
    let dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let xtask_path = format!("{dir}/../../scripts/xtask/src/main.rs");
    let src = std::fs::read_to_string(&xtask_path).unwrap_or_else(|_| panic!("read {xtask_path}"));

    let start = src
        .find("const STRUCTURAL_EXCLUSIONS")
        .expect("STRUCTURAL_EXCLUSIONS const must exist in xtask main.rs");
    let end = src[start..]
        .find("\n];")
        .expect("STRUCTURAL_EXCLUSIONS const must end with `\\n];`");
    let block = &src[start..start + end];
    let compact: String = block.chars().filter(|c| !c.is_whitespace()).collect();

    for marker in [
        "impl Service<Exchange> for LogProcessor",
        "impl Service<Exchange> for DynamicLog",
    ] {
        // Both the block and the expected pair are whitespace-stripped, so the
        // comparison is layout-independent. The trailing tuple comma sits
        // between the marker and `)`, so the pair is compared without the
        // enclosing parens.
        let expected: String = format!("\"camel-processor/src/log.rs\",\"{marker}\"")
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        assert!(
            compact.contains(&expected),
            "STRUCTURAL_EXCLUSIONS must bind camel-processor/src/log.rs to \
             `{marker}`; without the binding, `level: error` on the log step \
             would be flagged by lint-log-levels"
        );
    }
}
