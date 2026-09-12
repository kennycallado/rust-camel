use async_trait::async_trait;
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header};
use std::sync::Arc;

use crate::claims::ClaimsMapper;
use crate::jwks::{Jwk, JwksProvider};
use crate::types::AuthError;
use camel_api::security_policy::Principal;

/// Validates JWT tokens and extracts a [`Principal`].
#[async_trait]
pub trait JwtValidator: Send + Sync {
    async fn validate(&self, token: &str) -> Result<Principal, AuthError>;

    /// Signature-only verification: constructor keyset verification, NO
    /// issuer/audience check.
    ///
    /// The default delegates to [`validate`](Self::validate) for implementors
    /// that do not distinguish signature-only from full validation.
    async fn validate_signature(&self, token: &str) -> Result<Principal, AuthError> {
        self.validate(token).await
    }
}

/// Production JWT validator backed by a dynamic JWKS provider.
///
/// Delegates Principal construction to a configurable [`ClaimsMapper`],
/// allowing provider-specific claim shapes without hardcoding extraction logic.
pub struct LocalJwtValidator {
    audience: Vec<String>,
    issuer: String,
    jwks: Arc<dyn JwksProvider>,
    mapper: Arc<dyn ClaimsMapper>,
}

impl LocalJwtValidator {
    pub fn new(
        audience: Vec<String>,
        issuer: String,
        jwks: Arc<dyn JwksProvider>,
        mapper: Arc<dyn ClaimsMapper>,
    ) -> Self {
        Self {
            audience,
            issuer,
            jwks,
            mapper,
        }
    }
}

/// Convert a JWK to a [`DecodingKey`].
///
/// Supports both PEM-encoded public keys (stored in `n` with a `-----BEGIN` prefix,
/// useful for testing) and standard JWKS base64url components (production).
fn jwk_to_decoding_key(n: &str, e: &str) -> Result<DecodingKey, AuthError> {
    if n.starts_with("-----BEGIN") {
        DecodingKey::from_rsa_pem(n.as_bytes())
            .map_err(|e| AuthError::TokenInvalid(format!("invalid RSA PEM: {e}"))) // allow-secret
    } else {
        DecodingKey::from_rsa_components(n, e)
            .map_err(|e| AuthError::TokenInvalid(format!("invalid JWK components: {e}"))) // allow-secret
    }
}

/// Expected signing algorithm — must match [`Validation::new(Algorithm::RS256)`].
const EXPECTED_ALG: &str = "RS256";

/// Returns `true` if the JWK matches `kid` and passes alg/use filters.
///
/// A JWK is considered acceptable when:
/// - Its `kid` matches the token header.
/// - Its `alg` is either absent (spec default) or equals [`EXPECTED_ALG`].
/// - Its `use` is either absent (spec default) or equals `"sig"`.
fn key_matches(k: &Jwk, kid: &str) -> bool {
    k.kid == kid
        && k.alg.as_deref().is_none_or(|a| a == EXPECTED_ALG)
        && k.r#use.as_deref().is_none_or(|u| u == "sig")
}

#[async_trait]
impl JwtValidator for LocalJwtValidator {
    async fn validate(&self, token: &str) -> Result<Principal, AuthError> {
        let principal = self.validate_signature(token).await?;

        // Fixed-claims-check: enforce the constructor-configured audience/issuer.
        //
        // Fail-closed: an empty constructor audience/issuer rejects. This
        // byte-preserves the old jsonwebtoken behavior where an empty configured
        // set matched nothing (reject-all), preventing an empty-audience config
        // from authenticating with zero audience scoping.
        if self.audience.is_empty() || !principal.audience.iter().any(|a| self.audience.contains(a))
        {
            return Err(AuthError::TokenInvalid("invalid audience".into()));
        }
        if self.issuer.is_empty() || principal.issuer != self.issuer {
            return Err(AuthError::TokenInvalid("invalid issuer".into()));
        }

        Ok(principal)
    }

    async fn validate_signature(&self, token: &str) -> Result<Principal, AuthError> {
        // Decode header to extract kid
        let header = decode_header(token)
            .map_err(|e| AuthError::TokenInvalid(format!("invalid JWT header: {e}")))?;

        let kid = header
            .kid
            .ok_or_else(|| AuthError::TokenInvalid("JWT missing kid".into()))?;

        // Fetch signing keys; on kid miss, force a JWKS refresh (handles key rotation)
        let keys = self.jwks.get_signing_keys().await?;
        let jwk = if let Some(k) = keys.iter().find(|k| key_matches(k, &kid)) {
            k.clone()
        } else {
            // Key not in cache — might be a newly rotated key; refresh once and retry
            self.jwks.refresh().await?;
            self.jwks
                .get_signing_keys()
                .await?
                .into_iter()
                .find(|k| key_matches(k, &kid))
                .ok_or_else(|| {
                    AuthError::TokenInvalid(format!("no key for kid={kid} after refresh"))
                })?
        };

        let decoding_key = jwk_to_decoding_key(&jwk.n, &jwk.e)?;

        // Configure validation — signature/validity only, NO issuer/audience check.
        let mut validation = Validation::new(Algorithm::RS256);
        validation.validate_aud = false;

        // Decode and verify
        let token_data =
            decode::<serde_json::Value>(token, &decoding_key, &validation).map_err(|e| match e
                .kind()
            {
                jsonwebtoken::errors::ErrorKind::ExpiredSignature => AuthError::TokenExpired,
                _ => AuthError::TokenInvalid(e.to_string()),
            })?;

        let claims = token_data.claims;

        // Delegate Principal construction to the configured ClaimsMapper
        self.mapper.to_principal(&claims)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::claims::{ClaimPaths, JsonPointerClaimsMapper};
    use crate::jwks::Jwk;
    use jsonwebtoken::{EncodingKey, Header, encode};
    use serde_json::json;

    static TEST_RSA_PRIVATE_PEM: &[u8] = include_bytes!("../tests/fixtures/test_rsa_private.pem");
    static TEST_RSA_PUBLIC_PEM: &[u8] = include_bytes!("../tests/fixtures/test_rsa_public.pem");

    /// Mock JWKS provider that returns a PEM-encoded public key.
    struct MockJwks {
        kid: String,
        public_pem: &'static [u8],
    }

    #[async_trait]
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

    /// Mock JWKS that starts empty and gains a key after refresh (simulates rotation).
    struct RotatingMockJwks {
        kid: String,
        public_pem: &'static [u8],
        refreshed: std::sync::atomic::AtomicBool,
    }

    #[async_trait]
    impl JwksProvider for RotatingMockJwks {
        async fn get_signing_keys(&self) -> Result<Vec<Jwk>, AuthError> {
            if self.refreshed.load(std::sync::atomic::Ordering::SeqCst) {
                Ok(vec![Jwk {
                    kid: self.kid.clone(),
                    kty: "RSA".into(),
                    alg: Some("RS256".into()),
                    r#use: None,
                    n: String::from_utf8_lossy(self.public_pem).into_owned(),
                    e: "AQAB".into(),
                }])
            } else {
                Ok(vec![]) // key not yet known
            }
        }

        async fn refresh(&self) -> Result<(), AuthError> {
            self.refreshed
                .store(true, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }
    }

    /// Build a mapper configured for multiple role paths.
    fn multi_role_mapper(role_paths: Vec<String>) -> Arc<JsonPointerClaimsMapper> {
        Arc::new(JsonPointerClaimsMapper::new(ClaimPaths {
            subject: "/sub".into(),
            roles: role_paths,
            scopes: Some("/scope".into()),
        }))
    }

    fn validator(audience: Vec<&str>, mapper: Arc<dyn ClaimsMapper>) -> LocalJwtValidator {
        LocalJwtValidator::new(
            audience.iter().map(|s| s.to_string()).collect(),
            "http://localhost:8080/realms/test".into(),
            Arc::new(MockJwks {
                kid: "test-key".into(),
                public_pem: TEST_RSA_PUBLIC_PEM,
            }),
            mapper,
        )
    }

    fn validator_with_jwk(jwk: Jwk, mapper: Arc<dyn ClaimsMapper>) -> LocalJwtValidator {
        struct SingleKeyJwks {
            jwk: Jwk,
        }

        #[async_trait]
        impl JwksProvider for SingleKeyJwks {
            async fn get_signing_keys(&self) -> Result<Vec<Jwk>, AuthError> {
                Ok(vec![self.jwk.clone()])
            }
            async fn refresh(&self) -> Result<(), AuthError> {
                Ok(())
            }
        }

        LocalJwtValidator::new(
            vec!["my-api".into()],
            "http://localhost:8080/realms/test".into(),
            Arc::new(SingleKeyJwks { jwk }),
            mapper,
        )
    }

    /// Standard claims set valid for "my-api" audience, used by multiple tests.
    fn claims_with_defaults() -> serde_json::Value {
        let now = chrono::Utc::now().timestamp() as u64;
        json!({
            "sub": "user-123",
            "iss": "http://localhost:8080/realms/test",
            "aud": "my-api",
            "exp": now + 3600,
            "iat": now,
        })
    }

    fn make_token(kid: &str, claims: &serde_json::Value) -> String {
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some(kid.to_string());
        let encoding_key = EncodingKey::from_rsa_pem(TEST_RSA_PRIVATE_PEM).unwrap();
        encode(&header, claims, &encoding_key).unwrap()
    }

    #[tokio::test]
    async fn validates_valid_token() {
        let v = validator(vec!["my-api"], multi_role_mapper(vec!["/groups".into()]));
        let now = chrono::Utc::now().timestamp() as u64;
        let claims = json!({
            "sub": "user-123",
            "iss": "http://localhost:8080/realms/test",
            "aud": "my-api",
            "exp": now + 3600,
            "iat": now,
        });
        let token = make_token("test-key", &claims);
        let principal = v.validate(&token).await.unwrap();
        assert_eq!(principal.subject, "user-123");
    }

    #[tokio::test]
    async fn rejects_expired_token() {
        let v = validator(vec!["my-api"], multi_role_mapper(vec!["/groups".into()]));
        let now = chrono::Utc::now().timestamp() as u64;
        let claims = json!({
            "sub": "user-123",
            "iss": "http://localhost:8080/realms/test",
            "aud": "my-api",
            "exp": now - 3600,
            "iat": now - 7200,
        });
        let token = make_token("test-key", &claims);
        assert!(matches!(
            v.validate(&token).await,
            Err(AuthError::TokenExpired)
        ));
    }

    #[tokio::test]
    async fn rejects_wrong_audience() {
        let v = validator(vec!["my-api"], multi_role_mapper(vec!["/groups".into()]));
        let now = chrono::Utc::now().timestamp() as u64;
        let claims = json!({
            "sub": "user-123",
            "iss": "http://localhost:8080/realms/test",
            "aud": "wrong-audience",
            "exp": now + 3600,
            "iat": now,
        });
        let token = make_token("test-key", &claims);
        assert!(matches!(
            v.validate(&token).await,
            Err(AuthError::TokenInvalid(_))
        ));
    }

    #[tokio::test]
    async fn extracts_resource_access_roles() {
        let mapper = multi_role_mapper(vec![
            "/realm_access/roles".into(),
            "/resource_access/my-client/roles".into(),
        ]);
        let v = validator(vec!["my-client"], mapper);
        let now = chrono::Utc::now().timestamp() as u64;
        let claims = json!({
            "sub": "user-123",
            "iss": "http://localhost:8080/realms/test",
            "aud": "my-client",
            "exp": now + 3600,
            "iat": now,
            "realm_access": { "roles": ["realm-role"] },
            "resource_access": {
                "my-client": { "roles": ["client-role-a"] }
            },
        });
        let token = make_token("test-key", &claims);
        let principal = v.validate(&token).await.unwrap();
        assert!(principal.has_role("realm-role"));
        assert!(principal.has_role("client-role-a"));
    }

    #[tokio::test]
    async fn rejects_missing_sub() {
        let v = validator(vec!["my-api"], multi_role_mapper(vec!["/groups".into()]));
        let now = chrono::Utc::now().timestamp() as u64;
        let claims = json!({
            // "sub" intentionally absent
            "iss": "http://localhost:8080/realms/test",
            "aud": "my-api",
            "exp": now + 3600,
            "iat": now,
        });
        let token = make_token("test-key", &claims);
        assert!(matches!(
            v.validate(&token).await,
            Err(AuthError::TokenInvalid(_))
        ));
    }

    #[tokio::test]
    async fn refreshes_on_unknown_kid() {
        let now = chrono::Utc::now().timestamp() as u64;
        let claims = json!({
            "sub": "user-123",
            "iss": "http://localhost:8080/realms/test",
            "aud": "my-api",
            "exp": now + 3600,
            "iat": now,
        });
        let token = make_token("test-key", &claims);

        // Validator backed by a JWKS that returns the key only after refresh
        let v = LocalJwtValidator::new(
            vec!["my-api".into()],
            "http://localhost:8080/realms/test".into(),
            Arc::new(RotatingMockJwks {
                kid: "test-key".into(),
                public_pem: TEST_RSA_PUBLIC_PEM,
                refreshed: std::sync::atomic::AtomicBool::new(false),
            }),
            multi_role_mapper(vec!["/groups".into()]),
        );

        // Token should validate after the forced JWKS refresh
        let principal = v.validate(&token).await.unwrap();
        assert_eq!(principal.subject, "user-123");
    }

    #[tokio::test]
    async fn mapper_configures_role_paths_independently_of_audience() {
        // Mapper is configured with explicit role paths — no audience heuristic needed.
        // Token audience is "other-audience" but mapper looks up roles under "my-service".
        let mapper = multi_role_mapper(vec![
            "/realm_access/roles".into(),
            "/resource_access/my-service/roles".into(),
        ]);
        let v = validator(vec!["other-audience"], mapper);

        let now = chrono::Utc::now().timestamp() as u64;
        let claims = json!({
            "sub": "user-123",
            "iss": "http://localhost:8080/realms/test",
            "aud": "other-audience",
            "exp": now + 3600,
            "iat": now,
            "resource_access": {
                "my-service": { "roles": ["svc-role"] },
                "other-audience": { "roles": ["aud-role"] },
            },
        });
        let token = make_token("test-key", &claims);
        let principal = v.validate(&token).await.unwrap();

        // Mapper finds "svc-role" under "my-service" via configured path,
        // NOT "aud-role" under "other-audience".
        assert!(
            principal.has_role("svc-role"),
            "expected svc-role from my-service path"
        );
        assert!(
            !principal.has_role("aud-role"),
            "must not pick aud-role when mapper path targets my-service"
        );
    }

    #[tokio::test]
    async fn extracts_scopes_from_scope_claim() {
        let mapper = multi_role_mapper(vec!["/groups".into()]);
        let v = validator(vec!["my-api"], mapper);
        let now = chrono::Utc::now().timestamp() as u64;
        let claims = json!({
            "sub": "user-123",
            "iss": "http://localhost:8080/realms/test",
            "aud": "my-api",
            "exp": now + 3600,
            "iat": now,
            "scope": "read write admin",
        });
        let token = make_token("test-key", &claims);
        let principal = v.validate(&token).await.unwrap();
        assert_eq!(principal.scopes, vec!["read", "write", "admin"]);
    }

    #[tokio::test]
    async fn rejects_key_with_alg_mismatch() {
        let jwk = Jwk {
            kid: "test-key".into(),
            kty: "RSA".into(),
            alg: Some("HS256".into()),
            r#use: None,
            n: String::from_utf8_lossy(TEST_RSA_PUBLIC_PEM).into_owned(),
            e: "AQAB".into(),
        };
        let v = validator_with_jwk(jwk, multi_role_mapper(vec!["/groups".into()]));
        let token = make_token("test-key", &claims_with_defaults());
        assert!(matches!(
            v.validate(&token).await,
            Err(AuthError::TokenInvalid(_))
        ));
    }

    #[tokio::test]
    async fn rejects_key_with_use_mismatch() {
        let jwk = Jwk {
            kid: "test-key".into(),
            kty: "RSA".into(),
            alg: Some("RS256".into()),
            r#use: Some("enc".into()),
            n: String::from_utf8_lossy(TEST_RSA_PUBLIC_PEM).into_owned(),
            e: "AQAB".into(),
        };
        let v = validator_with_jwk(jwk, multi_role_mapper(vec!["/groups".into()]));
        let token = make_token("test-key", &claims_with_defaults());
        assert!(matches!(
            v.validate(&token).await,
            Err(AuthError::TokenInvalid(_))
        ));
    }

    #[tokio::test]
    async fn accepts_key_without_alg_or_use() {
        let jwk = Jwk {
            kid: "test-key".into(),
            kty: "RSA".into(),
            alg: None,
            r#use: None,
            n: String::from_utf8_lossy(TEST_RSA_PUBLIC_PEM).into_owned(),
            e: "AQAB".into(),
        };
        let v = validator_with_jwk(jwk, multi_role_mapper(vec!["/groups".into()]));
        let token = make_token("test-key", &claims_with_defaults());
        let principal = v.validate(&token).await.unwrap();
        assert_eq!(principal.subject, "user-123");
    }

    // ---- RemoteJwksProvider-backed validator harness (jwks-refresh-guard) ----

    use std::time::Duration;

    use crate::jwks::RemoteJwksProvider;

    /// JWKS document carrying the test public PEM in `n` (PEM-in-`n` convention
    /// accepted by `jwk_to_decoding_key`).
    fn jwks_body(kid: &str) -> String {
        let pem = std::str::from_utf8(TEST_RSA_PUBLIC_PEM).unwrap();
        serde_json::json!({
            "keys": [{
                "kid": kid,
                "kty": "RSA",
                "alg": "RS256",
                "n": pem,
                "e": "AQAB",
            }]
        })
        .to_string()
    }

    /// Validator backed by a real `RemoteJwksProvider` against a wiremock URI,
    /// with a 100ms forced-refresh cooldown. The second handle is kept for
    /// priming the provider cache.
    fn remote_validator(server_uri: String) -> (LocalJwtValidator, Arc<RemoteJwksProvider>) {
        remote_validator_with_cooldown(server_uri, Duration::from_millis(100))
    }

    /// Like `remote_validator`, but with an explicit forced-refresh cooldown so
    /// individual tests can widen the margin against CI scheduling stalls.
    fn remote_validator_with_cooldown(
        server_uri: String,
        cooldown: Duration,
    ) -> (LocalJwtValidator, Arc<RemoteJwksProvider>) {
        let provider = Arc::new(RemoteJwksProvider::new_for_test_with_cooldown(
            server_uri, cooldown,
        ));
        let validator = LocalJwtValidator::new(
            vec!["my-api".into()],
            "http://localhost:8080/realms/test".into(),
            provider.clone(),
            multi_role_mapper(vec!["/groups".into()]),
        );
        (validator, provider)
    }

    #[tokio::test]
    async fn validate_signature_concurrent_unknown_kids_bounded() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_raw(jwks_body("test-key"), "application/json")
                    .insert_header("cache-control", "max-age=3600")
                    .set_delay(Duration::from_millis(300)),
            )
            .mount(&server)
            .await;

        let (validator, provider) =
            remote_validator_with_cooldown(server.uri(), Duration::from_millis(500));

        // Prime GET: populates the cache (fetched_at = now, TTL 3600s).
        provider.get_signing_keys().await.unwrap();

        // The cache is private to `jwks`, so backdating `fetched_at` directly
        // is impossible from this module; sleeping past the cooldown leaves
        // the cache TTL-fresh yet outside the forced-refresh interval. The
        // 500ms cooldown also dominates the 300ms response delay: a task
        // queued on the mutex during the forced fetch that stalls before its
        // post-lock re-check still sees `forced_start.elapsed()` < 500ms, so
        // the cooldown suppresses any further GET. The 750ms aging sleep must
        // exceed the 500ms cooldown so the single forced attempt is eligible.
        tokio::time::sleep(Duration::from_millis(750)).await;

        // 32 concurrent unknown-kid tokens; signatures are never reached on
        // kid-miss, so this models unknown-kid attack traffic exactly.
        let claims = claims_with_defaults();
        let tokens: Vec<String> = (1..=32)
            .map(|i| make_token(&format!("atk-{i}"), &claims))
            .collect();

        let validator = Arc::new(validator);
        let handles: Vec<_> = tokens
            .iter()
            .map(|token| {
                let validator = validator.clone();
                let token = token.clone();
                tokio::spawn(async move { validator.validate_signature(&token).await })
            })
            .collect();
        for (i, handle) in handles.into_iter().enumerate() {
            let result = handle.await.unwrap();
            assert!(
                matches!(result, Err(AuthError::TokenInvalid(_))),
                "kid atk-{} must be rejected as TokenInvalid, got {result:?}",
                i + 1
            );
        }

        let gets = server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .filter(|r| r.method.as_str() == "GET")
            .count();
        assert_eq!(
            gets, 2,
            "prime + exactly one forced fetch: 32 unknown kids must not amplify fetches"
        );
    }

    #[tokio::test]
    async fn rotated_key_recovery_after_cooldown() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_raw(jwks_body("test-key"), "application/json")
                    .insert_header("cache-control", "max-age=3600"),
            )
            // Same-matcher mocks are matched first-mounted-first in wiremock,
            // so the pre-rotation mock must retire after the prime GET and
            // the consumed forced attempt for the rotated mock to serve.
            .up_to_n_times(2)
            .mount(&server)
            .await;

        let (validator, provider) = remote_validator(server.uri());

        // Prime GET.
        provider.get_signing_keys().await.unwrap();

        // Sleep past the 100ms cooldown so a forced attempt is eligible.
        tokio::time::sleep(Duration::from_millis(150)).await;

        // Consume one forced attempt: unknown kid → forced fetch (GET #2),
        // kid still missing → TokenInvalid.
        let unknown = make_token("atk-1", &claims_with_defaults());
        assert!(matches!(
            validator.validate_signature(&unknown).await,
            Err(AuthError::TokenInvalid(_))
        ));
        assert_eq!(server.received_requests().await.unwrap().len(), 2);

        // Rotation: the endpoint now serves the rotated key set.
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_raw(jwks_body("rotated-key"), "application/json")
                    .insert_header("cache-control", "max-age=3600"),
            )
            .mount(&server)
            .await;

        // Cooldown elapsed since the first forced attempt.
        tokio::time::sleep(Duration::from_millis(150)).await;

        let rotated = make_token("rotated-key", &claims_with_defaults());
        let principal = validator
            .validate_signature(&rotated)
            .await
            .expect("rotated key must validate after the cooldown elapsed");
        assert_eq!(principal.subject, "user-123");

        let gets = server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .filter(|r| r.method.as_str() == "GET")
            .count();
        assert_eq!(gets, 3, "prime + consumed attempt + rotation fetch");
    }

    #[tokio::test]
    async fn extracts_generic_groups_roles() {
        // Test with generic /groups claim path
        let mapper = multi_role_mapper(vec!["/groups".into()]);
        let v = validator(vec!["my-api"], mapper);
        let now = chrono::Utc::now().timestamp() as u64;
        let claims = json!({
            "sub": "user-123",
            "iss": "http://localhost:8080/realms/test",
            "aud": "my-api",
            "exp": now + 3600,
            "iat": now,
            "groups": ["admin", "editor", "viewer"],
        });
        let token = make_token("test-key", &claims);
        let principal = v.validate(&token).await.unwrap();
        assert!(principal.has_role("admin"));
        assert!(principal.has_role("editor"));
        assert!(principal.has_role("viewer"));
    }
}
