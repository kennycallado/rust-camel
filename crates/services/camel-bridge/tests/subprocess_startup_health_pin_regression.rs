//! Regression: per ADR-0012 Phase C, bridge subprocess startup failures
//! are (g). They MUST keep error! with system-broken annotation.

#[test]
fn bridge_startup_uses_system_broken() {
    let dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let src = std::fs::read_to_string(format!("{dir}/src/process.rs")).expect("read");
    assert!(
        src.contains("// log-policy: system-broken"),
        "bridge process.rs must have system-broken annotation"
    );
}
