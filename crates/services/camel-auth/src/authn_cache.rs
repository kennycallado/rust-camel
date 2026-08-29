//! Authn result cache: provider-local caching of successful authentication.
//!
//! Mirrors [`CachingPermissionEvaluator`](crate::permission_cache::CachingPermissionEvaluator)'s
//! backend shape — `RwLock<HashMap>` with lazy eviction, no moka, no new
//! dependency — but caches AUTHENTICATION results (minted
//! [`AuthenticatedPrincipal`]s) keyed by provider + request constraints +
//! token hash. Denials are never cached. This is a different plane from
//! `CachingPermissionEvaluator` (which caches authorization decisions) and is
//! purely additive.

use std::collections::HashMap;
use std::fmt;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

use camel_api::security_policy::{AuthPrincipal, TransportId};

use crate::kernel::AuthenticatedPrincipal;

/// Configuration for [`AuthnCache`].
#[derive(Debug, Clone)]
pub struct AuthnCacheOptions {
    /// TTL for cached authentication results. Default 30 s — matches
    /// [`PermissionCacheOptions::positive_ttl`](crate::permission_cache::PermissionCacheOptions::positive_ttl).
    /// The effective entry lifetime is `min(ttl, token_exp - now)`: a cached
    /// authentication never outlives its token.
    pub ttl: Duration,
    /// Maximum number of cache entries before eviction kicks in.
    pub max_entries: usize,
}

impl Default for AuthnCacheOptions {
    fn default() -> Self {
        Self {
            ttl: Duration::from_secs(30),
            max_entries: 10_000,
        }
    }
}

/// Cache key for an authentication result.
///
/// The token is stored only as a SHA-256 hash — the raw token never appears in
/// the key. `Debug` redacts the hash to `[hash]` (ExtractedToken precedent).
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct AuthnCacheKey {
    provider: String,
    audiences: Vec<String>,
    issuers: Vec<String>,
    transport: TransportId,
    token_hash: String,
}

impl AuthnCacheKey {
    /// Build a key from the request-scoped authn inputs, hashing the token.
    pub fn new(
        provider: &str,
        audiences: &[String],
        issuers: &[String],
        transport: TransportId,
        token: &str,
    ) -> Self {
        Self {
            provider: provider.to_string(),
            audiences: audiences.to_vec(),
            issuers: issuers.to_vec(),
            transport,
            token_hash: token_hash(token),
        }
    }
}

impl fmt::Debug for AuthnCacheKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AuthnCacheKey")
            .field("provider", &self.provider)
            .field("audiences", &self.audiences)
            .field("issuers", &self.issuers)
            .field("transport", &self.transport)
            .field("token_hash", &"[hash]")
            .finish()
    }
}

/// SHA-256 hex digest of a token (same utility as
/// [`CachingTokenIntrospector::token_hash`](crate::introspection::CachingTokenIntrospector)).
fn token_hash(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    hex::encode(hasher.finalize())
}

struct CacheEntry {
    principal: AuthenticatedPrincipal,
    expires_at: Instant,
}

/// Provider-local cache of successful authentication results.
///
/// Backed by `std::sync::RwLock<HashMap>` with lazy eviction, mirroring
/// [`CachingPermissionEvaluator`](crate::permission_cache::CachingPermissionEvaluator).
/// Entries never outlive their token: the effective lifetime is
/// `min(configured TTL, token_exp - now)`, and hits re-check `now < expires_at`.
pub struct AuthnCache {
    cache: std::sync::RwLock<HashMap<AuthnCacheKey, CacheEntry>>,
    options: AuthnCacheOptions,
}

impl AuthnCache {
    pub fn new(options: AuthnCacheOptions) -> Self {
        Self {
            cache: std::sync::RwLock::new(HashMap::new()),
            options,
        }
    }

    /// Look up a cached authentication result.
    ///
    /// Returns `None` on miss OR when the entry's token has expired (hits
    /// re-check `now < expires_at`).
    pub fn get(&self, key: &AuthnCacheKey) -> Option<AuthenticatedPrincipal> {
        let cache = self
            .cache
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let entry = cache.get(key)?;
        if Instant::now() < entry.expires_at {
            Some(entry.principal.clone())
        } else {
            None
        }
    }

    /// Insert a minted principal, returning `false` when it is not stored.
    ///
    /// The effective lifetime is `min(configured TTL, token_exp - now)`; a
    /// token whose `exp` has already passed (≤ 0 remaining) is NOT stored.
    /// Tokens without an `exp` claim (e.g. static native tokens) use the
    /// configured TTL.
    pub fn insert(&self, key: AuthnCacheKey, principal: AuthenticatedPrincipal) -> bool {
        let now = Instant::now();
        let lifetime = match token_remaining(&principal) {
            Some(remaining) => {
                if remaining.is_zero() {
                    return false;
                }
                remaining.min(self.options.ttl)
            }
            None => self.options.ttl,
        };

        self.evict_if_needed();

        let mut cache = self
            .cache
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        cache.insert(
            key,
            CacheEntry {
                principal,
                expires_at: now + lifetime,
            },
        );
        true
    }

    pub fn len(&self) -> usize {
        let cache = self
            .cache
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        cache.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Lazy eviction: drop expired entries, then evict the oldest when still
    /// over capacity (mirrors `CachingPermissionEvaluator::evict_if_needed`).
    fn evict_if_needed(&self) {
        let mut cache = self
            .cache
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if cache.len() < self.options.max_entries {
            return;
        }
        let now = Instant::now();
        cache.retain(|_, entry| now < entry.expires_at);
        if cache.len() >= self.options.max_entries {
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

impl fmt::Debug for AuthnCache {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AuthnCache")
            .field("ttl", &self.options.ttl)
            .field("max_entries", &self.options.max_entries)
            .finish_non_exhaustive()
    }
}

/// Remaining token lifetime derived from the `exp` claim (unix seconds) on the
/// minted principal's claims. `None` when the token carries no `exp` claim;
/// `Some(0)` when `exp` has already passed (saturating — never cached).
fn token_remaining(principal: &AuthenticatedPrincipal) -> Option<Duration> {
    let exp = principal.principal().claims.get("exp")?.as_u64()?;
    let now_wall = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    Some(Duration::from_secs(exp.saturating_sub(now_wall)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::claims::{ClaimPaths, JsonPointerClaimsMapper};
    use crate::credential_source::{CredentialSource, ExtractedToken};
    use crate::jwks::{Jwk, JwksProvider};
    use crate::jwt::LocalJwtValidator;
    use crate::kernel::kernel_authenticate;
    use crate::registry::{ProviderEntry, ProviderRegistry};
    use crate::token_authenticator::AuthnRequest;
    use crate::types::AuthError;
    use camel_api::CamelError;
    use camel_api::security_policy::{AccessMode, AudienceBinding, Principal, RouteSecurityPlan};
    use serde_json::json;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static TEST_RSA_PRIVATE_PEM: &[u8] = include_bytes!("../tests/fixtures/test_rsa_private.pem");
    static TEST_RSA_PUBLIC_PEM: &[u8] = include_bytes!("../tests/fixtures/test_rsa_public.pem");

    fn test_principal() -> Principal {
        Principal {
            subject: "svc-user".into(),
            issuer: "test".into(),
            audience: vec![],
            scopes: vec![],
            roles: vec![],
            claims: serde_json::Value::Null,
        }
    }

    /// Authenticator that counts calls and accepts or rejects every token.
    struct CountingAuthenticator {
        count: AtomicUsize,
        accept: bool,
    }

    #[async_trait::async_trait]
    impl crate::token_authenticator::TokenAuthenticator for CountingAuthenticator {
        async fn authenticate_bearer(&self, _token: &str) -> Result<Principal, CamelError> {
            self.count.fetch_add(1, Ordering::SeqCst);
            if self.accept {
                Ok(test_principal())
            } else {
                Err(CamelError::Unauthenticated("bad token".into()))
            }
        }
    }

    /// Counting wrapper around a real [`LocalJwtValidator`] that ALSO enforces
    /// `exp` itself: jsonwebtoken's default 60 s leeway would otherwise accept
    /// the token past its `exp`, which would mask the cache's expiry behavior.
    struct CountingValidator {
        inner: LocalJwtValidator,
        count: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl crate::token_authenticator::TokenAuthenticator for CountingValidator {
        async fn authenticate_bearer(&self, token: &str) -> Result<Principal, CamelError> {
            self.count.fetch_add(1, Ordering::SeqCst);
            self.inner.authenticate_bearer(token).await
        }

        async fn authenticate(&self, req: AuthnRequest<'_>) -> Result<Principal, CamelError> {
            self.count.fetch_add(1, Ordering::SeqCst);
            let now_wall = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            if let Some(exp) = token_exp(req.token)
                && exp <= now_wall
            {
                return Err(CamelError::Unauthenticated("token expired".into()));
            }
            self.inner.authenticate(req).await
        }
    }

    /// Decode the `exp` claim (unix seconds) from a JWT payload without
    /// signature verification (test-only helper).
    fn token_exp(token: &str) -> Option<u64> {
        use base64::Engine;
        let payload = token.split('.').nth(1)?;
        let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(payload)
            .ok()?;
        let claims: serde_json::Value = serde_json::from_slice(&decoded).ok()?;
        claims.get("exp")?.as_u64()
    }

    struct MockJwks {
        kid: String,
        public_pem: &'static [u8],
    }

    #[async_trait::async_trait]
    impl JwksProvider for MockJwks {
        async fn get_signing_keys(&self) -> Result<Vec<Jwk>, AuthError> {
            Ok(vec![Jwk {
                kid: self.kid.clone(),
                kty: "RSA".into(),
                alg: Some("RS256".into()),
                r#use: None,
                n: String::from_utf8_lossy(self.public_pem).into_owned(),
                e: "AQAB".into(),
            }])
        }

        async fn refresh(&self) -> Result<(), AuthError> {
            Ok(())
        }
    }

    fn jwt_validator(audience: Vec<&str>, issuer: &str) -> LocalJwtValidator {
        let mapper = Arc::new(JsonPointerClaimsMapper::new(ClaimPaths {
            subject: "/sub".into(),
            roles: vec!["/groups".into()],
            scopes: Some("/scope".into()),
        }));
        LocalJwtValidator::new(
            audience.iter().map(|s| s.to_string()).collect(),
            issuer.to_string(),
            Arc::new(MockJwks {
                kid: "test-key".into(),
                public_pem: TEST_RSA_PUBLIC_PEM,
            }),
            mapper,
        )
    }

    fn make_token(kid: &str, claims: &serde_json::Value) -> String {
        let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256);
        header.kid = Some(kid.to_string());
        let encoding_key = jsonwebtoken::EncodingKey::from_rsa_pem(TEST_RSA_PRIVATE_PEM).unwrap();
        jsonwebtoken::encode(&header, claims, &encoding_key).unwrap()
    }

    fn plan(
        provider_ref: &str,
        transport: TransportId,
        binding: Option<AudienceBinding>,
    ) -> RouteSecurityPlan {
        RouteSecurityPlan {
            access_mode: AccessMode::Authenticated,
            provider_ref: Some(provider_ref.to_string()),
            transport,
            credential_sources: vec![CredentialSource::AuthorizationHeader],
            audience_binding: binding,
        }
    }

    fn credentials(token: &str) -> ExtractedToken {
        ExtractedToken {
            token: token.to_string(),
            source: CredentialSource::AuthorizationHeader,
        }
    }

    fn cached_registry(
        accept: bool,
    ) -> (
        Arc<AuthnCache>,
        Arc<CountingAuthenticator>,
        ProviderRegistry,
    ) {
        let cache = Arc::new(AuthnCache::new(AuthnCacheOptions::default()));
        let auth = Arc::new(CountingAuthenticator {
            count: AtomicUsize::new(0),
            accept,
        });
        let registry = ProviderRegistry::new().with_authn_cache(cache.clone());
        registry.register(
            "idp-a",
            ProviderEntry {
                authenticator: auth.clone(),
                audience_binding: None,
            },
        );
        (cache, auth, registry)
    }

    #[tokio::test]
    async fn cache_separates_providers() {
        let cache = Arc::new(AuthnCache::new(AuthnCacheOptions::default()));
        let binding = AudienceBinding {
            issuers: vec!["https://a".into()],
            audiences: vec!["api".into()],
        };
        let auth_a = Arc::new(CountingAuthenticator {
            count: AtomicUsize::new(0),
            accept: true,
        });
        let auth_b = Arc::new(CountingAuthenticator {
            count: AtomicUsize::new(0),
            accept: true,
        });
        let registry = ProviderRegistry::new().with_authn_cache(cache.clone());
        registry.register(
            "idp-a",
            ProviderEntry {
                authenticator: auth_a.clone(),
                audience_binding: Some(binding.clone()),
            },
        );
        registry.register(
            "idp-b",
            ProviderEntry {
                authenticator: auth_b.clone(),
                audience_binding: Some(binding.clone()),
            },
        );

        let token = "same-token";
        let plan_a = plan("idp-a", TransportId::Http, Some(binding.clone()));
        let plan_b = plan("idp-b", TransportId::Http, Some(binding.clone()));

        kernel_authenticate(&plan_a, &registry, &credentials(token))
            .await
            .unwrap();
        kernel_authenticate(&plan_b, &registry, &credentials(token))
            .await
            .unwrap();

        assert_eq!(
            cache.len(),
            2,
            "provider is part of the key — identical binding+token must yield two entries"
        );
        assert_eq!(auth_a.count.load(Ordering::SeqCst), 1);
        assert_eq!(auth_b.count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn cache_separates_bindings() {
        // Same provider+transport+token, different audience bindings —
        // the binding is part of the key, so the 3.1 fail-closed
        // enforcement cannot regress via a cache collision.
        let cache = Arc::new(AuthnCache::new(AuthnCacheOptions::default()));
        let binding_a = AudienceBinding {
            issuers: vec!["https://a".into()],
            audiences: vec!["api-a".into()],
        };
        let binding_b = AudienceBinding {
            issuers: vec!["https://a".into()],
            audiences: vec!["api-b".into()],
        };
        let auth = Arc::new(CountingAuthenticator {
            count: AtomicUsize::new(0),
            accept: true,
        });
        let registry = ProviderRegistry::new().with_authn_cache(cache.clone());
        registry.register(
            "idp-a",
            ProviderEntry {
                authenticator: auth.clone(),
                audience_binding: Some(binding_a.clone()),
            },
        );

        let token = "same-token";
        let plan_a = plan("idp-a", TransportId::Http, Some(binding_a));
        let plan_b = plan("idp-a", TransportId::Http, Some(binding_b));

        kernel_authenticate(&plan_a, &registry, &credentials(token))
            .await
            .unwrap();
        kernel_authenticate(&plan_b, &registry, &credentials(token))
            .await
            .unwrap();

        assert_eq!(
            cache.len(),
            2,
            "audiences/issuers are part of the key — different bindings must not collide"
        );
        assert_eq!(auth.count.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn cache_separates_transports() {
        let (cache, auth, registry) = cached_registry(true);
        let token = "same-token";

        let plan_http = plan("idp-a", TransportId::Http, None);
        let plan_ws = plan("idp-a", TransportId::Ws, None);

        kernel_authenticate(&plan_http, &registry, &credentials(token))
            .await
            .unwrap();
        kernel_authenticate(&plan_ws, &registry, &credentials(token))
            .await
            .unwrap();

        assert_eq!(
            cache.len(),
            2,
            "transport is part of the key — same provider+token on Http and Ws must yield two entries"
        );
        assert_eq!(auth.count.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn cache_key_debug_redacts_token() {
        let key = AuthnCacheKey::new(
            "idp-a",
            &["api".to_string()],
            &["https://a".to_string()],
            TransportId::Http,
            "super-secret-token-value",
        );
        let debug = format!("{key:?}");
        assert!(
            !debug.contains("super-secret-token-value"),
            "Debug must not leak the token: {debug}"
        );
        assert!(
            debug.contains("[hash]"),
            "Debug must show the redaction marker: {debug}"
        );
    }

    #[tokio::test]
    async fn denials_not_cached() {
        let (cache, auth, registry) = cached_registry(false);
        let plan = plan("idp-a", TransportId::Http, None);
        let token = "wrong-token";

        let r1 = kernel_authenticate(&plan, &registry, &credentials(token)).await;
        let r2 = kernel_authenticate(&plan, &registry, &credentials(token)).await;

        assert!(matches!(r1, Err(CamelError::Unauthenticated(_))));
        assert!(matches!(r2, Err(CamelError::Unauthenticated(_))));
        assert_eq!(
            auth.count.load(Ordering::SeqCst),
            2,
            "denials must not be cached — the provider is called every time"
        );
        assert_eq!(cache.len(), 0, "denials are never inserted");
    }

    #[tokio::test]
    async fn expired_token_not_served_from_cache() {
        // JWT with exp 5s ahead: the cache entry lifetime is min(ttl, exp - now).
        let now = chrono::Utc::now().timestamp() as u64;
        let claims = json!({
            "sub": "user-1",
            "iss": "https://a",
            "aud": "api",
            "exp": now + 5,
            "iat": now,
        });
        let token = make_token("test-key", &claims);

        let counting = Arc::new(CountingValidator {
            inner: jwt_validator(vec!["api"], "https://a"),
            count: AtomicUsize::new(0),
        });
        let cache = Arc::new(AuthnCache::new(AuthnCacheOptions::default()));
        let binding = AudienceBinding {
            issuers: vec!["https://a".into()],
            audiences: vec!["api".into()],
        };
        let registry = ProviderRegistry::new().with_authn_cache(cache.clone());
        registry.register(
            "idp-a",
            ProviderEntry {
                authenticator: counting.clone(),
                audience_binding: Some(binding.clone()),
            },
        );
        let plan = plan("idp-a", TransportId::Http, Some(binding));

        // First call: authenticates and is cached with a ~1s lifetime.
        let p1 = kernel_authenticate(&plan, &registry, &credentials(&token))
            .await
            .unwrap();
        assert_eq!(p1.provider_id(), "idp-a");
        assert_eq!(counting.count.load(Ordering::SeqCst), 1);

        // Sleep past exp (tokio time).
        tokio::time::sleep(Duration::from_secs(6)).await;

        // Second call: the hit re-checks exp — NOT served from cache; the
        // provider is called again and rejects the now-expired token.
        let r2 = kernel_authenticate(&plan, &registry, &credentials(&token)).await;
        assert!(
            matches!(r2, Err(CamelError::Unauthenticated(_))),
            "expired token must not be served from cache"
        );
        assert_eq!(
            counting.count.load(Ordering::SeqCst),
            2,
            "post-expiry re-auth must call the provider again"
        );
    }
}
