//! Shared HTTP transport for the decision adapters (feature `decisions`).
//!
//! Owns URL joining, the lazily built hardened client (SSRF validation and
//! DNS pinning), bounded response reads, and status-to-`LlmError` mapping.

use std::fmt;
use std::net::SocketAddr;
use std::time::Duration;

use bytes::Bytes;
use bytes::BytesMut;
use camel_api::SsrfPolicy;
use camel_api::redact::redact_url;
use tokio::sync::OnceCell;
use url::Host;
use url::Url;

use crate::LlmHttpPolicy;
use crate::error::LlmError;
use crate::provider::http_errors;
use crate::provider_factory;

/// Maximum accepted decision response body (8 MiB).
pub(crate) const MAX_DECISION_RESPONSE_BYTES: usize = 8 * 1024 * 1024;

/// Join a base URL and an endpoint path, trimming trailing `/` from the base.
pub(crate) fn join_url(base: &str, path: &str) -> String {
    format!("{}{}", base.trim_end_matches('/'), path)
}

/// Where a decision base URL came from. It decides whether the URL is
/// SSRF-validated and DNS-pinned before the first request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BaseUrlSource {
    /// Set by the operator in configuration: validated and pinned.
    Configured,
    /// Built-in provider endpoint: trusted and not validated.
    Default,
}

/// Shared HTTP transport for one decision provider instance.
pub(crate) struct DecisionHttp {
    url: String,
    base_url: String,
    base_url_source: BaseUrlSource,
    api_key: Option<String>,
    policy: SsrfPolicy,
    timeout: Duration,
    client: OnceCell<reqwest::Client>,
}

impl fmt::Debug for DecisionHttp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DecisionHttp")
            .field("url", &redact_url(&self.url))
            .field("base_url", &redact_url(&self.base_url))
            .field("base_url_source", &self.base_url_source)
            .field("api_key", &self.api_key.as_ref().map(|_| "[REDACTED]"))
            .field("policy", &self.policy)
            .field("timeout", &self.timeout)
            .finish_non_exhaustive()
    }
}

impl DecisionHttp {
    /// Build a transport for `path` under `base_url`.
    pub(crate) fn new(
        base_url: String,
        base_url_source: BaseUrlSource,
        path: &str,
        api_key: Option<String>,
        policy: SsrfPolicy,
        timeout: Duration,
    ) -> Self {
        let url = join_url(&base_url, path);
        Self {
            url,
            base_url,
            base_url_source,
            api_key,
            policy,
            timeout,
            client: OnceCell::new(),
        }
    }

    /// POST a JSON body and return the raw 2xx response bytes.
    pub(crate) async fn post_json(&self, body: Vec<u8>, model: &str) -> Result<Bytes, LlmError> {
        let client = self.client().await?;
        let mut request = client
            .post(&self.url)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .timeout(self.timeout)
            .body(body);
        if let Some(key) = &self.api_key {
            request = request.bearer_auth(key);
        }
        let mut resp = request.send().await.map_err(|e| {
            http_errors::map_reqwest_error(&e, LlmHttpPolicy::decision(), Some(self.timeout))
        })?;
        let status = resp.status().as_u16();
        let retry_after = http_errors::parse_retry_after(
            resp.headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|v| v.to_str().ok()),
        );
        let (bytes, truncated) = read_capped(&mut resp, self.timeout).await?;
        if (200..300).contains(&status) {
            if truncated {
                return Err(LlmError::Protocol("decision response exceeds 8 MiB".into()));
            }
            return Ok(bytes);
        }
        // An error body that is over the cap is cut short, not rejected: the
        // status still decides the error kind.
        let text = redact_api_key(&String::from_utf8_lossy(&bytes), self.api_key.as_deref());
        match status {
            413 | 415 => Err(LlmError::InvalidRequest(http_errors::truncate_body(&text))),
            // `map_status` needs the whole (8 MiB-capped) body: the quota check
            // parses it as JSON, and a real `insufficient_quota` body is longer
            // than 200 bytes. Every arm truncates what it puts in the error text.
            _ => Err(http_errors::map_status(
                status,
                &text,
                Some(model),
                retry_after,
            )),
        }
    }

    /// Return the lazily built hardened client, building it on first use.
    async fn client(&self) -> Result<&reqwest::Client, LlmError> {
        self.client.get_or_try_init(|| self.build_client()).await
    }

    /// Build the hardened client, pinning DNS when the base URL warrants it.
    async fn build_client(&self) -> Result<reqwest::Client, LlmError> {
        let pinning = self.pinning().await?;
        let pinned = pinning
            .as_ref()
            .map(|(host, addrs)| (host.as_str(), addrs.as_slice()));
        crate::hardened_http_client_with_policy(LlmHttpPolicy::decision(), pinned)
            .map_err(|e| LlmError::provider(e.to_string()))
    }

    /// SSRF-validate an explicit base URL off the runtime and collect pinned
    /// addresses. Only a domain host is pinned; default URLs are not validated.
    async fn pinning(&self) -> Result<Option<(String, Vec<SocketAddr>)>, LlmError> {
        if self.base_url_source == BaseUrlSource::Default {
            return Ok(None);
        }
        let base = self.base_url.clone();
        let policy = self.policy;
        let validated = tokio::task::spawn_blocking(move || {
            provider_factory::validate_llm_url_pinned(&base, policy)
        })
        .await
        .map_err(|e| {
            LlmError::InvalidRequest(format!(
                "decision base_url SSRF validation task failed: {e}"
            ))
        })?;
        let (host, addrs) = validated?;
        Ok(is_domain_host(&self.base_url).then_some((host, addrs)))
    }
}

/// Read a response body chunk by chunk, keeping at most
/// `MAX_DECISION_RESPONSE_BYTES`. The flag is true when the body was longer
/// than the cap; reading stops at the cap, so the rest is never buffered.
async fn read_capped(
    resp: &mut reqwest::Response,
    timeout: Duration,
) -> Result<(Bytes, bool), LlmError> {
    let mut buf = BytesMut::new();
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| http_errors::map_reqwest_error(&e, LlmHttpPolicy::decision(), Some(timeout)))?
    {
        let room = MAX_DECISION_RESPONSE_BYTES - buf.len();
        if chunk.len() > room {
            buf.extend_from_slice(&chunk[..room]);
            return Ok((buf.freeze(), true));
        }
        buf.extend_from_slice(&chunk);
    }
    Ok((buf.freeze(), false))
}

/// Replaces every occurrence of the configured API key in `text` with
/// `[REDACTED]`, so a provider that echoes the key in an error body cannot
/// leak it. A missing or empty key leaves `text` unchanged. Runs before
/// truncation, so a key split at the 200-byte boundary cannot leak a prefix.
/// Only verbatim echoes are matched: a JSON-escaped or masked echo (for
/// example `sk-****abcd`) is not the full key and is left as is.
fn redact_api_key(text: &str, api_key: Option<&str>) -> String {
    match api_key {
        Some(key) if !key.is_empty() => text.replace(key, "[REDACTED]"),
        _ => text.to_string(),
    }
}

/// True when `base_url` parses with a domain (not IP literal) host.
fn is_domain_host(base_url: &str) -> bool {
    match Url::parse(base_url) {
        Ok(parsed) => matches!(parsed.host(), Some(Host::Domain(_))),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn join_url_trims_trailing_slash() {
        assert_eq!(
            join_url("https://a/v1/", "/decisions"),
            "https://a/v1/decisions"
        );
    }

    #[tokio::test]
    async fn post_json_returns_body_on_200() {
        let server = MockServer::start().await;
        Mock::given(matchers::method("POST"))
            .and(matchers::path("/x"))
            .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"ok":1}"#))
            .mount(&server)
            .await;
        let http = DecisionHttp::new(
            server.uri(),
            BaseUrlSource::Configured,
            "/x",
            Some("k".into()),
            SsrfPolicy::AllowInternal,
            Duration::from_secs(5),
        );

        let bytes = http
            .post_json(b"{}".to_vec(), "m")
            .await
            .expect("200 must succeed");

        assert_eq!(&bytes[..], br#"{"ok":1}"#);
        let requests = server
            .received_requests()
            .await
            .expect("wiremock request journal available");
        assert_eq!(requests.len(), 1, "exactly one request expected");
        assert_eq!(
            requests[0]
                .headers
                .get("authorization")
                .map(|v| v.to_str().expect("ascii header")),
            Some("Bearer k")
        );
    }

    #[tokio::test]
    async fn no_auth_header_without_key() {
        let server = MockServer::start().await;
        Mock::given(matchers::method("POST"))
            .and(matchers::path("/x"))
            .respond_with(ResponseTemplate::new(200).set_body_string("{}"))
            .mount(&server)
            .await;
        let http = DecisionHttp::new(
            server.uri(),
            BaseUrlSource::Configured,
            "/x",
            None,
            SsrfPolicy::AllowInternal,
            Duration::from_secs(5),
        );

        http.post_json(b"{}".to_vec(), "m")
            .await
            .expect("200 must succeed");

        let requests = server
            .received_requests()
            .await
            .expect("wiremock request journal available");
        assert_eq!(requests.len(), 1, "exactly one request expected");
        assert!(
            requests[0].headers.get("authorization").is_none(),
            "no authorization header without an api key"
        );
    }

    #[tokio::test]
    async fn oversize_response_is_protocol() {
        let server = MockServer::start().await;
        Mock::given(matchers::method("POST"))
            .and(matchers::path("/x"))
            .respond_with(
                ResponseTemplate::new(200).set_body_bytes(vec![b'a'; 8 * 1024 * 1024 + 1]),
            )
            .mount(&server)
            .await;
        let http = DecisionHttp::new(
            server.uri(),
            BaseUrlSource::Configured,
            "/x",
            Some("k".into()),
            SsrfPolicy::AllowInternal,
            Duration::from_secs(5),
        );

        let err = http
            .post_json(b"{}".to_vec(), "m")
            .await
            .expect_err("oversize body must fail");

        assert!(matches!(err, LlmError::Protocol(_)), "got {err:?}");
    }

    #[tokio::test]
    async fn oversize_error_body_still_maps_status() {
        let server = MockServer::start().await;
        Mock::given(matchers::method("POST"))
            .and(matchers::path("/x"))
            .respond_with(
                ResponseTemplate::new(429).set_body_bytes(vec![b'a'; 8 * 1024 * 1024 + 1]),
            )
            .mount(&server)
            .await;
        let http = DecisionHttp::new(
            server.uri(),
            BaseUrlSource::Configured,
            "/x",
            Some("k".into()),
            SsrfPolicy::AllowInternal,
            Duration::from_secs(5),
        );

        let err = http
            .post_json(b"{}".to_vec(), "m")
            .await
            .expect_err("429 must fail");

        assert!(
            matches!(err, LlmError::RateLimit { .. }),
            "oversize error body must keep its status mapping: {err:?}"
        );
    }

    #[tokio::test]
    async fn echoed_api_key_is_redacted_in_error() {
        let server = MockServer::start().await;
        Mock::given(matchers::method("POST"))
            .and(matchers::path("/x"))
            .respond_with(ResponseTemplate::new(401).set_body_string(
                r#"{"error":{"message":"Incorrect API key provided: sk-secret-123"}}"#, // allow-secret: provider echoes the key
            ))
            .mount(&server)
            .await;
        let http = DecisionHttp::new(
            server.uri(),
            BaseUrlSource::Configured,
            "/x",
            Some("sk-secret-123".into()),
            SsrfPolicy::AllowInternal,
            Duration::from_secs(5),
        );

        let err = http
            .post_json(b"{}".to_vec(), "m")
            .await
            .expect_err("401 must fail");

        let text = err.to_string();
        assert!(
            !text.contains("sk-secret-123"),
            "error text leaks the key: {text}"
        ); // allow-secret: asserts redaction
        assert!(
            text.contains("[REDACTED]"),
            "echoed key must be replaced: {text}"
        );
    }

    #[test]
    fn debug_redacts_api_key() {
        let http = DecisionHttp::new(
            "https://api.example.com".into(),
            BaseUrlSource::Configured,
            "/x",
            Some("sk-secret-123".into()),
            SsrfPolicy::PublicHttpsOnly,
            Duration::from_secs(5),
        );

        assert!(
            !format!("{http:?}").contains("sk-secret-123"), // allow-secret: asserts Debug redaction
            "Debug output must not leak the api key"
        );
    }

    #[tokio::test]
    async fn status_413_and_415_are_invalid_request() {
        for status in [413u16, 415] {
            let server = MockServer::start().await;
            Mock::given(matchers::method("POST"))
                .and(matchers::path("/x"))
                .respond_with(ResponseTemplate::new(status).set_body_string("too big"))
                .mount(&server)
                .await;
            let http = DecisionHttp::new(
                server.uri(),
                BaseUrlSource::Configured,
                "/x",
                Some("k".into()),
                SsrfPolicy::AllowInternal,
                Duration::from_secs(5),
            );

            let err = http
                .post_json(b"{}".to_vec(), "m")
                .await
                .expect_err("413/415 must fail");

            assert!(
                matches!(err, LlmError::InvalidRequest(_)),
                "status {status}: got {err:?}"
            );
        }
    }

    #[tokio::test]
    async fn long_insufficient_quota_body_is_quota_exceeded() {
        let body = r#"{"error":{"message":"You exceeded your current quota, please check your plan and billing details. For more information on this error, read the docs: https://platform.openai.com/docs/guides/error-codes/api-errors.","type":"insufficient_quota","param":null,"code":"insufficient_quota"}}"#;
        assert!(
            body.len() > 200,
            "fixture must exceed the 200-byte error cap"
        );
        let server = MockServer::start().await;
        Mock::given(matchers::method("POST"))
            .and(matchers::path("/x"))
            .respond_with(ResponseTemplate::new(429).set_body_string(body))
            .mount(&server)
            .await;
        let http = DecisionHttp::new(
            server.uri(),
            BaseUrlSource::Configured,
            "/x",
            Some("k".into()),
            SsrfPolicy::AllowInternal,
            Duration::from_secs(5),
        );

        let err = http
            .post_json(b"{}".to_vec(), "m")
            .await
            .expect_err("429 must fail");

        assert!(
            matches!(err, LlmError::QuotaExceeded { .. }),
            "long quota body must not degrade to RateLimit: {err:?}"
        );
        assert!(
            err.to_string().len() < 300,
            "error text must stay bounded: {err}"
        );
    }
}
