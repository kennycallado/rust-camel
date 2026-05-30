use crate::jwks::Jwk;
use crate::native_issuer::NativeSigningKey;
use crate::types::AuthError;
use async_trait::async_trait;

pub struct NativeJwksProvider {
    jwk: Jwk,
}

impl std::fmt::Debug for NativeJwksProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NativeJwksProvider")
            .field("kid", &self.jwk.kid)
            .finish()
    }
}

impl NativeJwksProvider {
    pub fn new(signing_key: NativeSigningKey) -> Result<Self, AuthError> {
        let public_pem = signing_key.public_pem();
        let (n, e) = Self::pem_to_jwk_components(public_pem)?;
        Ok(Self {
            jwk: Jwk {
                kid: signing_key.kid().to_string(),
                kty: "RSA".to_string(),
                alg: Some("RS256".to_string()),
                r#use: Some("sig".to_string()),
                n,
                e,
            },
        })
    }

    fn pem_to_jwk_components(pem: &str) -> Result<(String, String), AuthError> {
        use base64::Engine as _;
        use base64::engine::general_purpose::URL_SAFE_NO_PAD;
        use rsa::pkcs1::DecodeRsaPublicKey;
        use rsa::traits::PublicKeyParts;

        let pub_key = rsa::RsaPublicKey::from_pkcs1_pem(pem)
            .map_err(|e| AuthError::ConfigError(format!("failed to parse public key PEM: {e}")))?;
        let n = URL_SAFE_NO_PAD.encode(pub_key.n().to_bytes_be());
        let e = URL_SAFE_NO_PAD.encode(pub_key.e().to_bytes_be());
        Ok((n, e))
    }
}

#[async_trait]
impl crate::jwks::JwksProvider for NativeJwksProvider {
    async fn get_signing_keys(&self) -> Result<Vec<Jwk>, AuthError> {
        Ok(vec![self.jwk.clone()])
    }

    async fn refresh(&self) -> Result<(), AuthError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jwks::JwksProvider;

    #[tokio::test]
    async fn native_jwks_returns_active_key() {
        let pem = include_str!("../tests/fixtures/test_rsa_private.pem");
        let signing_key = NativeSigningKey::from_pem(pem, "test-kid-1".to_string()).unwrap();
        let provider = NativeJwksProvider::new(signing_key).unwrap();
        let keys = provider.get_signing_keys().await.unwrap();
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].kid, "test-kid-1");
        assert_eq!(keys[0].kty, "RSA");
        assert_eq!(keys[0].alg.as_deref(), Some("RS256"));
        assert_eq!(keys[0].r#use.as_deref(), Some("sig"));
    }

    #[tokio::test]
    async fn native_jwks_refresh_is_noop() {
        let pem = include_str!("../tests/fixtures/test_rsa_private.pem");
        let signing_key = NativeSigningKey::from_pem(pem, "test-kid-2".to_string()).unwrap();
        let provider = NativeJwksProvider::new(signing_key).unwrap();
        provider.refresh().await.unwrap();
    }

    #[tokio::test]
    async fn native_jwks_validates_issued_token() {
        use crate::claims::{ClaimPaths, JsonPointerClaimsMapper};
        use crate::jwt::JwtValidator;
        use crate::native_client_store::{M2mClient, M2mClientSecret, M2mClientStore};
        use crate::native_issuer::NativeTokenIssuer;
        use std::sync::Arc;

        let pem = include_str!("../tests/fixtures/test_rsa_private.pem");
        let signing_key = NativeSigningKey::from_pem(pem, "kid-validate".to_string()).unwrap();
        let provider = Arc::new(NativeJwksProvider::new(signing_key).unwrap());

        let store = M2mClientStore::try_new(vec![M2mClient {
            client_id: "test-client".into(),
            secret: M2mClientSecret::Plaintext { value: "s".into() },
            roles: vec!["test".into()],
            scopes: vec!["read".into()],
        }])
        .unwrap();

        let signing_key_ref = NativeSigningKey::from_pem(pem, "kid-validate".to_string()).unwrap();
        let issuer = NativeTokenIssuer::try_new(
            "https://test.local".to_string(),
            vec!["api".to_string()],
            std::time::Duration::from_secs(300),
            signing_key_ref,
            store,
        )
        .unwrap();

        let token_resp = issuer
            .issue_token("test-client", "s", None, None)
            .await
            .unwrap();

        let claim_paths = ClaimPaths {
            subject: "/sub".to_string(),
            roles: vec!["/roles".to_string()],
            scopes: Some("/scope".to_string()),
        };
        let mapper = Arc::new(JsonPointerClaimsMapper::new(claim_paths));
        let validator = crate::jwt::LocalJwtValidator::new(
            vec!["api".to_string()],
            "https://test.local".to_string(),
            provider.clone(),
            mapper,
        );

        let principal = validator.validate(&token_resp.access_token).await.unwrap();
        assert_eq!(principal.subject, "test-client");
        assert_eq!(principal.roles, vec!["test"]);
    }
}
