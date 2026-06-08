//! Regression: per ADR-0012 Phase C Fix 2, keycloak consumer MUST have
//! 3 sites with 3 DIFFERENT categories: (b'), (e), (c).

#[test]
fn keycloak_has_three_distinct_categories() {
    let dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let src = std::fs::read_to_string(format!("{dir}/src/keycloak_consumer.rs")).expect("read");
    assert!(
        src.contains("// log-policy: outside-contract"),
        "must have outside-contract (b' or e)"
    );
    assert!(
        src.contains("// log-policy: system-broken"),
        "must have system-broken (c)"
    );
    assert!(
        src.contains("b-prime:keycloak:"),
        "must have b-prime metric"
    );
    assert!(src.contains("e:keycloak:"), "must have e metric");
}
