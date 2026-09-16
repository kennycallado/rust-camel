pub mod bench_instrument;
pub mod compile;
pub mod errors;
pub mod job;
pub mod journal;
pub mod lint;
#[cfg(feature = "lsp")]
pub mod lsp;
pub mod new;
pub mod openapi;
pub mod plugin;
pub mod run;
pub mod test;

// Shared test helpers: `security` covers the run_tests.rs callers,
// `integration-http` the scenario tests; outside that union the helper
// would be dead code.
#[cfg(all(test, any(feature = "security", feature = "integration-http")))]
pub(crate) mod test_support;
