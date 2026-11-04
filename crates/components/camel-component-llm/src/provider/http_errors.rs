//! Shared HTTP status and transport error mapping for LLM providers.

use std::time::Duration;

use crate::error::LlmError;

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

pub(crate) fn map_reqwest_error(err: &reqwest::Error, configured_timeout: Duration) -> LlmError {
    if err.is_timeout() {
        LlmError::Timeout(configured_timeout)
    } else {
        LlmError::Network(err.to_string())
    }
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
