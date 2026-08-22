//! Deterministic security config fixture for kernel E2E tests
//! (`unify-transport-auth`, Task 1.10).
//!
//! Builds a `SecurityConfig` whose native section carries exactly one
//! inline (plaintext) static credential — no `{{env:}}` placeholders, no
//! network, fully deterministic. Feeds `resolve_authenticators` in E2E
//! tests and mirrors the store the CLI builds from the same config.
//!
//! Lockstep chain (ADR-0055 store synthesis): update together with
//! `native_authenticator` in `crates/camel-cli/src/security.rs` and
//! `native_store_from_config` in
//! `crates/camel-test/tests/auth_multi_credential_test.rs`.

use std::sync::Arc;

use camel_auth::native_auth::{NativeCredential, NativeCredentialSecret, NativeCredentialStore};
use camel_auth::{Principal, ProviderEntry, ProviderRegistry, StaticTokenAuthenticator};
use camel_config::config::{NativeAuthConfig, NativeCredentialEntry, SecurityConfig};

/// Fixture with a single static provider named `name`.
///
/// - token: `test-token-<name>`
/// - subject: `test-user-<name>`
/// - roles: `["test-role"]`
pub struct SecurityConfigFixture {
    name: String,
}

impl SecurityConfigFixture {
    pub fn single_static_provider(name: &str) -> Self {
        Self {
            name: name.to_string(),
        }
    }

    fn subject(&self) -> String {
        format!("test-user-{}", self.name)
    }

    fn token(&self) -> String {
        format!("test-token-{}", self.name) // allow-secret
    }

    /// The fixture's `SecurityConfig` (concrete type — feeds
    /// `resolve_authenticators` in E2E tests).
    pub fn to_config(&self) -> SecurityConfig {
        SecurityConfig {
            native: Some(NativeAuthConfig {
                subject: self.subject(),
                issuer: None,
                bearer_token: None,
                api_key: None,
                roles: vec![],
                scopes: vec![],
                credentials: vec![NativeCredentialEntry {
                    subject: self.subject(),
                    secret_env: None,
                    secret: Some(self.token()),
                    roles: vec!["test-role".to_string()],
                    scopes: vec![],
                }],
            }),
            ..Default::default()
        }
    }

    /// Convenience: a `ProviderRegistry` holding the fixture's static
    /// authenticator registered under `name` — the same store the CLI
    /// builds from [`Self::to_config`].
    pub fn providers(&self) -> ProviderRegistry {
        let credential = NativeCredential {
            secret: NativeCredentialSecret::Plaintext {
                value: self.token().into(),
            },
            principal: Principal {
                subject: self.subject(),
                issuer: "native".into(),
                audience: vec![],
                scopes: vec![],
                roles: vec!["test-role".into()],
                claims: serde_json::Value::Object(Default::default()),
            },
        };
        let store = NativeCredentialStore::try_new(vec![credential])
            .expect("fixture credential is structurally valid"); // allow-unwrap
        let registry = ProviderRegistry::new();
        registry.register(
            &self.name,
            ProviderEntry {
                authenticator: Arc::new(StaticTokenAuthenticator::new(store)),
                audience_binding: None,
            },
        );
        registry
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixture_serializes_without_env_placeholders() {
        let fixture = SecurityConfigFixture::single_static_provider("idp-test");
        let toml = toml::to_string(&fixture.to_config().native.expect("native section"))
            .expect("serializes"); // allow-unwrap
        assert!(
            toml.contains("test-token-idp-test"),
            "inline token must serialize for inspection: {toml}"
        );
        assert!(
            !toml.contains("{{env:"),
            "no env placeholders in a deterministic fixture: {toml}"
        );
    }

    #[tokio::test]
    async fn fixture_principal_shape_matches_cli_mapping() {
        // Pins the lockstep contract: same principal the CLI's
        // `native_principal` builds from the fixture config.
        let fixture = SecurityConfigFixture::single_static_provider("idp-test");
        let registry = fixture.providers();
        let entry = registry.resolve("idp-test").expect("registered"); // allow-unwrap
        let principal = entry
            .authenticator
            .authenticate_bearer("test-token-idp-test")
            .await
            .expect("fixture token authenticates"); // allow-unwrap
        assert_eq!(principal.subject, "test-user-idp-test");
        assert_eq!(principal.issuer, "native");
        assert_eq!(principal.roles, vec!["test-role".to_string()]);
        assert_eq!(principal.claims, serde_json::json!({}));
    }

    #[test]
    fn fixture_registry_resolves() {
        let fixture = SecurityConfigFixture::single_static_provider("idp-test");
        let registry = fixture.providers();
        assert!(registry.resolve("idp-test").is_some());
        assert!(registry.resolve("ghost").is_none());
    }
}
