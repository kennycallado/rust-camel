use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::net::IpAddr;
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, RwLock};

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

impl RemoteJwksProvider {
    /// Creates a production provider with HTTPS enforcement, SSRF guard, and hardened timeouts.
    pub fn new(jwks_uri: String) -> Result<Self, AuthError> {
        validate_https_public_uri(&jwks_uri, "JWKS URI")?;
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(10))
            .build()
            .map_err(|e| AuthError::ConfigError(format!("failed to build HTTP client: {e}")))?;
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
        let resp = self
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

        let ttl = resp
            .headers()
            .get("cache-control")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| {
                v.split(',').find_map(|part| {
                    let part = part.trim();
                    part.strip_prefix("max-age=")
                        .and_then(|s| s.parse::<u64>().ok())
                        .map(Duration::from_secs)
                })
            })
            .unwrap_or(self.default_ttl);

        #[derive(Deserialize)]
        struct JwksResponse {
            keys: Vec<Jwk>,
        }

        let body: JwksResponse = resp
            .json()
            .await
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

/// Validates that `uri` is a public HTTPS endpoint safe for outbound requests.
///
/// Rules:
/// - Scheme must be `https`
/// - Host must not be a loopback or RFC-1918 private address
pub fn validate_https_public_uri(uri: &str, label: &str) -> Result<(), AuthError> {
    let parsed = uri
        .parse::<reqwest::Url>()
        .map_err(|e| AuthError::ConfigError(format!("invalid {label} '{uri}': {e}")))?;

    if parsed.scheme() != "https" {
        return Err(AuthError::ConfigError(format!(
            "{label} must use HTTPS (got scheme '{}')",
            parsed.scheme()
        )));
    }

    if parsed.host_str().is_some_and(is_private_or_loopback_host) {
        return Err(AuthError::ConfigError(format!(
            "{label} host '{}' is a private or loopback address (SSRF guard)",
            parsed.host_str().unwrap_or("")
        )));
    }

    Ok(())
}

/// Returns `true` if the host string resolves to a loopback or private IP.
fn is_private_or_loopback_host(host: &str) -> bool {
    // Named loopback / unspecified
    if matches!(host, "localhost" | "localhost.localdomain" | "0.0.0.0") {
        return true;
    }
    // url::Url::host_str() wraps IPv6 addresses in brackets: "[::1]".
    // std::net::IpAddr::from_str rejects the bracket form, so strip them first.
    let ip_str = host
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .unwrap_or(host);
    if let Ok(ip) = ip_str.parse::<IpAddr>() {
        return ip.is_loopback() || is_private_ip(ip);
    }
    false
}

fn is_private_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            // RFC 1918 private ranges, link-local (169.254/16 — cloud metadata),
            // loopback (127/8), and unspecified (0.0.0.0).
            v4.is_private() || v4.is_link_local() || v4.is_loopback() || v4.is_unspecified()
        }
        IpAddr::V6(v6) => {
            // Loopback (::1), unique-local (fc00::/7), and unspecified (::).
            v6.is_loopback() || v6.is_unique_local() || v6.is_unspecified()
        }
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

    #[test]
    fn https_enforcement_rejects_http() {
        let result = RemoteJwksProvider::new(
            "http://kc.example.com/realms/test/protocol/openid-connect/certs".into(),
        );
        assert!(matches!(result, Err(AuthError::ConfigError(s)) if s.contains("HTTPS")));
    }

    #[test]
    fn ssrf_guard_rejects_localhost() {
        let result = RemoteJwksProvider::new(
            "https://localhost/realms/test/protocol/openid-connect/certs".into(),
        );
        assert!(matches!(result, Err(AuthError::ConfigError(s)) if s.contains("loopback")));
    }

    #[test]
    fn ssrf_guard_rejects_private_ip() {
        let result = RemoteJwksProvider::new(
            "https://192.168.1.1/realms/test/protocol/openid-connect/certs".into(),
        );
        assert!(matches!(result, Err(AuthError::ConfigError(s)) if s.contains("private")));
    }

    #[test]
    fn production_url_accepted() {
        // Should not fail URL validation (will fail at network level, not construction)
        let result = RemoteJwksProvider::new(
            "https://kc.example.com/realms/test/protocol/openid-connect/certs".into(),
        );
        assert!(result.is_ok());
    }

    #[test]
    fn ssrf_guard_rejects_link_local_metadata_endpoint() {
        // 169.254.169.254 is the cloud instance-metadata address (AWS/GCP/Azure)
        let result = RemoteJwksProvider::new("https://169.254.169.254/latest/meta-data".into());
        assert!(
            matches!(result, Err(AuthError::ConfigError(s)) if s.contains("private") || s.contains("loopback"))
        );
    }

    #[test]
    fn ssrf_guard_rejects_ipv6_unique_local() {
        let result = RemoteJwksProvider::new(
            "https://[fc00::1]/realms/test/protocol/openid-connect/certs".into(),
        );
        assert!(
            matches!(result, Err(AuthError::ConfigError(s)) if s.contains("private") || s.contains("loopback"))
        );
    }

    #[test]
    fn ssrf_guard_rejects_ipv6_loopback() {
        let result = RemoteJwksProvider::new(
            "https://[::1]/realms/test/protocol/openid-connect/certs".into(),
        );
        assert!(
            matches!(result, Err(AuthError::ConfigError(s)) if s.contains("private") || s.contains("loopback"))
        );
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
