// trybuild compile-fail tests. Run with `cargo test -p camel-endpoint-macros`.
// The expected `.stderr` snapshots live alongside each `*.rs` case in `tests/ui/`.
#[test]
fn ui() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/ui/*_fail.rs");
}
