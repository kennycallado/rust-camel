use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::{Mutex, RwLock};

use crate::jwks::validate_https_public_uri;
use crate::types::AuthError;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct IntrospectionResult {
    pub active: bool,
    #[serde(default)]
    pub sub: Option<String>,
    #[serde(default)]
    pub exp: Option<u64>,
    #[serde(default)]
    pub iat: Option<u64>,
    #[serde(default)]
    pub nbf: Option<u64>,
    #[serde(default)]
    pub scope: Option<String>,
    #[serde(default)]
    pub client_id: Option<String>,
    #[serde(default)]
    pub token_type: Option<String>,
    #[serde(default)]
    pub iss: Option<String>,
    #[serde(default)]
    pub aud: Option<serde_json::Value>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone)]
pub struct IntrospectionCacheOptions {
    pub max_entries: usize,
    pub default_ttl: Duration,
    pub negative_ttl: Duration,
}

impl Default for IntrospectionCacheOptions {
    fn default() -> Self {
        Self {
            max_entries: 10_000,
            default_ttl: Duration::from_secs(60),
            negative_ttl: Duration::from_secs(5),
        }
    }
}

#[async_trait]
pub trait TokenIntrospector: Send + Sync {
    async fn introspect(&self, token: &str) -> Result<IntrospectionResult, AuthError>;
}

pub(crate) struct CachedEntry {
    result: IntrospectionResult,
    expires_at: Instant,
}

pub struct CachingTokenIntrospector {
    endpoint: String,
    client_id: String,
    client_secret: String,
    http: reqwest::Client,
    pub(crate) cache: Arc<RwLock<HashMap<String, CachedEntry>>>,
    in_flight: Mutex<()>,
    max_cache_size: usize,
    default_ttl: Duration,
    negative_ttl: Duration,
}

impl CachingTokenIntrospector {
    pub fn new(
        endpoint: String,
        client_id: String,
        client_secret: String,
        options: IntrospectionCacheOptions,
    ) -> Result<Self, AuthError> {
        validate_https_public_uri(&endpoint, "introspection endpoint URI")?;
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(10))
            .build()
            .map_err(|e| AuthError::ConfigError(format!("failed to build HTTP client: {e}")))?;
        Ok(Self::with_client(
            endpoint,
            client_id,
            client_secret,
            options,
            http,
        ))
    }

    #[doc(hidden)]
    pub fn new_unchecked_for_test(
        endpoint: String,
        client_id: String,
        client_secret: String,
        options: IntrospectionCacheOptions,
    ) -> Self {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(10))
            .build()
            .expect("hardened HTTP client builder config is valid"); // allow-unwrap
        Self::with_client(endpoint, client_id, client_secret, options, http)
    }

    fn with_client(
        endpoint: String,
        client_id: String,
        client_secret: String,
        options: IntrospectionCacheOptions,
        http: reqwest::Client,
    ) -> Self {
        Self {
            endpoint,
            client_id,
            client_secret,
            http,
            cache: Arc::new(RwLock::new(HashMap::new())),
            in_flight: Mutex::new(()),
            max_cache_size: options.max_entries,
            default_ttl: options.default_ttl,
            negative_ttl: options.negative_ttl,
        }
    }

    fn token_hash(token: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(token.as_bytes());
        hex::encode(hasher.finalize())
    }

    fn compute_ttl(&self, result: &IntrospectionResult) -> Duration {
        if !result.active {
            return self.negative_ttl;
        }
        if let Some(exp) = result.exp {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            if exp > now {
                let remaining = Duration::from_secs(exp - now);
                return remaining.min(self.default_ttl);
            }
        }
        self.default_ttl
    }

    async fn evict_if_needed(&self) {
        let mut cache = self.cache.write().await;
        if cache.len() < self.max_cache_size {
            return;
        }
        let now = Instant::now();
        cache.retain(|_, entry| entry.expires_at > now);
        if cache.len() >= self.max_cache_size {
            let oldest_key = cache
                .iter()
                .min_by_key(|(_, e)| e.expires_at)
                .map(|(k, _)| k.clone());
            if let Some(key) = oldest_key {
                cache.remove(&key);
            }
        }
    }
}

impl fmt::Debug for CachingTokenIntrospector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CachingTokenIntrospector")
            .field("endpoint", &self.endpoint)
            .field("client_id", &self.client_id)
            .field("client_secret", &"[REDACTED]")
            .field("max_cache_size", &self.max_cache_size)
            .field("default_ttl", &self.default_ttl)
            .field("negative_ttl", &self.negative_ttl)
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl TokenIntrospector for CachingTokenIntrospector {
    async fn introspect(&self, token: &str) -> Result<IntrospectionResult, AuthError> {
        let key = Self::token_hash(token);

        {
            let cache = self.cache.read().await;
            if let Some(entry) = cache.get(&key)
                && entry.expires_at > Instant::now()
            {
                tracing::debug!(target: "camel_auth::introspection", cache_outcome = "hit");
                return Ok(entry.result.clone());
            }
        }

        let _guard = self.in_flight.lock().await;

        {
            let cache = self.cache.read().await;
            if let Some(entry) = cache.get(&key)
                && entry.expires_at > Instant::now()
            {
                tracing::debug!(target: "camel_auth::introspection", cache_outcome = "hit_after_wait");
                return Ok(entry.result.clone());
            }
        }

        tracing::debug!(
            target: "camel_auth::introspection",
            cache_outcome = "miss"
        );

        let response = self
            .http
            .post(&self.endpoint)
            .form(&[
                ("token", token),
                ("client_id", &self.client_id),
                ("client_secret", &self.client_secret),
            ])
            .send()
            .await
            .map_err(|e| {
                AuthError::ProviderUnavailable(format!("introspection request failed: {e}"))
            })?;

        let status = response.status();
        if status.as_u16() == 401 || status.as_u16() == 403 {
            return Err(AuthError::ProviderUnavailable(
                "introspection client unauthorized".into(),
            ));
        }
        if status.is_server_error() {
            return Err(AuthError::ProviderUnavailable(format!(
                "introspection endpoint returned {}",
                status
            )));
        }
        if status.is_client_error() {
            return Err(AuthError::TokenInvalid(format!(
                "introspection endpoint returned client error {}",
                status
            )));
        }

        let result: IntrospectionResult = response.json().await.map_err(|e| {
            AuthError::ProviderUnavailable(format!("invalid introspection response: {e}"))
        })?;

        let ttl = self.compute_ttl(&result);
        let entry = CachedEntry {
            result: result.clone(),
            expires_at: Instant::now() + ttl,
        };

        self.evict_if_needed().await;
        {
            let mut cache = self.cache.write().await;
            cache.insert(key, entry);
        }

        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{body_string_contains, method};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn deserialize_minimal_active() {
        let json = r#"{"active": true}"#;
        let result: IntrospectionResult = serde_json::from_str(json).unwrap();
        assert!(result.active);
        assert!(result.sub.is_none());
        assert!(result.extra.is_empty());
    }

    #[test]
    fn deserialize_full_rfc7662() {
        let json = r#"{
            "active": true,
            "sub": "user-1",
            "exp": 1700000000,
            "iat": 1699999999,
            "nbf": 1699999900,
            "scope": "read write",
            "client_id": "my-client",
            "token_type": "Bearer",
            "iss": "https://kc.example.com/realms/test",
            "aud": ["my-api"],
            "realm_access": {"roles": ["admin", "user"]},
            "resource_access": {"my-client": {"roles": ["client-role"]}}
        }"#;
        let result: IntrospectionResult = serde_json::from_str(json).unwrap();
        assert!(result.active);
        assert_eq!(result.sub.as_deref(), Some("user-1"));
        assert_eq!(result.exp, Some(1700000000));
        assert_eq!(result.scope.as_deref(), Some("read write"));
        assert_eq!(result.client_id.as_deref(), Some("my-client"));
        assert_eq!(result.token_type.as_deref(), Some("Bearer"));
        assert_eq!(
            result.iss.as_deref(),
            Some("https://kc.example.com/realms/test")
        );
        assert!(result.extra.contains_key("realm_access"));
        assert!(result.extra.contains_key("resource_access"));
    }

    #[test]
    fn deserialize_inactive() {
        let json = r#"{"active": false}"#;
        let result: IntrospectionResult = serde_json::from_str(json).unwrap();
        assert!(!result.active);
    }

    #[test]
    fn deserialize_unknown_fields_go_to_extra() {
        let json = r#"{"active": true, "custom_field": "hello"}"#;
        let result: IntrospectionResult = serde_json::from_str(json).unwrap();
        assert_eq!(result.extra["custom_field"], "hello");
    }

    #[test]
    fn cache_options_defaults() {
        let opts = IntrospectionCacheOptions::default();
        assert_eq!(opts.max_entries, 10_000);
        assert_eq!(opts.default_ttl, Duration::from_secs(60));
        assert_eq!(opts.negative_ttl, Duration::from_secs(5));
    }

    fn test_cache_opts() -> IntrospectionCacheOptions {
        IntrospectionCacheOptions {
            max_entries: 100,
            default_ttl: Duration::from_secs(60),
            negative_ttl: Duration::from_secs(2),
        }
    }

    #[tokio::test]
    async fn cache_hit_returns_cached_result_without_http_call() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "active": true,
                "sub": "cached-user"
            })))
            .expect(1)
            .mount(&server)
            .await;

        let introspector = CachingTokenIntrospector::new_unchecked_for_test(
            server.uri(),
            "client-id".into(),
            "client-secret".into(),
            test_cache_opts(),
        );

        let r1 = introspector.introspect("token-a").await.unwrap();
        let r2 = introspector.introspect("token-a").await.unwrap();
        assert_eq!(r1.sub, r2.sub);
        assert!(r1.active);
    }

    #[tokio::test]
    async fn expired_entry_re_introspects() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "active": true, "sub": "user"
            })))
            .expect(2)
            .mount(&server)
            .await;

        let opts = IntrospectionCacheOptions {
            max_entries: 100,
            default_ttl: Duration::from_millis(50),
            negative_ttl: Duration::from_secs(2),
        };
        let introspector = CachingTokenIntrospector::new_unchecked_for_test(
            server.uri(),
            "cid".into(),
            "cs".into(),
            opts,
        );

        introspector.introspect("tok").await.unwrap();
        tokio::time::sleep(Duration::from_millis(80)).await;
        introspector.introspect("tok").await.unwrap();
    }

    #[tokio::test]
    async fn inactive_token_cached_with_negative_ttl() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "active": false
            })))
            .expect(1)
            .mount(&server)
            .await;

        let opts = IntrospectionCacheOptions {
            max_entries: 100,
            default_ttl: Duration::from_secs(60),
            negative_ttl: Duration::from_secs(10),
        };
        let introspector = CachingTokenIntrospector::new_unchecked_for_test(
            server.uri(),
            "cid".into(),
            "cs".into(),
            opts,
        );

        let r = introspector.introspect("dead-token").await.unwrap();
        assert!(!r.active);
        let r2 = introspector.introspect("dead-token").await.unwrap();
        assert!(!r2.active);
    }

    #[tokio::test]
    async fn cache_key_does_not_contain_raw_token() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"active": true})),
            )
            .mount(&server)
            .await;

        let introspector = CachingTokenIntrospector::new_unchecked_for_test(
            server.uri(),
            "cid".into(),
            "cs".into(),
            test_cache_opts(),
        );

        introspector.introspect("secret-token-value").await.unwrap();
        let cache = introspector.cache.read().await;
        for key in cache.keys() {
            assert!(
                !key.contains("secret-token-value"),
                "cache key must not contain raw token"
            );
        }
    }

    #[tokio::test]
    async fn eviction_removes_oldest_when_over_capacity() {
        let server = MockServer::start().await;
        for i in 0..5 {
            Mock::given(method("POST"))
                .and(body_string_contains(format!("token-{i}"))) // allow-secret
                .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "active": true, "sub": format!("user-{i}")
                })))
                .mount(&server)
                .await;
        }

        let opts = IntrospectionCacheOptions {
            max_entries: 2,
            default_ttl: Duration::from_secs(600),
            negative_ttl: Duration::from_secs(5),
        };
        let introspector = CachingTokenIntrospector::new_unchecked_for_test(
            server.uri(),
            "cid".into(),
            "cs".into(),
            opts,
        );

        introspector.introspect("token-0").await.unwrap();
        introspector.introspect("token-1").await.unwrap();
        introspector.introspect("token-2").await.unwrap();
        introspector.introspect("token-3").await.unwrap();
        introspector.introspect("token-4").await.unwrap();

        let cache = introspector.cache.read().await;
        assert!(cache.len() <= 2, "cache must respect max_entries");
    }

    #[test]
    fn debug_redacts_client_secret() {
        let introspector = CachingTokenIntrospector::new_unchecked_for_test(
            "https://example.com".into(),
            "cid".into(),
            "super-secret-value".into(),
            test_cache_opts(),
        );
        let debug = format!("{introspector:?}");
        assert!(
            !debug.contains("super-secret-value"),
            "Debug must not leak client_secret"
        );
        assert!(debug.contains("REDACTED"));
    }

    #[tokio::test]
    async fn production_constructor_rejects_http_endpoint() {
        let result = CachingTokenIntrospector::new(
            "http://insecure.example.com/introspect".into(),
            "cid".into(),
            "cs".into(),
            test_cache_opts(),
        );
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, AuthError::ConfigError(ref s) if s.contains("HTTPS")));
    }

    #[tokio::test]
    async fn production_constructor_rejects_localhost() {
        let result = CachingTokenIntrospector::new(
            "https://localhost:8080/introspect".into(),
            "cid".into(),
            "cs".into(),
            test_cache_opts(),
        );
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(err, AuthError::ConfigError(ref s) if s.contains("private") || s.contains("loopback"))
        );
    }

    #[tokio::test]
    async fn http_error_500_returns_provider_unavailable() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;

        let introspector = CachingTokenIntrospector::new_unchecked_for_test(
            server.uri(),
            "cid".into(),
            "cs".into(),
            test_cache_opts(),
        );
        let result = introspector.introspect("tok").await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, AuthError::ProviderUnavailable(_)));
    }

    #[tokio::test]
    async fn http_401_returns_provider_unavailable() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;

        let introspector = CachingTokenIntrospector::new_unchecked_for_test(
            server.uri(),
            "cid".into(),
            "cs".into(),
            test_cache_opts(),
        );
        let result = introspector.introspect("tok").await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, AuthError::ProviderUnavailable(ref s) if s.contains("unauthorized")));
    }
}
