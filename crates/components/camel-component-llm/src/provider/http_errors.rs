//! Shared HTTP status and transport error mapping for LLM providers.

use std::time::Duration;

use crate::error::{LlmError, TransportTimeoutPhase};

/// Maximum length for body-derived error details.
const MAX_BODY_ERROR_BYTES: usize = 200;

/// Truncate a response body for display, keeping it within
/// `MAX_BODY_ERROR_BYTES` at a UTF-8 boundary.
pub(crate) fn truncate_body(body: &str) -> String {
    if body.len() <= MAX_BODY_ERROR_BYTES {
        body.to_string()
    } else {
        let cut = body.floor_char_boundary(MAX_BODY_ERROR_BYTES);
        format!("{}...[truncated]", &body[..cut])
    }
}

/// Map a reqwest transport error onto the Camel taxonomy.
///
/// The reported duration must be the bound the transport actually enforced:
/// the transport must never report a configured duration it did not enforce
/// (rc-9pfby).
///
/// - connect timeout → [`LlmError::TransportTimeout`] with the connect bound.
/// - any other timeout with a per-request total → [`LlmError::Timeout`] with
///   that total (the decision path).
/// - any other timeout without a per-request total → [`LlmError::TransportTimeout`]
///   with the read-idle guard (the chat path).
/// - everything else → [`LlmError::Network`].
pub(crate) fn map_reqwest_error(
    err: &reqwest::Error,
    policy: crate::LlmHttpPolicy,
    request_total: Option<Duration>,
) -> LlmError {
    if err.is_timeout() && err.is_connect() {
        return LlmError::TransportTimeout {
            phase: TransportTimeoutPhase::Connect,
            after: policy.connect,
        };
    }
    if err.is_timeout() {
        if let Some(total) = request_total {
            return LlmError::Timeout(total);
        }
        debug_assert!(
            policy.read_idle.is_some() || request_total.is_some(),
            "no enforced idle guard and no request total: a reported timeout would be a lie"
        );
        let Some(after) = policy.read_idle else {
            // Unreachable in practice: decision callers pass `request_total`
            // (handled above) and chat callers set `read_idle`; the debug_assert
            // covers strays in debug/test builds. Release builds surface an
            // honest provider error instead of a fabricated duration (rc-9pfby).
            return LlmError::provider("transport timeout without an enforced bound");
        };
        return LlmError::TransportTimeout {
            phase: TransportTimeoutPhase::Idle,
            after,
        };
    }
    LlmError::Network(err.to_string())
}

pub(crate) fn map_status(
    status: u16,
    body: &str,
    model: Option<&str>,
    retry_after: Option<Duration>,
) -> LlmError {
    match status {
        401 | 403 => LlmError::AuthFailed {
            detail: truncate_body(body),
        },
        404 => LlmError::ModelNotFound(
            model
                .map(str::to_string)
                .unwrap_or_else(|| truncate_body(body)),
        ),
        400 | 422 => LlmError::InvalidRequest(truncate_body(body)),
        429 if is_insufficient_quota(status, body) => LlmError::QuotaExceeded {
            detail: truncate_body(body),
        },
        429 => LlmError::RateLimit { retry_after },
        s if (500..=599).contains(&s) => {
            LlmError::ProviderUnavailable(truncate_body(&format!("HTTP {s}: {body}")))
        }
        _ => LlmError::provider(format!("HTTP {status}: {body}")),
    }
}

/// Detect an OpenAI-style billing quota exhaustion response.
pub(crate) fn is_insufficient_quota(status: u16, body: &str) -> bool {
    if status != 429 {
        return false;
    }
    let Ok(value) = serde_json::from_str::<serde_json::Value>(body) else {
        return false;
    };
    let Some(error) = value.get("error") else {
        return false;
    };

    error.get("code").and_then(|v| v.as_str()) == Some("insufficient_quota")
        || error.get("type").and_then(|v| v.as_str()) == Some("insufficient_quota")
}

/// Parse a delta-seconds `Retry-After` header; HTTP-date forms are rejected.
pub(crate) fn parse_retry_after(value: Option<&str>) -> Option<Duration> {
    let raw = value?.trim();
    if raw.is_empty() || !raw.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    raw.parse::<u64>().ok().map(Duration::from_secs)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A refused connection: `is_connect()` true, `is_timeout()` false.
    async fn connect_error() -> reqwest::Error {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        drop(listener);

        reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_millis(500))
            .build()
            .expect("client")
            .get(format!("http://{addr}/"))
            .send()
            .await
            .expect_err("connect must fail")
    }

    /// A connect timeout against the TEST-NET-1 blackhole, or `None` when the
    /// sandbox answers the blackhole with an immediate unreachable/refused
    /// error instead of a timeout.
    async fn connect_timeout_error() -> Option<reqwest::Error> {
        let connect = reqwest::Client::builder()
            .no_proxy()
            .connect_timeout(Duration::from_millis(1))
            .build()
            .expect("client")
            .get("http://192.0.2.1/")
            .send()
            .await
            .expect_err("connect timeout must fail");
        if connect.is_connect() && connect.is_timeout() {
            Some(connect)
        } else {
            None
        }
    }

    /// A read timeout: the peer accepts and never responds.
    async fn timeout_error() -> reqwest::Error {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        let server = tokio::spawn(async move {
            let (_socket, _) = listener.accept().await.expect("accept");
            tokio::time::sleep(Duration::from_secs(5)).await;
        });

        let err = reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_millis(50))
            .build()
            .expect("client")
            .get(format!("http://{addr}/"))
            .send()
            .await
            .expect_err("timeout must fail");

        server.abort();
        err
    }

    #[tokio::test]
    async fn map_reqwest_error_routes_by_layer() {
        // Connection refused → Network (unchanged arm).
        let refused = connect_error().await;
        assert!(refused.is_connect(), "fixture must be a connect error");
        assert!(
            !refused.is_timeout(),
            "a refused connect must not be a timeout"
        );
        assert!(
            matches!(
                map_reqwest_error(&refused, crate::LlmHttpPolicy::chat(), None),
                LlmError::Network(_)
            ),
            "refused connect must map to Network"
        );

        // Connect timeout → TransportTimeout::Connect with the 10s bound.
        match connect_timeout_error().await {
            Some(connect) => {
                assert!(
                    matches!(
                        map_reqwest_error(&connect, crate::LlmHttpPolicy::chat(), None),
                        LlmError::TransportTimeout {
                            phase: TransportTimeoutPhase::Connect,
                            after,
                        } if after == Duration::from_secs(10)
                    ),
                    "connect timeout must carry the connect bound"
                );
            }
            None => {
                // Environment-dependent: some sandboxes answer the blackhole
                // with an immediate unreachable/refused error instead of a
                // timeout. Keep the refused→Network assertion above; do not
                // fake the branch.
                eprintln!("connect-timeout-not-constructible-in-env");
            }
        }

        let idle = timeout_error().await;
        assert!(idle.is_timeout(), "fixture must be a timeout");
        assert!(!idle.is_connect(), "fixture must not be a connect error");

        // Timeout with a per-request total (decision path) → Timeout(total).
        assert!(
            matches!(
                map_reqwest_error(
                    &idle,
                    crate::LlmHttpPolicy::chat(),
                    Some(Duration::from_secs(90))
                ),
                LlmError::Timeout(total) if total == Duration::from_secs(90)
            ),
            "a per-request total must map to Timeout(total)"
        );

        // Timeout without a per-request total (chat path) → Idle guard.
        assert!(
            matches!(
                map_reqwest_error(&idle, crate::LlmHttpPolicy::chat(), None),
                LlmError::TransportTimeout {
                    phase: TransportTimeoutPhase::Idle,
                    after,
                } if after == Duration::from_secs(600)
            ),
            "chat transport timeout must carry the 600s read-idle guard"
        );
    }

    /// Decision-path mapping rows: connect timeout reports the 10s connect
    /// bound (one-row llm-decisions delta), and the per-request total keeps
    /// `Timeout(configured)` (unchanged row).
    #[tokio::test]
    async fn decision_mapping_rows() {
        let configured = Duration::from_secs(90);

        // Connect timeout → TransportTimeout::Connect { after: 10s }.
        match connect_timeout_error().await {
            Some(connect) => {
                let mapped =
                    map_reqwest_error(&connect, crate::LlmHttpPolicy::decision(), Some(configured));
                assert!(
                    matches!(
                        mapped,
                        LlmError::TransportTimeout {
                            phase: TransportTimeoutPhase::Connect,
                            after,
                        } if after == Duration::from_secs(10)
                    ),
                    "decision connect timeout must carry the 10s connect bound, got {mapped:?}"
                );
                let display = mapped.to_string();
                assert!(
                    display.contains("timeout"),
                    "connect display must mention timeout: {display}"
                );
                assert!(
                    display.contains("10s"),
                    "connect display must report the 10s bound, not the configured \
                     per-request total: {display}"
                );
            }
            None => {
                // Environment-dependent: some sandboxes answer the blackhole
                // with an immediate unreachable/refused error instead of a
                // timeout. Do not fake the branch.
                eprintln!("connect-timeout-not-constructible-in-env");
            }
        }

        // Total deadline (decode/total timeout) → Timeout(configured), unchanged.
        let idle = timeout_error().await;
        assert!(idle.is_timeout(), "fixture must be a timeout");
        assert!(!idle.is_connect(), "fixture must not be a connect error");
        let mapped = map_reqwest_error(&idle, crate::LlmHttpPolicy::decision(), Some(configured));
        assert!(
            matches!(mapped, LlmError::Timeout(total) if total == configured),
            "decision total timeout must map to Timeout(configured), got {mapped:?}"
        );
        let display = mapped.to_string();
        assert!(
            display.contains("timeout"),
            "total display must mention timeout: {display}"
        );
        assert!(
            display.contains("90s"),
            "total display must report the configured 90s total: {display}"
        );
    }
}
