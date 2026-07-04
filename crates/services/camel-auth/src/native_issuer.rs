use crate::native_client_store::M2mClientStore;
use crate::types::AuthError;
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use serde::Serialize;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use zeroize::Zeroizing;

#[derive(Debug, thiserror::Error)]
pub enum IssuerError {
    #[error("invalid_client")]
    InvalidClient,
    #[error("invalid_scope")]
    InvalidScope,
    #[error("invalid_audience")]
    InvalidAudience,
    #[error("unsupported_grant_type")]
    UnsupportedGrantType,
    #[error("{0}")]
    Other(String),
}

impl From<AuthError> for IssuerError {
    fn from(e: AuthError) -> Self {
        IssuerError::Other(e.to_string())
    }
}

pub struct NativeSigningKey {
    encoding_key: EncodingKey,
    kid: String,
    public_pem: String,
}

impl fmt::Debug for NativeSigningKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NativeSigningKey")
            .field("kid", &self.kid)
            .finish()
    }
}

impl NativeSigningKey {
    pub fn from_pem(private_pem: &str, kid: String) -> Result<Self, AuthError> {
        if private_pem.is_empty() {
            return Err(AuthError::ConfigError("signing key PEM is empty".into()));
        }
        let encoding_key = EncodingKey::from_rsa_pem(private_pem.as_bytes())
            .map_err(|e| AuthError::ConfigError(format!("invalid signing key PEM: {e}")))?;
        let public_pem = Self::extract_public_pem(private_pem)?;
        Ok(Self {
            encoding_key,
            kid,
            public_pem,
        })
    }

    fn extract_public_pem(private_pem: &str) -> Result<String, AuthError> {
        use pkcs1::der::Decode;
        use pkcs1::{LineEnding, RsaPrivateKey, RsaPublicKey};

        let (label, doc) = pkcs1::der::SecretDocument::from_pem(private_pem)
            .map_err(|e| AuthError::ConfigError(format!("failed to parse RSA private key: {e}")))?;
        let der_bytes = doc.as_bytes();

        let rsa_der: &[u8] = match label {
            "RSA PRIVATE KEY" => der_bytes,
            "PRIVATE KEY" => {
                let pki = <pkcs8::PrivateKeyInfo as Decode>::from_der(der_bytes).map_err(|e| {
                    AuthError::ConfigError(format!("failed to parse RSA private key: {e}"))
                })?;
                pki.private_key
            }
            other => {
                return Err(AuthError::ConfigError(format!(
                    "unsupported RSA key PEM label: {other}"
                )));
            }
        };

        let private_key = <RsaPrivateKey as Decode>::from_der(rsa_der)
            .map_err(|e| AuthError::ConfigError(format!("failed to parse RSA private key: {e}")))?;

        let public_key = RsaPublicKey {
            modulus: private_key.modulus,
            public_exponent: private_key.public_exponent,
        };
        let public_doc = <pkcs1::der::Document as TryFrom<&RsaPublicKey>>::try_from(&public_key)
            .map_err(|e| AuthError::ConfigError(format!("failed to encode public key: {e}")))?;
        public_doc
            .to_pem("RSA PUBLIC KEY", LineEnding::LF)
            .map_err(|e| AuthError::ConfigError(format!("failed to encode public key: {e}")))
    }

    pub fn kid(&self) -> &str {
        &self.kid
    }

    pub fn public_pem(&self) -> &str {
        &self.public_pem
    }

    pub(crate) fn encoding_key(&self) -> &EncodingKey {
        &self.encoding_key
    }
}

#[derive(Debug, Clone, Serialize)]
struct NativeTokenClaims {
    iss: String,
    sub: String,
    aud: serde_json::Value,
    iat: u64,
    exp: u64,
    jti: String,
    scope: String,
    roles: Vec<String>,
}

#[derive(Debug)]
#[non_exhaustive]
pub struct TokenResponse {
    pub access_token: Zeroizing<String>,
    pub token_type: String,
    pub expires_in: u64,
    pub scope: String,
}

pub struct NativeTokenIssuer {
    issuer: String,
    audience: Vec<String>,
    ttl: Duration,
    signing_key: NativeSigningKey,
    client_store: M2mClientStore,
    jti_counter: AtomicU64,
}

impl fmt::Debug for NativeTokenIssuer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NativeTokenIssuer")
            .field("issuer", &self.issuer)
            .field("audience", &self.audience)
            .field("ttl_secs", &self.ttl.as_secs())
            .field("signing_key", &self.signing_key)
            .finish()
    }
}

impl NativeTokenIssuer {
    pub fn try_new(
        issuer: String,
        audience: Vec<String>,
        ttl: Duration,
        signing_key: NativeSigningKey,
        client_store: M2mClientStore,
    ) -> Result<Self, AuthError> {
        if audience.is_empty() {
            return Err(AuthError::ConfigError(
                "native issuer requires at least one audience".into(),
            ));
        }
        if ttl.is_zero() {
            return Err(AuthError::ConfigError(
                "native issuer token_ttl_secs must be greater than 0".into(),
            ));
        }
        if ttl > Duration::from_secs(3600) {
            return Err(AuthError::ConfigError(format!(
                "native issuer token_ttl_secs {} exceeds maximum 3600",
                ttl.as_secs()
            )));
        }
        Ok(Self {
            issuer,
            audience,
            ttl,
            signing_key,
            client_store,
            jti_counter: AtomicU64::new(1),
        })
    }

    pub async fn issue_token(
        &self,
        client_id: &str,
        client_secret: &str,
        requested_scope: Option<&str>,
        requested_audience: Option<&str>,
    ) -> Result<TokenResponse, IssuerError> {
        let client = self
            .client_store
            .lookup(client_id, client_secret)
            .ok_or(IssuerError::InvalidClient)?;

        let granted_scopes = match requested_scope {
            Some(req) => {
                let requested: Vec<&str> = req.split_whitespace().collect();
                for s in &requested {
                    if !client.scopes.iter().any(|cs| cs == *s) {
                        return Err(IssuerError::InvalidScope);
                    }
                }
                requested.iter().map(|s| s.to_string()).collect::<Vec<_>>()
            }
            None => client.scopes.to_vec(),
        };

        let aud = match requested_audience {
            Some(req) => {
                if !self.audience.iter().any(|a| a == req) {
                    return Err(IssuerError::InvalidAudience);
                }
                serde_json::Value::String(req.to_string())
            }
            None => {
                if self.audience.len() == 1 {
                    serde_json::Value::String(self.audience[0].clone())
                } else {
                    serde_json::Value::Array(
                        self.audience
                            .iter()
                            .map(|a| serde_json::Value::String(a.clone()))
                            .collect(),
                    )
                }
            }
        };

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| IssuerError::Other(format!("system clock error: {e}")))?
            .as_secs();

        let jti = format!("{:016x}", self.jti_counter.fetch_add(1, Ordering::Relaxed));

        let claims = NativeTokenClaims {
            iss: self.issuer.clone(),
            sub: client.client_id.to_string(),
            aud,
            iat: now,
            exp: now + self.ttl.as_secs(),
            jti,
            scope: granted_scopes.join(" "),
            roles: client.roles.to_vec(),
        };

        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some(self.signing_key.kid().to_string());

        let token = encode(&header, &claims, self.signing_key.encoding_key())
            .map_err(|e| IssuerError::Other(format!("JWT encoding failed: {e}")))?;

        Ok(TokenResponse {
            access_token: Zeroizing::new(token),
            token_type: "Bearer".to_string(),
            expires_in: self.ttl.as_secs(),
            scope: claims.scope.clone(),
        })
    }

    pub async fn handle_token_request(&self, body: &str) -> Result<TokenResponse, IssuerError> {
        let params: std::collections::HashMap<String, String> = serde_urlencoded::from_str(body)
            .map_err(|e| IssuerError::Other(format!("invalid request body: {e}")))?;

        let grant_type = params.get("grant_type").map(|s| s.as_str()).unwrap_or("");
        if grant_type != "client_credentials" {
            return Err(IssuerError::UnsupportedGrantType);
        }

        let client_id = params.get("client_id").ok_or(IssuerError::InvalidClient)?;
        let client_secret = params
            .get("client_secret")
            .ok_or(IssuerError::InvalidClient)?;
        let scope = params.get("scope").map(|s| s.as_str());
        let audience = params
            .get("audience")
            .or_else(|| params.get("resource"))
            .map(|s| s.as_str());

        self.issue_token(client_id, client_secret, scope, audience)
            .await
    }

    pub fn signing_key(&self) -> &NativeSigningKey {
        &self.signing_key
    }

    pub fn issuer(&self) -> &str {
        &self.issuer
    }

    pub fn audience(&self) -> &[String] {
        &self.audience
    }

    pub fn ttl(&self) -> Duration {
        self.ttl
    }

    pub fn client_store(&self) -> &M2mClientStore {
        &self.client_store
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native_client_store::{M2mClient, M2mClientSecret, M2mClientStore};
    use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode};
    use serde_json::json;

    fn test_store() -> M2mClientStore {
        M2mClientStore::try_new(vec![M2mClient {
            client_id: "billing".into(),
            secret: M2mClientSecret::Plaintext {
                value: Zeroizing::new("secret".into()),
            },
            roles: vec!["billing".into()],
            scopes: vec!["orders:read".into(), "orders:write".into()],
        }])
        .unwrap()
    }

    fn test_issuer() -> NativeTokenIssuer {
        let pem = include_str!("../tests/fixtures/test_rsa_private.pem");
        let signing_key = NativeSigningKey::from_pem(pem, "test-kid".to_string()).unwrap();
        let store = test_store();
        NativeTokenIssuer::try_new(
            "https://orders.local".to_string(),
            vec!["orders-api".to_string()],
            std::time::Duration::from_secs(900),
            signing_key,
            store,
        )
        .unwrap()
    }

    #[test]
    fn signing_key_loads_pem() {
        let pem = include_str!("../tests/fixtures/test_rsa_private.pem");
        let key = NativeSigningKey::from_pem(pem, "test-key-1".to_string()).unwrap();
        assert_eq!(key.kid(), "test-key-1");
    }

    #[test]
    fn signing_key_loads_pkcs1_pem() {
        let pem = include_str!("../tests/fixtures/test_rsa_private_pkcs1.pem");
        let key = NativeSigningKey::from_pem(pem, "pkcs1-kid".to_string()).unwrap();
        assert_eq!(key.kid(), "pkcs1-kid");
        assert!(
            key.public_pem()
                .starts_with("-----BEGIN RSA PUBLIC KEY-----"),
            "expected PKCS#1 public key PEM, got: {}",
            key.public_pem()
        );
    }

    #[test]
    fn public_pem_is_pkcs1_rsa_public_key() {
        let pem = include_str!("../tests/fixtures/test_rsa_private.pem");
        let key = NativeSigningKey::from_pem(pem, "pkcs1-kid".to_string()).unwrap();
        let public_pem = key.public_pem();
        assert!(!public_pem.trim().is_empty());
        assert!(
            public_pem.starts_with("-----BEGIN RSA PUBLIC KEY-----"),
            "expected PKCS#1 public key PEM, got: {public_pem}"
        );
        assert!(public_pem.contains("-----END RSA PUBLIC KEY-----"));
    }

    #[test]
    fn signing_key_rejects_empty_pem() {
        let result = NativeSigningKey::from_pem("", "key-1".to_string());
        assert!(result.is_err());
    }

    #[test]
    fn issuer_try_new_rejects_ttl_above_3600() {
        let pem = include_str!("../tests/fixtures/test_rsa_private.pem");
        let signing_key = NativeSigningKey::from_pem(pem, "k".to_string()).unwrap();
        let store = M2mClientStore::try_new(vec![]).unwrap();
        let result = NativeTokenIssuer::try_new(
            "https://test.local".into(),
            vec!["orders-api".into()],
            std::time::Duration::from_secs(4000),
            signing_key,
            store,
        );
        let msg = format!("{}", result.unwrap_err());
        assert!(msg.contains("3600"));
    }

    #[test]
    fn issuer_try_new_accepts_ttl_at_3600() {
        let pem = include_str!("../tests/fixtures/test_rsa_private.pem");
        let signing_key = NativeSigningKey::from_pem(pem, "k".to_string()).unwrap();
        let store = M2mClientStore::try_new(vec![]).unwrap();
        let result = NativeTokenIssuer::try_new(
            "https://test.local".into(),
            vec!["orders-api".into()],
            std::time::Duration::from_secs(3600),
            signing_key,
            store,
        );
        assert!(result.is_ok());
    }

    #[test]
    fn issuer_try_new_rejects_empty_audience() {
        let pem = include_str!("../tests/fixtures/test_rsa_private.pem");
        let signing_key = NativeSigningKey::from_pem(pem, "k".to_string()).unwrap();
        let store = M2mClientStore::try_new(vec![]).unwrap();
        let result = NativeTokenIssuer::try_new(
            "https://test.local".into(),
            vec![],
            std::time::Duration::from_secs(900),
            signing_key,
            store,
        );
        let msg = format!("{}", result.unwrap_err());
        assert!(msg.contains("audience"));
    }

    #[test]
    fn issuer_try_new_rejects_zero_ttl() {
        let pem = include_str!("../tests/fixtures/test_rsa_private.pem");
        let signing_key = NativeSigningKey::from_pem(pem, "k".to_string()).unwrap();
        let store = M2mClientStore::try_new(vec![]).unwrap();
        let result = NativeTokenIssuer::try_new(
            "https://test.local".into(),
            vec!["api".into()],
            Duration::ZERO,
            signing_key,
            store,
        );
        let msg = format!("{}", result.unwrap_err());
        assert!(msg.contains("greater than 0"));
    }

    #[tokio::test]
    async fn issuer_issues_valid_jwt() {
        let issuer = test_issuer();
        let response = issuer
            .issue_token("billing", "secret", None, None)
            .await
            .unwrap();
        assert_eq!(response.token_type, "Bearer");
        assert_eq!(response.expires_in, 900);
        assert!(!response.access_token.is_empty());

        let pub_pem = include_str!("../tests/fixtures/test_rsa_public.pem");
        let mut validation = Validation::new(Algorithm::RS256);
        validation.set_issuer(&["https://orders.local"]);
        validation.set_audience(&["orders-api"]);
        let decoded = decode::<serde_json::Value>(
            response.access_token.as_str(),
            &DecodingKey::from_rsa_pem(pub_pem.as_bytes()).unwrap(),
            &validation,
        )
        .unwrap();
        let claims = decoded.claims;
        assert_eq!(claims["sub"], "billing");
        assert_eq!(claims["scope"], "orders:read orders:write");
        assert_eq!(claims["roles"], json!(["billing"]));
        assert!(claims["jti"].is_string());
    }

    #[tokio::test]
    async fn issuer_narrows_scopes() {
        let issuer = test_issuer();
        let response = issuer
            .issue_token("billing", "secret", Some("orders:read"), None)
            .await
            .unwrap();
        let pub_pem = include_str!("../tests/fixtures/test_rsa_public.pem");
        let mut validation = Validation::new(Algorithm::RS256);
        validation.set_issuer(&["https://orders.local"]);
        validation.set_audience(&["orders-api"]);
        let decoded = decode::<serde_json::Value>(
            response.access_token.as_str(),
            &DecodingKey::from_rsa_pem(pub_pem.as_bytes()).unwrap(),
            &validation,
        )
        .unwrap();
        assert_eq!(decoded.claims["scope"], "orders:read");
    }

    #[tokio::test]
    async fn issuer_rejects_scope_escalation() {
        let issuer = test_issuer();
        let result = issuer
            .issue_token("billing", "secret", Some("admin:super"), None)
            .await;
        assert!(result.is_err());
        let msg = format!("{}", result.unwrap_err());
        assert!(msg.contains("invalid_scope") || msg.contains("scope"));
    }

    #[tokio::test]
    async fn issuer_rejects_bad_credentials() {
        let issuer = test_issuer();
        let result = issuer
            .issue_token("billing", "wrong-secret", None, None)
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn issuer_rejects_unknown_client() {
        let issuer = test_issuer();
        let result = issuer.issue_token("unknown", "secret", None, None).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn issuer_constrains_requested_audience() {
        let pem = include_str!("../tests/fixtures/test_rsa_private.pem");
        let signing_key = NativeSigningKey::from_pem(pem, "test-kid".to_string()).unwrap();
        let store = test_store();
        let issuer = NativeTokenIssuer::try_new(
            "https://orders.local".to_string(),
            vec!["orders-api".to_string(), "internal-api".to_string()],
            std::time::Duration::from_secs(900),
            signing_key,
            store,
        )
        .unwrap();

        let response = issuer
            .issue_token("billing", "secret", None, Some("orders-api"))
            .await
            .unwrap();
        let pub_pem = include_str!("../tests/fixtures/test_rsa_public.pem");
        let mut validation = Validation::new(Algorithm::RS256);
        validation.set_issuer(&["https://orders.local"]);
        validation.set_audience(&["orders-api"]);
        let decoded = decode::<serde_json::Value>(
            response.access_token.as_str(),
            &DecodingKey::from_rsa_pem(pub_pem.as_bytes()).unwrap(),
            &validation,
        )
        .unwrap();
        assert_eq!(decoded.claims["aud"], json!("orders-api"));
    }

    #[tokio::test]
    async fn issuer_rejects_invalid_audience() {
        let issuer = test_issuer();
        let result = issuer
            .issue_token("billing", "secret", None, Some("evil-api"))
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn handle_token_request_valid_client_credentials() {
        let issuer = test_issuer();
        let body = "grant_type=client_credentials&client_id=billing&client_secret=secret";
        let response = issuer.handle_token_request(body).await.unwrap();
        assert_eq!(response.token_type, "Bearer");
        assert_eq!(response.expires_in, 900);
    }

    #[tokio::test]
    async fn handle_token_request_with_scope() {
        let issuer = test_issuer();
        let body = "grant_type=client_credentials&client_id=billing&client_secret=secret&scope=orders%3Aread";
        let response = issuer.handle_token_request(body).await.unwrap();
        assert_eq!(response.scope, "orders:read");
    }

    #[tokio::test]
    async fn handle_token_request_unsupported_grant_type() {
        let issuer = test_issuer();
        let body = "grant_type=authorization_code&client_id=billing&client_secret=secret";
        let result = issuer.handle_token_request(body).await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            IssuerError::UnsupportedGrantType
        ));
    }

    #[tokio::test]
    async fn handle_token_request_invalid_client() {
        let issuer = test_issuer();
        let body = "grant_type=client_credentials&client_id=evil&client_secret=guess";
        let result = issuer.handle_token_request(body).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), IssuerError::InvalidClient));
    }

    #[tokio::test]
    async fn handle_token_request_invalid_scope() {
        let issuer = test_issuer();
        let body = "grant_type=client_credentials&client_id=billing&client_secret=secret&scope=admin%3Asuper";
        let result = issuer.handle_token_request(body).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), IssuerError::InvalidScope));
    }

    #[tokio::test]
    async fn handle_token_request_error_does_not_leak_secret() {
        let issuer = test_issuer();
        let body =
            "grant_type=client_credentials&client_id=billing&client_secret=super-secret-value";
        let result = issuer.handle_token_request(body).await;
        assert!(result.is_err());
        let err_msg = format!("{}", result.unwrap_err());
        assert!(!err_msg.contains("super-secret-value"));
    }

    #[tokio::test]
    async fn handle_token_request_resource_alias_for_audience() {
        let issuer = test_issuer();
        let body = "grant_type=client_credentials&client_id=billing&client_secret=secret&resource=orders-api";
        let response = issuer.handle_token_request(body).await.unwrap();
        assert_eq!(response.token_type, "Bearer");
    }

    #[tokio::test]
    async fn handle_token_request_resource_alias_rejects_invalid() {
        let issuer = test_issuer();
        let body = "grant_type=client_credentials&client_id=billing&client_secret=secret&resource=evil-api";
        let result = issuer.handle_token_request(body).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), IssuerError::InvalidAudience));
    }

    #[test]
    fn issuer_from_config_builds_valid_issuer() {
        // SAFETY: test-only, single-threaded, unique env var names
        unsafe {
            std::env::set_var(
                "TEST_ISSUER_KEY_PEM_WIRING",
                include_str!("../tests/fixtures/test_rsa_private.pem"),
            );
            std::env::set_var("TEST_M2M_CLIENT_SECRET_WIRING", "test-secret");
        }

        let signing_key_pem = std::env::var("TEST_ISSUER_KEY_PEM_WIRING").unwrap();
        let signing_key =
            NativeSigningKey::from_pem(&signing_key_pem, "config-kid".to_string()).unwrap();

        let store = M2mClientStore::try_new(vec![M2mClient {
            client_id: "worker".into(),
            secret: M2mClientSecret::Env {
                name: "TEST_M2M_CLIENT_SECRET_WIRING".into(),
            },
            roles: vec!["worker".into()],
            scopes: vec!["api:read".into()],
        }])
        .unwrap();

        let issuer = NativeTokenIssuer::try_new(
            "https://config.local".into(),
            vec!["api".into()],
            Duration::from_secs(600),
            signing_key,
            store,
        )
        .unwrap();

        assert_eq!(issuer.issuer(), "https://config.local");
        assert_eq!(issuer.ttl(), Duration::from_secs(600));

        // SAFETY: test-only cleanup
        unsafe {
            std::env::remove_var("TEST_ISSUER_KEY_PEM_WIRING");
            std::env::remove_var("TEST_M2M_CLIENT_SECRET_WIRING");
        }
    }
}
