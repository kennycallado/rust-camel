//! Keycloak UMA (User-Managed Access) permission evaluator.
//!
//! Implements [`PermissionEvaluator`] using Keycloak's UMA ticket flow.
//! Obtains a client-credentials token, then POSTs a permission request
//! to the Keycloak token endpoint with `grant_type=urn:ietf:params:oauth:grant-type:uma-ticket`.

use std::sync::Arc;

use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use camel_api::SsrfPolicy;
use camel_auth::http_client::{SsrfClientOptions, build_ssrf_pinned_client};
use camel_auth::oauth2::{ClientCredentialsProvider, TokenProvider};
use camel_auth::permission::{PermissionDecision, PermissionEvaluator, PermissionRequest};
use camel_auth::types::AuthError;

/// Keycloak UMA permission evaluator.
///
/// Uses the client-credentials grant to obtain a service-account token,
/// then calls Keycloak's UMA authorization endpoint to check whether
/// a principal may perform an action on a resource.
pub struct KeycloakUmaEvaluator {
    realm_url: String,
    client_id: String,
    http: reqwest::Client,
    token_provider: Arc<ClientCredentialsProvider>,
}

impl std::fmt::Debug for KeycloakUmaEvaluator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KeycloakUmaEvaluator")
            .field("realm_url", &self.realm_url)
            .field("client_id", &self.client_id)
            .finish_non_exhaustive()
    }
}

impl KeycloakUmaEvaluator {
    /// Production constructor with SSRF validation.
    ///
    /// Validates that the derived token endpoint URI is a public HTTPS endpoint
    /// before constructing the internal [`ClientCredentialsProvider`].
    pub async fn new(
        server_url: String,
        realm: String,
        client_id: String,
        client_secret: String,
        policy: SsrfPolicy,
    ) -> Result<Self, AuthError> {
        let server_url_trimmed = server_url.trim_end_matches('/');
        let realm_url = format!("{}/realms/{}", server_url_trimmed, realm);
        let token_endpoint = format!("{}/protocol/openid-connect/token", realm_url); // allow-secret

        let token_provider = ClientCredentialsProvider::new(
            token_endpoint.clone(),
            client_id.clone(),
            client_secret,
            None,
            None,
            policy,
        )
        .await?;
        // DNS-pin the permission client to the same host as the token endpoint,
        // closing the TOCTOU window between validation and the first request.
        let http = build_ssrf_pinned_client(
            &token_endpoint,
            "UMA permission",
            &SsrfClientOptions::new(policy)
                .with_connect_timeout(std::time::Duration::from_secs(5))
                .with_request_timeout(std::time::Duration::from_secs(30)),
        )
        .await?;
        Ok(Self {
            realm_url,
            client_id,
            http,
            token_provider: Arc::new(token_provider),
        })
    }

    /// Test-only constructor that bypasses SSRF validation and accepts a pre-built HTTP client.
    ///
    /// Intended for use with `wiremock` or other test harnesses.
    /// Do NOT use in production code.
    #[doc(hidden)]
    pub fn new_unchecked_for_test(
        server_url: &str,
        realm: &str,
        client_id: &str,
        client_secret: &str,
        http: reqwest::Client,
    ) -> Self {
        let server_url_trimmed = server_url.trim_end_matches('/');
        let realm_url = format!("{}/realms/{}", server_url_trimmed, realm);
        let token_endpoint = format!("{}/protocol/openid-connect/token", realm_url); // allow-secret
        let token_provider = ClientCredentialsProvider::new_unchecked_for_test(
            token_endpoint,
            client_id.to_string(),
            client_secret.to_string(),
            None,
            None,
            http.clone(),
        );
        Self {
            realm_url,
            client_id: client_id.to_string(),
            http,
            token_provider: Arc::new(token_provider),
        }
    }

    /// The UMA permission endpoint — same as the realm's OpenID Connect token endpoint.
    fn permission_endpoint(&self) -> String {
        format!("{}/protocol/openid-connect/token", self.realm_url) // allow-secret
    }

    /// Build the Keycloak permission string from resource, action, and optional scopes.
    fn build_permission_string(resource: &str, action: &str, scopes: &[String]) -> String {
        if scopes.is_empty() {
            format!("{}#{}", resource, action)
        } else {
            let scopes_joined = scopes.join(",");
            format!("{}#{}#{}", resource, action, scopes_joined) // allow-secret
        }
    }
}

#[async_trait]
impl PermissionEvaluator for KeycloakUmaEvaluator {
    async fn evaluate(&self, request: PermissionRequest) -> Result<PermissionDecision, AuthError> {
        // 1. Obtain client-credentials token (no HTTP param — provider owns its client)
        let access_token = self.token_provider.get_token().await?;

        // 2. Build permission string
        let permission_str = Self::build_permission_string(
            &request.resource,
            &request.action,
            &request.requested_scopes,
        );

        // 3. Build claim_token from principal claims so Keycloak evaluates
        //    the requesting user's identity, not just the service account.
        let claim_json = serde_json::to_string(&request.principal.claims).map_err(|e| {
            AuthError::ProviderUnavailable(format!("failed to serialize principal claims: {e}"))
        })?;
        let claim_token = BASE64.encode(claim_json);

        // 4. POST to UMA permission endpoint
        let response = self
            .http
            .post(self.permission_endpoint())
            .header("Authorization", format!("Bearer {}", access_token)) // allow-secret
            .form(&[
                ("grant_type", "urn:ietf:params:oauth:grant-type:uma-ticket"),
                ("audience", &self.client_id),
                ("permission", &permission_str),
                ("claim_token", &claim_token),
                ("claim_token_format", "urn:ietf:params:oauth:token-type:jwt"),
            ])
            .send()
            .await
            .map_err(|e| {
                AuthError::ProviderUnavailable(format!("UMA permission request failed: {e}"))
            })?;

        // 4. Handle response
        let status = response.status();
        match status.as_u16() {
            200 => Ok(PermissionDecision::Granted),
            403 => {
                let body = response.text().await.unwrap_or_default();
                let parsed: serde_json::Value = serde_json::from_str(&body)
                    .unwrap_or_else(|_| serde_json::json!({"error_description": "access denied"}));
                let description = parsed
                    .get("error_description")
                    .and_then(|v| v.as_str())
                    .unwrap_or("access denied");
                let reason = description.to_string();
                Ok(PermissionDecision::Denied { reason })
            }
            401 => Err(AuthError::ProviderUnavailable(
                "client credentials rejected".into(),
            )),
            _ => Err(AuthError::ProviderUnavailable(format!(
                "UMA endpoint returned {}",
                status
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_api::security_policy::Principal;
    use serde_json::json;

    #[allow(dead_code)]
    fn test_principal() -> Principal {
        Principal {
            subject: "alice".into(),
            issuer: "https://kc.example.com/realms/test".into(),
            audience: vec!["camel-api".into()],
            roles: vec!["admin".into()],
            scopes: vec!["read".into()],
            claims: json!({}),
        }
    }

    #[test]
    fn build_permission_string_without_scopes() {
        let result = KeycloakUmaEvaluator::build_permission_string("/orders", "read", &[]);
        assert_eq!(result, "/orders#read");
    }

    #[test]
    fn build_permission_string_with_scopes() {
        let result = KeycloakUmaEvaluator::build_permission_string(
            "/orders",
            "read",
            &["scope1".to_string(), "scope2".to_string()],
        );
        assert_eq!(result, "/orders#read#scope1,scope2");
    }

    #[test]
    fn build_permission_string_with_single_scope() {
        let result = KeycloakUmaEvaluator::build_permission_string(
            "/data",
            "write",
            &["exclusive".to_string()],
        );
        assert_eq!(result, "/data#write#exclusive");
    }

    #[tokio::test]
    async fn new_rejects_non_https() {
        let result = KeycloakUmaEvaluator::new(
            "http://localhost:8080".into(),
            "test".into(),
            "client".into(),
            "secret".into(),
            SsrfPolicy::PublicHttpsOnly,
        )
        .await;
        assert!(result.is_err(), "should reject non-HTTPS server URL");
    }

    #[test]
    fn new_unchecked_for_test_builds_successfully() {
        let evaluator = KeycloakUmaEvaluator::new_unchecked_for_test(
            "http://localhost:8080",
            "test-realm",
            "test-client",
            "test-secret",
            crate::hardened_http_client().unwrap(), // allow-unwrap
        );
        assert_eq!(evaluator.client_id, "test-client");
        assert_eq!(
            evaluator.realm_url,
            "http://localhost:8080/realms/test-realm"
        );
    }

    #[test]
    fn permission_endpoint_derives_correctly() {
        let evaluator = KeycloakUmaEvaluator::new_unchecked_for_test(
            "https://kc.example.com",
            "myrealm",
            "myclient",
            "secret",
            crate::hardened_http_client().unwrap(), // allow-unwrap
        );
        assert_eq!(
            evaluator.permission_endpoint(),
            "https://kc.example.com/realms/myrealm/protocol/openid-connect/token"
        );
    }

    #[test]
    fn trailing_slash_stripped_in_realm_url() {
        let evaluator = KeycloakUmaEvaluator::new_unchecked_for_test(
            "https://kc.example.com/",
            "myrealm",
            "myclient",
            "secret",
            crate::hardened_http_client().unwrap(), // allow-unwrap
        );
        assert_eq!(evaluator.realm_url, "https://kc.example.com/realms/myrealm");
    }

    #[test]
    fn debug_hides_secrets() {
        let evaluator = KeycloakUmaEvaluator::new_unchecked_for_test(
            "https://kc.example.com",
            "myrealm",
            "myclient",
            "super-secret-value",
            crate::hardened_http_client().unwrap(), // allow-unwrap
        );
        let debug_str = format!("{evaluator:?}");
        assert!(!debug_str.contains("super-secret-value"));
        assert!(debug_str.contains("KeycloakUmaEvaluator"));
    }

    #[test]
    fn claim_token_encodes_principal_claims() {
        let claims = json!({
            "sub": "alice",
            "email": "alice@example.com",
            "roles": ["admin", "user"]
        });
        let encoded = BASE64.encode(serde_json::to_string(&claims).unwrap());
        // Verify it's valid base64 and decodes back to the original JSON
        let decoded_bytes = BASE64.decode(&encoded).unwrap();
        let decoded: serde_json::Value = serde_json::from_slice(&decoded_bytes).unwrap();
        assert_eq!(decoded, claims);
        // Verify the encoded string appears as a valid form value (no newlines in standard base64)
        assert!(!encoded.contains('\n'));
    }

    #[test]
    fn claim_token_encodes_null_claims_gracefully() {
        let claims = serde_json::Value::Null;
        let encoded = BASE64.encode(serde_json::to_string(&claims).unwrap());
        let decoded_bytes = BASE64.decode(&encoded).unwrap();
        let decoded: serde_json::Value = serde_json::from_slice(&decoded_bytes).unwrap();
        assert_eq!(decoded, serde_json::Value::Null);
    }
}
