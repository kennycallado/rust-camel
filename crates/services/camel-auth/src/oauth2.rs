use async_trait::async_trait;
use serde::Deserialize;
use serde::de::Deserializer;
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, RwLock};
use zeroize::Zeroizing;

use camel_api::SsrfPolicy;

use crate::http_client::{SsrfClientOptions, build_ssrf_pinned_client};
use crate::types::AuthError;

fn deserialize_zeroizing_string<'de, D>(deserializer: D) -> Result<Zeroizing<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let s = String::deserialize(deserializer)?;
    Ok(Zeroizing::new(s))
}

const DEFAULT_SKEW: Duration = Duration::from_secs(30);

#[async_trait]
pub trait TokenProvider: Send + Sync + std::fmt::Debug {
    async fn get_token(&self) -> Result<String, AuthError>;
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    #[serde(deserialize_with = "deserialize_zeroizing_string")]
    access_token: Zeroizing<String>,
    #[allow(dead_code)]
    token_type: String,
    expires_in: u64,
}

struct CachedToken {
    access_token: Zeroizing<String>,
    #[allow(dead_code)]
    expires_at: Instant,
    refresh_at: Instant,
}

impl CachedToken {
    fn new(access_token: Zeroizing<String>, expires_in: Duration, skew: Duration) -> Self {
        let expires_at = Instant::now() + expires_in;
        Self {
            access_token,
            refresh_at: expires_at.checked_sub(skew).unwrap_or(expires_at),
            expires_at,
        }
    }

    fn is_usable(&self) -> bool {
        Instant::now() < self.refresh_at
    }
}

pub struct ClientCredentialsProvider {
    token_endpoint: String,
    client_id: String,
    client_secret: Zeroizing<String>,
    scope: Option<String>,
    audience: Option<Vec<String>>,
    cache: RwLock<Option<CachedToken>>,
    refresh_lock: Mutex<()>,
    http: reqwest::Client,
}

impl std::fmt::Debug for ClientCredentialsProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClientCredentialsProvider")
            .field("token_endpoint", &self.token_endpoint)
            .field("client_id", &self.client_id)
            .field("scope", &self.scope)
            .field("audience", &self.audience)
            .finish_non_exhaustive()
    }
}

impl ClientCredentialsProvider {
    /// Creates a production OAuth2 provider with HTTPS enforcement,
    /// SSRF guard, DNS-rebinding protection, and hardened timeouts.
    ///
    /// Validates the token endpoint is a public HTTPS URI, resolves DNS
    /// and pins validated IPs on the HTTP client — closing the TOCTOU
    /// window between validation and the first outbound token request.
    pub async fn new(
        token_endpoint: String,
        client_id: String,
        client_secret: String,
        scope: Option<String>,
        audience: Option<Vec<String>>,
        policy: SsrfPolicy,
    ) -> Result<Self, AuthError> {
        let http = build_ssrf_pinned_client(
            &token_endpoint,
            "OAuth2 token endpoint",
            &SsrfClientOptions::new(policy)
                .with_connect_timeout(Duration::from_secs(10))
                .with_request_timeout(Duration::from_secs(30)),
        )
        .await?;
        Ok(Self {
            token_endpoint,
            client_id,
            client_secret: Zeroizing::new(client_secret),
            scope,
            audience,
            cache: RwLock::new(None),
            refresh_lock: Mutex::new(()),
            http,
        })
    }

    /// Test-only constructor that accepts a pre-built HTTP client.
    ///
    /// Skips SSRF validation and allows injecting a mock-capable client
    /// (e.g. `wiremock`-configured). Do NOT use in production code.
    #[doc(hidden)]
    pub fn new_unchecked_for_test(
        token_endpoint: String,
        client_id: String,
        client_secret: String,
        scope: Option<String>,
        audience: Option<Vec<String>>,
        http: reqwest::Client,
    ) -> Self {
        Self {
            token_endpoint,
            client_id,
            client_secret: Zeroizing::new(client_secret),
            scope,
            audience,
            cache: RwLock::new(None),
            refresh_lock: Mutex::new(()),
            http,
        }
    }

    async fn fetch_token(&self) -> Result<CachedToken, AuthError> {
        let secret = self.client_secret.as_str();
        let mut params: Vec<(&str, &str)> = vec![
            ("grant_type", "client_credentials"),
            ("client_id", &self.client_id),
            ("client_secret", secret),
        ];
        if let Some(ref scope) = self.scope {
            params.push(("scope", scope));
        }
        if let Some(ref audience) = self.audience {
            for aud in audience {
                params.push(("resource", aud));
            }
        }

        let resp = self
            .http
            .post(&self.token_endpoint)
            .form(&params)
            .send()
            .await
            .map_err(|e| AuthError::ProviderUnavailable(format!("OAuth2 request failed: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            let sanitized = if body.len() > 128 {
                format!("{}...(truncated)", &body[..128])
            } else {
                body
            };
            let message = format!("token endpoint returned {status}: {sanitized}"); // allow-secret
            return Err(AuthError::ProviderUnavailable(message));
        }

        let token_resp: TokenResponse = resp
            .json()
            .await
            .map_err(|e| AuthError::ProviderUnavailable(format!("invalid OAuth2 response: {e}")))?;

        Ok(CachedToken::new(
            token_resp.access_token,
            Duration::from_secs(token_resp.expires_in),
            DEFAULT_SKEW,
        ))
    }
}

#[async_trait]
impl TokenProvider for ClientCredentialsProvider {
    async fn get_token(&self) -> Result<String, AuthError> {
        {
            let cache = self.cache.read().await;
            if let Some(ref cached) = *cache
                && cached.is_usable()
            {
                return Ok(cached.access_token.as_str().to_owned());
            }
        }

        let _guard = self.refresh_lock.lock().await;

        {
            let cache = self.cache.read().await;
            if let Some(ref cached) = *cache
                && cached.is_usable()
            {
                return Ok(cached.access_token.as_str().to_owned());
            }
        }

        let cached = self.fetch_token().await?;
        let token = cached.access_token.as_str().to_owned();
        {
            let mut cache = self.cache.write().await;
            *cache = Some(cached);
        }
        Ok(token)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use wiremock::matchers::{body_string_contains, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn oauth2_rejects_private_ip_token_endpoint() {
        // validate_uri catches IP literals like
        // 169.254.169.254 before DNS resolution
        let result = ClientCredentialsProvider::new(
            "https://169.254.169.254/token".into(),
            "client".into(),
            "secret".into(),
            None,
            None,
            SsrfPolicy::PublicHttpsOnly,
        )
        .await;
        assert!(
            result.is_err(),
            "private IP token endpoint should be rejected"
        );
    }

    fn token_response(access_token: &str, expires_in: u64) -> serde_json::Value {
        serde_json::json!({
            "access_token": access_token,
            "token_type": "Bearer",
            "expires_in": expires_in,
        })
    }

    #[tokio::test]
    async fn test_get_token_fresh() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/protocol/openid-connect/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(token_response("abc123", 300)))
            .mount(&server)
            .await;

        let provider = ClientCredentialsProvider::new_unchecked_for_test(
            format!("{}/protocol/openid-connect/token", server.uri()), // allow-secret
            "test-client".into(),
            "test-secret".into(),
            None,
            None,
            reqwest::Client::new(),
        );
        let token = provider.get_token().await.unwrap();
        assert_eq!(token, "abc123");
    }

    #[tokio::test]
    async fn test_get_token_uses_cache() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(token_response("cached", 300)))
            .expect(1)
            .mount(&server)
            .await;

        let provider = ClientCredentialsProvider::new_unchecked_for_test(
            format!("{}/protocol/openid-connect/token", server.uri()), // allow-secret
            "c".into(),
            "s".into(),
            None,
            None,
            reqwest::Client::new(),
        );
        let t1 = provider.get_token().await.unwrap();
        let t2 = provider.get_token().await.unwrap();
        assert_eq!(t1, "cached");
        assert_eq!(t2, "cached");
    }

    #[tokio::test]
    async fn test_get_token_refreshes_when_stale() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(token_response("first", 1)))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(token_response("second", 300)))
            .mount(&server)
            .await;

        let provider = ClientCredentialsProvider::new_unchecked_for_test(
            format!("{}/protocol/openid-connect/token", server.uri()), // allow-secret
            "c".into(),
            "s".into(),
            None,
            None,
            reqwest::Client::new(),
        );
        let t1 = provider.get_token().await.unwrap();
        assert_eq!(t1, "first");
        tokio::time::sleep(Duration::from_millis(1100)).await;
        let t2 = provider.get_token().await.unwrap();
        assert_eq!(t2, "second");
    }

    #[tokio::test]
    async fn test_get_token_server_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;

        let provider = ClientCredentialsProvider::new_unchecked_for_test(
            format!("{}/protocol/openid-connect/token", server.uri()), // allow-secret
            "c".into(),
            "s".into(),
            None,
            None,
            reqwest::Client::new(),
        );
        let err = provider.get_token().await.unwrap_err();
        assert!(matches!(err, AuthError::ProviderUnavailable(_)));
    }

    #[tokio::test]
    async fn test_get_token_invalid_response() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"error": "invalid_grant"})),
            )
            .mount(&server)
            .await;

        let provider = ClientCredentialsProvider::new_unchecked_for_test(
            format!("{}/protocol/openid-connect/token", server.uri()), // allow-secret
            "c".into(),
            "s".into(),
            None,
            None,
            reqwest::Client::new(),
        );
        let err = provider.get_token().await.unwrap_err();
        assert!(matches!(err, AuthError::ProviderUnavailable(_)));
    }

    #[tokio::test]
    async fn test_get_token_sends_audience_as_resource() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(body_string_contains(
                "resource=https%3A%2F%2Fapi.example.com",
            ))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(token_response("aud-token", 300)),
            )
            .mount(&server)
            .await;

        let provider = ClientCredentialsProvider::new_unchecked_for_test(
            format!("{}/protocol/openid-connect/token", server.uri()), // allow-secret
            "c".into(),
            "s".into(),
            None,
            Some(vec!["https://api.example.com".into()]),
            reqwest::Client::new(),
        );
        let token = provider.get_token().await.unwrap();
        assert_eq!(token, "aud-token");
    }

    #[tokio::test]
    async fn test_single_flight_concurrent_callers() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "single-flight-token",
                "token_type": "Bearer",
                "expires_in": 300,
            })))
            .expect(1)
            .mount(&server)
            .await;

        let provider = Arc::new(ClientCredentialsProvider::new_unchecked_for_test(
            format!("{}/protocol/openid-connect/token", server.uri()), // allow-secret
            "c".into(),
            "s".into(),
            None,
            None,
            reqwest::Client::new(),
        ));

        let mut handles = vec![];
        for _ in 0..5 {
            let p = Arc::clone(&provider);
            handles.push(tokio::spawn(async move { p.get_token().await }));
        }
        for h in handles {
            let token = h.await.unwrap().unwrap();
            assert_eq!(token, "single-flight-token");
        }
    }
}
