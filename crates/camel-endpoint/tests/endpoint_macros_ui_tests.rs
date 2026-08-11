//! Origin: camel-endpoint-macros/tests/ui_tests.rs (relocated per ADR-0055)

// trybuild compile-fail tests. Run with `cargo test -p camel-endpoint`.
// The expected `.stderr` snapshots live alongside each `*.rs` case in `tests/ui/`.
#[test]
fn ui() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/ui/*_fail.rs");
}
