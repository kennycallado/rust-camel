//! Regression: per ADR-0012 Phase C, ws accept-loop errors MUST emit
//! metrics AND keep error!.

#[test]
fn ws_accept_loop_uses_metric_and_error() {
    let dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let src = std::fs::read_to_string(format!("{dir}/src/lib.rs")).expect("read lib.rs");
    assert!(
        src.contains("increment_errors") && src.contains("e:ws:"),
        "ws lib.rs must use increment_errors with e:ws: metric label"
    );
}
