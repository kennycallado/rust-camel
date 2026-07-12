use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, RwLock};

use camel_api::SsrfPolicy;

use crate::http_client::{SsrfClientOptions, build_ssrf_pinned_client};
use crate::types::AuthError;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Jwk {
    pub kid: String,
    pub kty: String,
    pub alg: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub r#use: Option<String>,
    pub n: String,
    pub e: String,
}

#[async_trait]
pub trait JwksProvider: Send + Sync {
    async fn get_signing_keys(&self) -> Result<Vec<Jwk>, AuthError>;
    async fn refresh(&self) -> Result<(), AuthError>;
}

struct CachedKeys {
    keys: Vec<Jwk>,
    fetched_at: Instant,
    ttl: Duration,
}

pub struct RemoteJwksProvider {
    jwks_uri: String,
    http: reqwest::Client,
    cache: RwLock<Option<CachedKeys>>,
    in_flight: Mutex<()>,
    default_ttl: Duration,
}

const MAX_JWKS_BODY_BYTES: u64 = 1024 * 1024; // 1 MiB
const MIN_JWKS_TTL_SECS: u64 = 60;
const MAX_JWKS_TTL_SECS: u64 = 3600;

impl RemoteJwksProvider {
    /// Creates a production provider with HTTPS enforcement, DNS-rebinding
    /// protection, SSRF guard, and hardened timeouts.
    ///
    /// Delegates to the shared [`build_ssrf_pinned_client`] helper which
    /// resolves the hostname at construction time and pins the validated
    /// IPs on the HTTP client, eliminating the TOCTOU window between DNS
    /// resolution and the first outbound request.
    pub async fn new(jwks_uri: String, policy: SsrfPolicy) -> Result<Self, AuthError> {
        let http = build_ssrf_pinned_client(
            &jwks_uri,
            "JWKS",
            &SsrfClientOptions::new(policy)
                .with_connect_timeout(Duration::from_secs(5))
                .with_request_timeout(Duration::from_secs(10)),
        )
        .await?;
        Ok(Self::with_client(jwks_uri, http))
    }

    /// Creates a provider with a custom HTTP client, bypassing URL validation.
    /// **For testing only.**
    #[cfg(test)]
    pub fn new_for_test(jwks_uri: String) -> Self {
        Self::with_client(jwks_uri, reqwest::Client::new())
    }

    fn with_client(jwks_uri: String, http: reqwest::Client) -> Self {
        Self {
            jwks_uri,
            http,
            cache: RwLock::new(None),
            in_flight: Mutex::new(()),
            default_ttl: Duration::from_secs(300),
        }
    }

    async fn fetch_and_store(&self) -> Result<Vec<Jwk>, AuthError> {
        let mut resp = self
            .http
            .get(&self.jwks_uri)
            .send()
            .await
            .map_err(|e| AuthError::ProviderUnavailable(format!("JWKS fetch failed: {e}")))?;

        if !resp.status().is_success() {
            return Err(AuthError::ProviderUnavailable(format!(
                "JWKS endpoint returned {}",
                resp.status()
            )));
        }

        // Parse Content-Length once; reuse for size guard and initial buffer capacity
        let content_length: Option<u64> = resp
            .headers()
            .get(reqwest::header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<u64>().ok());

        // Check Content-Length before buffering
        if let Some(cl) = content_length
            && cl > MAX_JWKS_BODY_BYTES
        {
            return Err(AuthError::ProviderUnavailable(format!(
                "JWKS body exceeds {MAX_JWKS_BODY_BYTES} bytes (Content-Length: {cl})"
            )));
        }

        // Bounded streaming read — abort once exceeding cap
        let initial_cap = content_length.unwrap_or(0) as usize;
        let mut body_bytes = Vec::with_capacity(initial_cap);
        while let Some(chunk) = resp
            .chunk()
            .await
            .map_err(|e| AuthError::ProviderUnavailable(format!("JWKS body read failed: {e}")))?
        {
            body_bytes.extend_from_slice(&chunk);
            if body_bytes.len() as u64 > MAX_JWKS_BODY_BYTES {
                return Err(AuthError::ProviderUnavailable(format!(
                    "JWKS body exceeds {} bytes (streaming)",
                    MAX_JWKS_BODY_BYTES
                )));
            }
        }

        // Extract and clamp max-age from Cache-Control
        let ttl_secs = resp
            .headers()
            .get("cache-control")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| {
                v.split(',').find_map(|part| {
                    let part = part.trim();
                    part.strip_prefix("max-age=")
                        .and_then(|s| s.parse::<u64>().ok())
                        .map(|s| s.clamp(MIN_JWKS_TTL_SECS, MAX_JWKS_TTL_SECS))
                })
            })
            .unwrap_or(self.default_ttl.as_secs());
        let ttl = Duration::from_secs(ttl_secs);

        #[derive(Deserialize)]
        struct JwksResponse {
            keys: Vec<Jwk>,
        }

        let body: JwksResponse = serde_json::from_slice(&body_bytes)
            .map_err(|e| AuthError::ProviderUnavailable(format!("JWKS parse failed: {e}")))?;

        let keys = body.keys;
        *self.cache.write().await = Some(CachedKeys {
            keys: keys.clone(),
            fetched_at: Instant::now(),
            ttl,
        });
        Ok(keys)
    }
}

#[async_trait]
impl JwksProvider for RemoteJwksProvider {
    async fn get_signing_keys(&self) -> Result<Vec<Jwk>, AuthError> {
        // Fast path: fresh cache
        {
            let cache = self.cache.read().await;
            if let Some(c) = cache.as_ref().filter(|c| c.fetched_at.elapsed() < c.ttl) {
                return Ok(c.keys.clone());
            }
        }

        // Slow path: single-flight fetch
        let _guard = self.in_flight.lock().await;

        // Re-check after acquiring lock (another task may have refreshed)
        {
            let cache = self.cache.read().await;
            if let Some(c) = cache.as_ref().filter(|c| c.fetched_at.elapsed() < c.ttl) {
                return Ok(c.keys.clone());
            }
        }

        self.fetch_and_store().await
    }

    async fn refresh(&self) -> Result<(), AuthError> {
        self.fetch_and_store().await.map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jwk_from_json_fields() {
        let jwk = Jwk {
            kid: "key-1".into(),
            kty: "RSA".into(),
            alg: Some("RS256".into()),
            r#use: None,
            n: "modulus-base64url".into(),
            e: "AQAB".into(),
        };
        assert_eq!(jwk.kid, "key-1");
        assert_eq!(jwk.e, "AQAB");
    }

    #[tokio::test]
    async fn https_enforcement_rejects_http() {
        let result = RemoteJwksProvider::new(
            "http://kc.example.com/realms/test/protocol/openid-connect/certs".into(),
            SsrfPolicy::PublicHttpsOnly,
        )
        .await;
        assert!(matches!(result, Err(AuthError::ConfigError(s)) if s.contains("HTTPS")));
    }

    #[tokio::test]
    async fn ssrf_guard_rejects_localhost() {
        let result = RemoteJwksProvider::new(
            "https://localhost/realms/test/protocol/openid-connect/certs".into(),
            SsrfPolicy::PublicHttpsOnly,
        )
        .await;
        assert!(matches!(result, Err(AuthError::ConfigError(s)) if s.contains("loopback")));
    }

    #[tokio::test]
    async fn ssrf_guard_rejects_private_ip() {
        let result = RemoteJwksProvider::new(
            "https://192.168.1.1/realms/test/protocol/openid-connect/certs".into(),
            SsrfPolicy::PublicHttpsOnly,
        )
        .await;
        assert!(matches!(result, Err(AuthError::ConfigError(s)) if s.contains("private")));
    }

    #[tokio::test]
    async fn production_url_accepted() {
        // 1.1.1.1 is a public IP — passes hostname validation, DNS resolution
        // (IP literal, no network needed), and IP validation.
        let result = RemoteJwksProvider::new(
            "https://1.1.1.1/realms/test/protocol/openid-connect/certs".into(),
            SsrfPolicy::PublicHttpsOnly,
        )
        .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn ssrf_guard_rejects_link_local_metadata_endpoint() {
        // 169.254.169.254 is the cloud instance-metadata address (AWS/GCP/Azure)
        let result = RemoteJwksProvider::new(
            "https://169.254.169.254/latest/meta-data".into(),
            SsrfPolicy::PublicHttpsOnly,
        )
        .await;
        assert!(
            matches!(result, Err(AuthError::ConfigError(s)) if s.contains("private") || s.contains("loopback"))
        );
    }

    #[tokio::test]
    async fn ssrf_guard_rejects_ipv6_unique_local() {
        let result = RemoteJwksProvider::new(
            "https://[fc00::1]/realms/test/protocol/openid-connect/certs".into(),
            SsrfPolicy::PublicHttpsOnly,
        )
        .await;
        assert!(
            matches!(result, Err(AuthError::ConfigError(s)) if s.contains("private") || s.contains("loopback"))
        );
    }

    #[tokio::test]
    async fn ssrf_guard_rejects_ipv6_loopback() {
        let result = RemoteJwksProvider::new(
            "https://[::1]/realms/test/protocol/openid-connect/certs".into(),
            SsrfPolicy::PublicHttpsOnly,
        )
        .await;
        assert!(
            matches!(result, Err(AuthError::ConfigError(s)) if s.contains("private") || s.contains("loopback"))
        );
    }

    #[tokio::test]
    async fn url_validator_rejects_loopback_ip_literal() {
        // 127.0.0.1 is a loopback IP literal — rejected at URL validation
        // stage before DNS pinning is attempted.
        let result = RemoteJwksProvider::new(
            "https://127.0.0.1/certs".into(),
            SsrfPolicy::PublicHttpsOnly,
        )
        .await;
        assert!(
            matches!(result, Err(AuthError::ConfigError(s)) if s.contains("loopback") || s.contains("private") || s.contains("SSRF"))
        );
    }

    #[tokio::test]
    async fn build_ssrf_pinned_client_rejects_localhost_dns() {
        // localhost would be caught by new()'s URL validation; we call the
        // helper directly to prove the DNS-resolution → SSRF-filter path
        // independently.
        let result = build_ssrf_pinned_client(
            "https://localhost/path",
            "test",
            &SsrfClientOptions::new(SsrfPolicy::PublicHttpsOnly)
                .with_connect_timeout(Duration::from_secs(5))
                .with_request_timeout(Duration::from_secs(10)),
        )
        .await;
        assert!(
            result.is_err(),
            "expected build_ssrf_pinned_client to reject localhost DNS, got Ok"
        );
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("SSRF") || err.contains("blocked") || err.contains("only to"),
            "expected SSRF-related error from DNS pin path, got: {err}"
        );
    }

    #[tokio::test]
    async fn dns_resolution_failure_returns_provider_unavailable() {
        // .invalid is an RFC 2606 reserved TLD guaranteed never to resolve.
        // Passes hostname validation but fails DNS resolution — exercises
        // the DNS-pinning code path.
        let result = RemoteJwksProvider::new(
            "https://nonexistent.invalid/certs".into(),
            SsrfPolicy::PublicHttpsOnly,
        )
        .await;
        assert!(matches!(result, Err(AuthError::ProviderUnavailable(s)) if s.contains("DNS")));
    }

    #[tokio::test]
    async fn cache_returns_fresh_keys_without_http() {
        let provider = RemoteJwksProvider::new_for_test("http://unreachable:9999/certs".into());
        // Seed cache manually
        {
            let mut cache = provider.cache.write().await;
            *cache = Some(CachedKeys {
                keys: vec![Jwk {
                    kid: "cached-key".into(),
                    kty: "RSA".into(),
                    alg: Some("RS256".into()),
                    r#use: None,
                    n: "n".into(),
                    e: "AQAB".into(),
                }],
                fetched_at: Instant::now(),
                ttl: Duration::from_secs(300),
            });
        }
        let keys = provider.get_signing_keys().await.unwrap();
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].kid, "cached-key");
    }

    /// Builds a JWKS body > 1 MiB (~2.1 MiB) to exercise size-limit guards.
    fn huge_jwks_body() -> String {
        let huge_keys: Vec<String> = (0..100)
            .map(|i| {
                format!(
                    r#"{{"kty":"RSA","kid":"k{i}","n":"{}","e":"AQAB"}}"#,
                    "A".repeat(20_000)
                )
            })
            .collect();
        format!(r#"{{"keys":[{}]}}"#, huge_keys.join(","))
    }

    #[tokio::test]
    async fn jwks_oversized_body_rejected() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let huge_body = huge_jwks_body();

        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/json")
                    .set_body_string(huge_body),
            )
            .mount(&server)
            .await;

        let provider = RemoteJwksProvider::new_for_test(server.uri());
        let result = provider.get_signing_keys().await;
        assert!(
            result.is_err(),
            "oversized JWKS body must be rejected, got Ok"
        );
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("exceeds"),
            "error should mention size limit, got: {err}"
        );
    }

    #[tokio::test]
    async fn jwks_oversized_body_streaming_rejected() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let huge_body = huge_jwks_body();

        // Serve WITHOUT Content-Length using Transfer-Encoding: chunked
        // so the streaming body guard is exercised rather than the CL guard.
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_raw(huge_body, "application/json")
                    .insert_header("transfer-encoding", "chunked"),
            )
            .mount(&server)
            .await;

        let provider = RemoteJwksProvider::new_for_test(server.uri());
        let result = provider.get_signing_keys().await;
        assert!(
            result.is_err(),
            "oversized JWKS body (streaming) must be rejected, got Ok"
        );
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("exceeds"),
            "error should mention size limit, got: {err}"
        );
        assert!(
            err.contains("streaming"),
            "should be streaming guard, not Content-Length guard, got: {err}"
        );
    }

    #[tokio::test]
    async fn jwks_max_age_clamped_to_ceiling() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let body = r#"{"keys":[{"kid":"k1","kty":"RSA","n":"AA","e":"AQAB"}]}"#;

        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_raw(body, "application/json")
                    .insert_header("cache-control", "max-age=100000"),
            )
            .mount(&server)
            .await;

        let provider = RemoteJwksProvider::new_for_test(server.uri());
        provider.refresh().await.unwrap();

        let cache = provider.cache.read().await;
        let cached = cache.as_ref().expect("cache should be populated");
        assert_eq!(
            cached.ttl,
            Duration::from_secs(MAX_JWKS_TTL_SECS),
            "max-age=100000 should be clamped to {}",
            MAX_JWKS_TTL_SECS,
        );
    }

    #[tokio::test]
    async fn jwks_max_age_clamped_to_floor() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let body = r#"{"keys":[{"kid":"k1","kty":"RSA","n":"AA","e":"AQAB"}]}"#;

        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_raw(body, "application/json")
                    .insert_header("cache-control", "max-age=10"),
            )
            .mount(&server)
            .await;

        let provider = RemoteJwksProvider::new_for_test(server.uri());
        provider.refresh().await.unwrap();

        let cache = provider.cache.read().await;
        let cached = cache.as_ref().expect("cache should be populated");
        assert_eq!(
            cached.ttl,
            Duration::from_secs(MIN_JWKS_TTL_SECS),
            "max-age=10 should be clamped to {}",
            MIN_JWKS_TTL_SECS,
        );
    }

    #[tokio::test]
    async fn jwks_default_ttl_used_when_no_cache_control() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let body = r#"{"keys":[{"kid":"k1","kty":"RSA","n":"AA","e":"AQAB"}]}"#;

        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(body, "application/json"))
            .mount(&server)
            .await;

        let provider = RemoteJwksProvider::new_for_test(server.uri());
        provider.refresh().await.unwrap();

        let cache = provider.cache.read().await;
        let cached = cache.as_ref().expect("cache should be populated");
        assert_eq!(
            cached.ttl, provider.default_ttl,
            "no Cache-Control should use default_ttl (unclamped)"
        );
    }

    #[tokio::test]
    async fn refresh_via_wiremock() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let mock_server = MockServer::start().await;
        let body =
            r#"{"keys":[{"kid":"key-1","kty":"RSA","alg":"RS256","n":"modulus","e":"AQAB"}]}"#;

        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(body, "application/json"))
            .mount(&mock_server)
            .await;

        let jwks_uri = format!(
            "{}/realms/test/protocol/openid-connect/certs",
            mock_server.uri()
        );
        let provider = RemoteJwksProvider::new_for_test(jwks_uri);
        provider.refresh().await.unwrap();

        let keys = provider.get_signing_keys().await.unwrap();
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].kid, "key-1");
    }
}
