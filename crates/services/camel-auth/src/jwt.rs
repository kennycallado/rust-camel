use async_trait::async_trait;
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header};
use std::sync::Arc;

use crate::claims::ClaimsMapper;
use crate::jwks::JwksProvider;
use crate::types::AuthError;
use camel_api::security_policy::Principal;

/// Validates JWT tokens and extracts a [`Principal`].
#[async_trait]
pub trait JwtValidator: Send + Sync {
    async fn validate(&self, token: &str) -> Result<Principal, AuthError>;
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

#[async_trait]
impl JwtValidator for LocalJwtValidator {
    async fn validate(&self, token: &str) -> Result<Principal, AuthError> {
        // Decode header to extract kid
        let header = decode_header(token)
            .map_err(|e| AuthError::TokenInvalid(format!("invalid JWT header: {e}")))?;

        let kid = header
            .kid
            .ok_or_else(|| AuthError::TokenInvalid("JWT missing kid".into()))?;

        // Fetch signing keys; on kid miss, force a JWKS refresh (handles key rotation)
        let keys = self.jwks.get_signing_keys().await?;
        let jwk = if let Some(k) = keys.iter().find(|k| k.kid == kid) {
            k.clone()
        } else {
            // Key not in cache — might be a newly rotated key; refresh once and retry
            self.jwks.refresh().await?;
            self.jwks
                .get_signing_keys()
                .await?
                .into_iter()
                .find(|k| k.kid == kid)
                .ok_or_else(|| {
                    AuthError::TokenInvalid(format!("no key for kid={kid} after refresh"))
                })?
        };

        let decoding_key = jwk_to_decoding_key(&jwk.n, &jwk.e)?;

        // Configure validation
        let mut validation = Validation::new(Algorithm::RS256);
        validation.set_audience(&self.audience);
        validation.set_issuer(&[&self.issuer]);

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
