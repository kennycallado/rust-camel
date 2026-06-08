//! Regression: per ADR-0012 Phase C, processor aggregator/log/wire_tap
//! sites MUST be downgraded to warn! with handler-owned annotation.

#[test]
fn processor_sites_use_warn_not_error() {
    let dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    for file in &["aggregator.rs", "log.rs", "wire_tap.rs"] {
        let src = std::fs::read_to_string(format!("{dir}/src/{file}"))
            .unwrap_or_else(|_| panic!("read {file}"));
        assert!(
            src.contains("// log-policy: handler-owned"),
            "{file} must have log-policy: handler-owned annotation"
        );
    }
}
