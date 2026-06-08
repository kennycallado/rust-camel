//! Regression: per ADR-0012 + Phase C, jms send_and_wait with b-bridged
//! discriminator MUST use metric + outside-contract annotation.

#[test]
fn jms_send_and_wait_uses_metric_and_outside_contract() {
    let dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let src = std::fs::read_to_string(format!("{dir}/src/consumer.rs")).expect("read consumer.rs");
    assert!(
        src.contains("increment_errors") && src.contains("b-prime:jms"),
        "jms consumer.rs must use increment_errors with b-prime:jms metric label"
    );
    assert!(
        src.contains("// log-policy: outside-contract"),
        "jms consumer.rs must have log-policy: outside-contract annotation"
    );
}
