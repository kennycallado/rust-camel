//! Keycloak integration for rust-camel.
//!
//! Provides Keycloak-specific claim presets, realm URL construction,
//! role extraction, and Admin API producer (Phase 7).
//! Event consumer will be added in Phase 8.

pub mod admin_endpoint_config;
pub mod admin_operation;
pub mod admin_types;
pub mod event_types;
pub mod events_endpoint_config;
pub mod keycloak_consumer;
pub mod keycloak_endpoint;
pub mod keycloak_producer;
pub mod uma;

use std::fmt;
use std::net::ToSocketAddrs;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use camel_api::{CamelError, is_ssrf_blocked_ip};
use camel_auth::claims::ClaimPaths;
use camel_auth::oauth2::{ClientCredentialsProvider, TokenProvider};
use camel_auth::permission::PermissionEvaluator;
use camel_auth::types::AuthError;
use camel_component_api::{Component, ComponentContext, Endpoint};
use serde::{Deserialize, Serialize};

#[derive(Clone, Deserialize, Serialize)]
pub struct KeycloakRealmConfig {
    server_url: String,
    realm: String,
    client_id: String,
    #[serde(skip_serializing)]
    client_secret: Option<String>,
}

impl fmt::Debug for KeycloakRealmConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let secret_display = if self.client_secret.is_some() {
            "REDACTED"
        } else {
            "None"
        };
        f.debug_struct("KeycloakRealmConfig")
            .field("server_url", &self.server_url)
            .field("realm", &self.realm)
            .field("client_id", &self.client_id)
            .field("client_secret", &secret_display)
            .finish()
    }
}

fn escape_json_pointer(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '~' => out.push_str("~0"),
            '/' => out.push_str("~1"),
            _ => out.push(c),
        }
    }
    out
}

impl KeycloakRealmConfig {
    pub fn new(server_url: String, realm: String, client_id: String) -> Self {
        Self {
            server_url,
            realm,
            client_id,
            client_secret: None,
        }
    }

    pub fn with_client_secret(mut self, secret: String) -> Self {
        self.client_secret = Some(secret);
        self
    }

    pub fn realm_url(&self) -> String {
        let url = format!(
            "{}/realms/{}",
            self.server_url.trim_end_matches('/'),
            self.realm
        ); // allow-secret
        url
    }

    pub fn jwks_uri(&self) -> String {
        format!("{}/protocol/openid-connect/certs", self.realm_url()) // allow-secret
    }

    pub fn token_endpoint(&self) -> String {
        format!("{}/protocol/openid-connect/token", self.realm_url()) // allow-secret
    }

    pub fn introspection_endpoint(&self) -> String {
        self.realm_url() + "/protocol/openid-connect/token/introspect"
    }

    pub fn admin_url(&self) -> String {
        format!(
            "{}/admin/realms/{}",
            self.server_url.trim_end_matches('/'),
            self.realm
        )
    }

    pub fn server_url(&self) -> &str {
        &self.server_url
    }

    pub fn realm(&self) -> &str {
        &self.realm
    }

    pub fn client_id(&self) -> &str {
        &self.client_id
    }

    pub fn client_secret(&self) -> Option<&str> {
        self.client_secret.as_deref()
    }

    pub async fn introspection_authenticator(
        &self,
        options: camel_auth::IntrospectionCacheOptions,
    ) -> Result<camel_auth::IntrospectionAuthenticator, CamelError> {
        use camel_auth::IntrospectionAuthenticator;
        use camel_auth::claims::{ClaimsMapper, JsonPointerClaimsMapper};

        let secret = self
            .client_secret()
            .ok_or_else(|| CamelError::Config("introspection requires client_secret".into()))?;
        let introspector = camel_auth::CachingTokenIntrospector::new(
            self.introspection_endpoint(),
            self.client_id().to_string(),
            secret.to_string(),
            options,
        )
        .await
        .map_err(|e| CamelError::Config(e.to_string()))?;
        let mapper: Arc<dyn ClaimsMapper> = Arc::new(JsonPointerClaimsMapper::new(
            keycloak_claim_paths(self.client_id()),
        ));
        Ok(IntrospectionAuthenticator::new(
            Arc::new(introspector),
            mapper,
        ))
    }

    pub async fn uma_evaluator(&self) -> Result<Arc<dyn PermissionEvaluator>, AuthError> {
        let secret = self
            .client_secret
            .as_ref()
            .ok_or_else(|| AuthError::ConfigError("client_secret required for UMA".into()))?;
        let evaluator = KeycloakUmaEvaluator::new(
            self.server_url.clone(),
            self.realm.clone(),
            self.client_id.clone(),
            secret.clone(),
        )
        .await?;
        Ok(Arc::new(evaluator))
    }
}

impl fmt::Display for KeycloakRealmConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "KeycloakRealmConfig(server={}, realm={}, client={})",
            self.server_url, self.realm, self.client_id
        )
    }
}

pub fn keycloak_claim_paths(client_id: &str) -> ClaimPaths {
    ClaimPaths {
        subject: "/sub".into(),
        roles: vec![
            "/realm_access/roles".into(),
            format!("/resource_access/{}/roles", escape_json_pointer(client_id)),
        ],
        scopes: Some("/scope".into()),
    }
}

pub use admin_endpoint_config::AdminEndpointConfig;
pub use admin_operation::AdminOperation;
pub use events_endpoint_config::EventsEndpointConfig;
pub use keycloak_consumer::KeycloakEventConsumer;
pub use keycloak_endpoint::{KeycloakEndpoint, KeycloakEndpointConfig, KeycloakEndpointKind};
pub use keycloak_producer::KeycloakAdminProducer;
pub use uma::KeycloakUmaEvaluator;

/// Build a hardened `reqwest::Client` for Keycloak HTTP traffic.
///
/// Hardening (H15):
/// - **No redirects** — a 302/303 to an attacker-controlled host could
///   bypass SSRF guards. Keycloak's own API is non-redirecting; a redirect
///   response is a signal of misconfiguration or attack.
/// - **Connect timeout 10s** — bound TCP handshake.
/// - **Request timeout 30s** — bound total request lifetime.
pub fn hardened_http_client() -> Result<reqwest::Client, CamelError> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| CamelError::EndpointCreationFailed(format!("HTTP client build failed: {e}")))
}

/// Validate a Keycloak server URL.
///
/// Rejects non-http(s) schemes, and (when `allow_internal` is `false`)
/// rejects hosts that resolve to any address matched by
/// [`camel_api::is_ssrf_blocked_ip`].
///
/// Set `allow_internal` to `true` only for local development against a
/// Keycloak instance bound to 127.0.0.1 / 0.0.0.0; production must keep
/// it at the default `false`.
pub fn validate_server_url(url: &str, allow_internal: bool) -> Result<(), CamelError> {
    let parsed = url::Url::parse(url)
        .map_err(|e| CamelError::EndpointCreationFailed(format!("invalid keycloak URL: {e}")))?;

    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(CamelError::EndpointCreationFailed(format!(
            "keycloak URL must use http/https, got: {}",
            parsed.scheme()
        )));
    }

    if allow_internal {
        return Ok(());
    }

    let Some(host) = parsed.host_str() else {
        return Err(CamelError::EndpointCreationFailed(format!(
            "keycloak URL '{url}' is missing a host"
        )));
    };

    let port = parsed.port_or_known_default().unwrap_or(0);
    let resolved = (host, port).to_socket_addrs().map_err(|e| {
        CamelError::EndpointCreationFailed(format!("failed to resolve keycloak host '{host}': {e}"))
    })?;

    for addr in resolved {
        let ip = addr.ip();
        if is_ssrf_blocked_ip(&ip) {
            return Err(CamelError::EndpointCreationFailed(format!(
                "keycloak URL resolves to blocked SSRF address: {ip}"
            )));
        }
    }
    Ok(())
}

pub struct KeycloakComponent {
    server_url: String,
    token_provider: Arc<dyn TokenProvider>,
    http: reqwest::Client,
}

impl KeycloakComponent {
    pub async fn new(config: &KeycloakRealmConfig) -> Result<Self, CamelError> {
        let secret = config.client_secret().ok_or_else(|| {
            CamelError::EndpointCreationFailed("keycloak component requires client_secret".into())
        })?;
        let token_provider = Arc::new(
            ClientCredentialsProvider::new(
                config.token_endpoint(),
                config.client_id().to_string(),
                secret.to_string(),
                None,
                None,
            )
            .await
            .map_err(|_| CamelError::EndpointCreationFailed("token provider init failed".into()))?,
        );
        Ok(Self {
            server_url: config.server_url().to_string(),
            token_provider,
            http: hardened_http_client()?,
        })
    }

    /// Test-only constructor that bypasses SSRF/HTTPS validation.
    ///
    /// Accepts a pre-built HTTP client for use with `wiremock` or other
    /// test harnesses. Do NOT use in production code.
    #[cfg(test)]
    pub fn new_for_test(config: &KeycloakRealmConfig) -> Result<Self, CamelError> {
        let secret = config.client_secret().ok_or_else(|| {
            CamelError::EndpointCreationFailed("keycloak component requires client_secret".into())
        })?;
        let token_provider = Arc::new(ClientCredentialsProvider::new_unchecked_for_test(
            config.token_endpoint(),
            config.client_id().to_string(),
            secret.to_string(),
            None,
            None,
            reqwest::Client::new(),
        ));
        Ok(Self {
            server_url: config.server_url().to_string(),
            token_provider,
            http: crate::hardened_http_client()?,
        })
    }
}

#[async_trait]
impl Component for KeycloakComponent {
    fn scheme(&self) -> &str {
        "keycloak"
    }

    fn create_endpoint(
        &self,
        uri: &str,
        _ctx: &dyn ComponentContext,
    ) -> Result<Box<dyn Endpoint>, CamelError> {
        let config = KeycloakEndpointConfig::from_uri(
            uri,
            &self.server_url,
            Arc::clone(&self.token_provider),
            self.http.clone(),
        )?;
        Ok(Box::new(KeycloakEndpoint::new(uri.to_string(), config)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_auth::IntrospectionCacheOptions;
    use camel_auth::claims::{ClaimsMapper, JsonPointerClaimsMapper};
    use camel_component_api::NoOpComponentContext;
    use serde_json::json;

    #[test]
    fn keycloak_claim_paths_contains_both_role_paths() {
        let paths = keycloak_claim_paths("my-client");
        assert_eq!(paths.subject, "/sub");
        assert!(paths.roles.contains(&"/realm_access/roles".into()));
        assert!(
            paths
                .roles
                .contains(&"/resource_access/my-client/roles".into())
        );
        assert_eq!(paths.scopes, Some("/scope".into()));
    }

    #[test]
    fn claim_paths_escapes_client_id() {
        let paths = keycloak_claim_paths("my/client");
        assert!(
            paths
                .roles
                .iter()
                .any(|p| p == "/resource_access/my~1client/roles")
        );
    }

    #[test]
    fn claim_paths_escapes_tilde_in_client_id() {
        let paths = keycloak_claim_paths("my~client");
        assert!(
            paths
                .roles
                .iter()
                .any(|p| p == "/resource_access/my~0client/roles")
        );
    }

    #[test]
    fn claim_paths_produces_valid_principal_via_mapper() {
        let paths = keycloak_claim_paths("my-client");
        let mapper = JsonPointerClaimsMapper::new(paths);
        let claims = json!({
            "sub": "user-1",
            "iss": "https://kc.example.com/realms/test",
            "aud": "my-api",
            "realm_access": { "roles": ["admin"] },
            "resource_access": {
                "my-client": { "roles": ["client-role"] }
            },
            "scope": "read write",
        });
        let principal = mapper.to_principal(&claims).unwrap();
        assert_eq!(principal.subject, "user-1");
        assert!(principal.has_role("admin"));
        assert!(principal.has_role("client-role"));
        assert_eq!(principal.scopes, vec!["read", "write"]);
    }

    #[test]
    fn realm_url() {
        let config = KeycloakRealmConfig::new(
            "http://localhost:8080".into(),
            "my-realm".into(),
            "my-client".into(),
        );
        assert_eq!(config.realm_url(), "http://localhost:8080/realms/my-realm");
    }

    #[test]
    fn jwks_uri() {
        let config = KeycloakRealmConfig::new(
            "http://localhost:8080".into(),
            "my-realm".into(),
            "my-client".into(),
        );
        assert_eq!(
            config.jwks_uri(),
            "http://localhost:8080/realms/my-realm/protocol/openid-connect/certs"
        );
    }

    #[test]
    fn token_endpoint() {
        let config = KeycloakRealmConfig::new(
            "http://localhost:8080".into(),
            "my-realm".into(),
            "my-client".into(),
        );
        assert_eq!(
            config.token_endpoint(),
            "http://localhost:8080/realms/my-realm/protocol/openid-connect/token"
        );
    }

    #[test]
    fn trailing_slash_handling() {
        let config = KeycloakRealmConfig::new(
            "http://localhost:8080/".into(),
            "test".into(),
            "client".into(),
        );
        assert_eq!(config.realm_url(), "http://localhost:8080/realms/test");
    }

    #[test]
    fn debug_redacts_client_secret() {
        let config = KeycloakRealmConfig::new(
            "https://kc.example.com".into(),
            "myrealm".into(),
            "myclient".into(),
        )
        .with_client_secret("super-secret".into());
        let debug_str = format!("{config:?}");
        assert!(!debug_str.contains("super-secret"));
        assert!(debug_str.contains("REDACTED"));
    }

    #[test]
    fn empty_client_id_produces_malformed_resource_path() {
        let paths = keycloak_claim_paths("");
        assert!(
            paths.roles.iter().any(|p| p == "/resource_access//roles"),
            "empty client_id should produce /resource_access//roles — caller must validate"
        );
    }

    #[test]
    fn keycloak_config_client_secret_accessor_with_secret() {
        let config = KeycloakRealmConfig::new(
            "https://kc.example.com".into(),
            "myrealm".into(),
            "myclient".into(),
        )
        .with_client_secret("secret-123".into());
        assert_eq!(config.client_secret(), Some("secret-123"));
    }

    #[test]
    fn keycloak_config_client_secret_accessor_without_secret() {
        let config = KeycloakRealmConfig::new(
            "https://kc.example.com".into(),
            "myrealm".into(),
            "myclient".into(),
        );
        assert!(config.client_secret().is_none());
    }

    #[test]
    fn keycloak_component_scheme() {
        let config = KeycloakRealmConfig::new(
            "https://kc.example.com".into(),
            "myrealm".into(),
            "myclient".into(),
        )
        .with_client_secret("secret".into());
        let component = KeycloakComponent::new_for_test(&config).unwrap();
        assert_eq!(component.scheme(), "keycloak");
    }

    #[test]
    fn keycloak_component_create_endpoint_valid() {
        let config = KeycloakRealmConfig::new(
            "https://kc.example.com".into(),
            "myrealm".into(),
            "myclient".into(),
        )
        .with_client_secret("secret".into());
        let component = KeycloakComponent::new_for_test(&config).unwrap();
        let ctx = NoOpComponentContext;
        // allowInternalUrls=true: the example.com host has no DNS in CI;
        // opting in lets us exercise the rest of create_endpoint without
        // standing up a network.
        let endpoint = component
            .create_endpoint(
                "keycloak:admin?operation=getUser&realm=myrealm&userId=user-1&allowInternalUrls=true",
                &ctx,
            )
            .unwrap();
        assert_eq!(
            endpoint.uri(),
            "keycloak:admin?operation=getUser&realm=myrealm&userId=user-1&allowInternalUrls=true"
        );
    }

    #[test]
    fn keycloak_component_create_endpoint_invalid() {
        let config = KeycloakRealmConfig::new(
            "https://kc.example.com".into(),
            "myrealm".into(),
            "myclient".into(),
        )
        .with_client_secret("secret".into());
        let component = KeycloakComponent::new_for_test(&config).unwrap();
        let ctx = NoOpComponentContext;
        let result = component.create_endpoint("keycloak:badpath", &ctx);
        assert!(result.is_err());
    }

    #[test]
    fn introspection_authenticator_builder_derives_endpoint() {
        use camel_auth::IntrospectionAuthenticator;
        use camel_auth::introspection::CachingTokenIntrospector;

        let opts = IntrospectionCacheOptions::default();
        let introspector = CachingTokenIntrospector::new_unchecked_for_test(
            "https://kc.example.com/admin/realms/test-realm/protocol/openid-connect/token/introspect".into(),
            "my-client".into(),
            "secret".into(),
            opts,
        );
        let mapper: Arc<dyn ClaimsMapper> = Arc::new(JsonPointerClaimsMapper::new(
            keycloak_claim_paths("my-client"),
        ));
        let _auth = IntrospectionAuthenticator::new(Arc::new(introspector), mapper);
    }

    #[tokio::test]
    async fn introspection_authenticator_builder_requires_client_secret() {
        let config = KeycloakRealmConfig::new(
            "https://kc.example.com".into(),
            "test-realm".into(),
            "my-client".into(),
        );

        let opts = IntrospectionCacheOptions::default();
        let result = config.introspection_authenticator(opts).await;
        assert!(result.is_err(), "builder should fail without client_secret");
        let err = result.unwrap_err();
        match err {
            CamelError::Config(msg) => assert!(msg.contains("client_secret")),
            other => panic!("expected Config error, got: {other:?}"),
        }
    }

    #[test]
    fn introspection_authenticator_maps_keycloak_roles() {
        use camel_auth::IntrospectionAuthenticator;
        use camel_auth::introspection::CachingTokenIntrospector;

        let opts = IntrospectionCacheOptions::default();
        let introspector = CachingTokenIntrospector::new_unchecked_for_test(
            "https://kc.example.com/admin/realms/svc/protocol/openid-connect/token/introspect"
                .into(),
            "svc".into(),
            "s".into(),
            opts,
        );
        let mapper: Arc<dyn ClaimsMapper> =
            Arc::new(JsonPointerClaimsMapper::new(keycloak_claim_paths("svc")));
        let _auth = IntrospectionAuthenticator::new(Arc::new(introspector), mapper);

        let claims = json!({
            "active": true,
            "sub": "user-1",
            "realm_access": {"roles": ["admin"]},
            "resource_access": {"svc": {"roles": ["svc-role"]}},
            "scope": "read"
        });

        let mapper = JsonPointerClaimsMapper::new(keycloak_claim_paths("svc"));
        let principal = mapper.to_principal(&claims).unwrap();
        assert!(principal.has_role("admin"));
        assert!(principal.has_role("svc-role"));
        assert_eq!(principal.scopes, vec!["read"]);
    }

    #[tokio::test]
    async fn uma_evaluator_builder_returns_evaluator() {
        let config = KeycloakRealmConfig::new(
            "https://1.1.1.1".into(),
            "test".into(),
            "authz-client".into(),
        )
        .with_client_secret("secret".into());
        let evaluator = config.uma_evaluator().await;
        assert!(evaluator.is_ok());
        let eval = evaluator.unwrap();
        let _arc: Arc<dyn PermissionEvaluator> = eval;
    }

    #[tokio::test]
    async fn uma_evaluator_fails_without_client_secret() {
        let config = KeycloakRealmConfig::new(
            "https://kc.example.com".into(),
            "test".into(),
            "authz-client".into(),
        );
        let result = config.uma_evaluator().await;
        assert!(result.is_err());
    }

    // -----------------------------------------------------------------------
    // H15: hardened HTTP client + SSRF URL validation
    // -----------------------------------------------------------------------

    #[test]
    fn hardened_http_client_builds_successfully() {
        let _client = hardened_http_client().expect("hardened client must build");
    }

    #[test]
    fn validate_server_url_accepts_https_public_host() {
        // Use an IP literal so the test is independent of DNS.
        validate_server_url("https://1.1.1.1", false).expect("public https IP literal must pass");
    }

    #[test]
    fn validate_server_url_rejects_non_http_scheme() {
        let err = validate_server_url("ftp://kc.example.com", false).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("http/https"), "msg: {msg}");
    }

    #[test]
    fn validate_server_url_rejects_loopback_when_internal_disallowed() {
        // localhost resolves to 127.0.0.1 / ::1 — both SSRF-blocked.
        let err = validate_server_url("http://localhost:8080", false).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("SSRF"), "msg: {msg}");
    }

    #[test]
    fn validate_server_url_allows_loopback_when_internal_allowed() {
        validate_server_url("http://localhost:8080", true)
            .expect("allow_internal opt-in must pass");
    }

    #[test]
    fn validate_server_url_rejects_invalid_url() {
        let err = validate_server_url("not a url", false).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("invalid"), "msg: {msg}");
    }
}
