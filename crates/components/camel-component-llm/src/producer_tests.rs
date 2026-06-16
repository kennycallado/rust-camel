// Thin dispatcher — each thematic test family lives in its own file.
// See ADR-0023 for the split rationale.

#[path = "producer_chat_tests.rs"]
mod producer_chat_tests;
#[path = "producer_retry_tests.rs"]
mod producer_retry_tests;
#[path = "producer_semaphore_tests.rs"]
mod producer_semaphore_tests;
#[path = "producer_test_helpers.rs"]
mod producer_test_helpers;
#[path = "producer_timeout_tests.rs"]
mod producer_timeout_tests;
#[path = "producer_tools_tests.rs"]
mod producer_tools_tests;
