//! trybuild compile-fail tests for the `AuthenticatedPrincipal` seal.
//!
//! Run with `cargo test -p camel-auth --test seal_compile_fail`.
//! The expected `.stderr` snapshots live alongside each `*.rs` case in
//! `tests/seal/`.

#[test]
fn seal() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/seal/*.rs");
}
