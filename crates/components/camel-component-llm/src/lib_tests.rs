//! Unit tests for the crate root (hardened HTTP client policy).

use super::*;

#[test]
fn llm_http_policy_values() {
    assert_eq!(LlmHttpPolicy::chat().connect, Duration::from_secs(10));
    assert_eq!(
        LlmHttpPolicy::chat().read_idle,
        Some(Duration::from_secs(600))
    );
    assert_eq!(LlmHttpPolicy::decision().connect, Duration::from_secs(10));
    assert_eq!(LlmHttpPolicy::decision().read_idle, None);
}

#[test]
fn hardened_clients_build_without_total_timeout() {
    hardened_http_client_with_policy(LlmHttpPolicy::decision(), None)
        .expect("decision client must build without a read-idle guard");
    hardened_http_client().expect("chat client must build with its read-idle guard");
}
