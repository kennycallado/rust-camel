use async_trait::async_trait;
use serde::Deserialize;
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, RwLock};

use crate::types::AuthError;

const DEFAULT_SKEW: Duration = Duration::from_secs(30);

#[async_trait]
pub trait TokenProvider: Send + Sync + std::fmt::Debug {
    async fn get_token(&self) -> Result<String, AuthError>;
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    #[allow(dead_code)]
    token_type: String,
    expires_in: u64,
}

struct CachedToken {
    access_token: String,
    #[allow(dead_code)]
    expires_at: Instant,
    refresh_at: Instant,
}

impl CachedToken {
    fn new(access_token: String, expires_in: Duration, skew: Duration) -> Self {
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
    client_secret: String,
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
    pub fn new(
        token_endpoint: String,
        client_id: String,
        client_secret: String,
        scope: Option<String>,
        audience: Option<Vec<String>>,
    ) -> Self {
        Self {
            token_endpoint,
            client_id,
            client_secret,
            scope,
            audience,
            cache: RwLock::new(None),
            refresh_lock: Mutex::new(()),
            http: reqwest::Client::new(),
        }
    }

    /// Test-only constructor that accepts a pre-built HTTP client.
    ///
    /// Skips SSRF validation and allows injecting a mock-capable client
    /// (e.g. `wiremock`-configured). Do NOT use in production code.
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
            client_secret,
            scope,
            audience,
            cache: RwLock::new(None),
            refresh_lock: Mutex::new(()),
            http,
        }
    }

    async fn fetch_token(&self) -> Result<CachedToken, AuthError> {
        let mut params = vec![
            ("grant_type", "client_credentials".to_string()),
            ("client_id", self.client_id.clone()),
            ("client_secret", self.client_secret.clone()),
        ];
        if let Some(ref scope) = self.scope {
            params.push(("scope", scope.clone()));
        }
        if let Some(ref audience) = self.audience {
            for aud in audience {
                params.push(("resource", aud.clone()));
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
                return Ok(cached.access_token.clone());
            }
        }

        let _guard = self.refresh_lock.lock().await;

        {
            let cache = self.cache.read().await;
            if let Some(ref cached) = *cache
                && cached.is_usable()
            {
                return Ok(cached.access_token.clone());
            }
        }

        let cached = self.fetch_token().await?;
        let token = cached.access_token.clone();
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

        let provider = ClientCredentialsProvider::new(
            format!("{}/protocol/openid-connect/token", server.uri()), // allow-secret
            "test-client".into(),
            "test-secret".into(),
            None,
            None,
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

        let provider = ClientCredentialsProvider::new(
            format!("{}/protocol/openid-connect/token", server.uri()), // allow-secret
            "c".into(),
            "s".into(),
            None,
            None,
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

        let provider = ClientCredentialsProvider::new(
            format!("{}/protocol/openid-connect/token", server.uri()), // allow-secret
            "c".into(),
            "s".into(),
            None,
            None,
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

        let provider = ClientCredentialsProvider::new(
            format!("{}/protocol/openid-connect/token", server.uri()), // allow-secret
            "c".into(),
            "s".into(),
            None,
            None,
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

        let provider = ClientCredentialsProvider::new(
            format!("{}/protocol/openid-connect/token", server.uri()), // allow-secret
            "c".into(),
            "s".into(),
            None,
            None,
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

        let provider = ClientCredentialsProvider::new(
            format!("{}/protocol/openid-connect/token", server.uri()), // allow-secret
            "c".into(),
            "s".into(),
            None,
            Some(vec!["https://api.example.com".into()]),
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

        let provider = Arc::new(ClientCredentialsProvider::new(
            format!("{}/protocol/openid-connect/token", server.uri()), // allow-secret
            "c".into(),
            "s".into(),
            None,
            None,
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
