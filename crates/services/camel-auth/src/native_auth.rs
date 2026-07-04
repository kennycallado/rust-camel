use async_trait::async_trait;
use camel_api::CamelError;
use camel_api::security_policy::Principal;
use std::fmt;
use tracing::warn;
use zeroize::Zeroizing;

pub struct NativeCredential {
    pub secret: NativeCredentialSecret,
    pub principal: Principal,
}

#[derive(Clone)]
pub enum NativeCredentialSecret {
    Env { name: String },
    Plaintext { value: Zeroizing<String> },
}

#[derive(Clone)]
struct ResolvedCredential {
    secret_value: Zeroizing<String>,
    principal: Principal,
}

pub struct NativeCredentialStore {
    credentials: Vec<ResolvedCredential>,
}

impl fmt::Debug for NativeCredentialSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NativeCredentialSecret::Env { name } => {
                write!(f, "Env {{ name: \"{name}\" }}") // allow-secret
            }
            NativeCredentialSecret::Plaintext { .. } => {
                write!(f, "Plaintext {{ value: \"[REDACTED]\" }}") // allow-secret
            }
        }
    }
}

impl fmt::Debug for NativeCredential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NativeCredential")
            .field("secret", &self.secret)
            .field("principal", &self.principal.subject)
            .finish()
    }
}

impl fmt::Debug for ResolvedCredential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ResolvedCredential")
            .field("secret_value", &"[REDACTED]")
            .field("principal", &self.principal.subject)
            .finish()
    }
}

impl fmt::Debug for NativeCredentialStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NativeCredentialStore")
            .field("credential_count", &self.credentials.len())
            .finish()
    }
}

impl NativeCredentialStore {
    pub fn try_new(credentials: Vec<NativeCredential>) -> Result<Self, CamelError> {
        let mut resolved = Vec::with_capacity(credentials.len());
        for c in credentials {
            let secret_value = match &c.secret {
                NativeCredentialSecret::Env { name } => {
                    let val = std::env::var(name).map_err(|_| {
                        CamelError::Config(format!("native auth env var not set: {name}"))
                    })?;
                    if val.is_empty() {
                        return Err(CamelError::Config(format!(
                            "native auth env var is empty: {name}"
                        )));
                    }
                    Zeroizing::new(val)
                }
                NativeCredentialSecret::Plaintext { value } => {
                    if value.is_empty() {
                        return Err(CamelError::Config(
                            "native auth plaintext secret is empty".into(),
                        ));
                    }
                    warn!("native credential uses plaintext secret — use env vars in production");
                    value.clone()
                }
            };
            resolved.push(ResolvedCredential {
                secret_value,
                principal: c.principal,
            });
        }
        Ok(Self {
            credentials: resolved,
        })
    }

    pub fn lookup(&self, presented: &str) -> Option<&Principal> {
        if presented.is_empty() {
            return None;
        }
        for c in &self.credentials {
            let a = c.secret_value.as_bytes();
            let b = presented.as_bytes();
            let mut acc: u8 = if a.len() != b.len() { 1 } else { 0 };
            let max_len = a.len().max(b.len());
            for i in 0..max_len {
                let x = if i < a.len() { a[i] } else { 0 };
                let y = if i < b.len() { b[i] } else { 0 };
                acc |= x ^ y;
            }
            if acc == 0 {
                return Some(&c.principal);
            }
        }
        None
    }
}

pub struct StaticTokenAuthenticator {
    store: NativeCredentialStore,
}

impl fmt::Debug for StaticTokenAuthenticator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StaticTokenAuthenticator")
            .field("store", &"[REDACTED]")
            .finish()
    }
}

impl StaticTokenAuthenticator {
    pub fn new(store: NativeCredentialStore) -> Self {
        Self { store }
    }
}

#[async_trait]
impl crate::TokenAuthenticator for StaticTokenAuthenticator {
    async fn authenticate_bearer(&self, token: &str) -> Result<Principal, CamelError> {
        self.store
            .lookup(token)
            .cloned()
            .ok_or_else(|| CamelError::Unauthenticated("invalid credential".into()))
    }
}

pub struct ApiKeyAuthenticator {
    header: String,
    store: NativeCredentialStore,
}

impl fmt::Debug for ApiKeyAuthenticator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ApiKeyAuthenticator")
            .field("header", &self.header)
            .field("store", &"[REDACTED]")
            .finish()
    }
}

impl ApiKeyAuthenticator {
    pub fn new(header: String, store: NativeCredentialStore) -> Self {
        Self { header, store }
    }

    pub fn header(&self) -> &str {
        &self.header
    }

    pub async fn authenticate_api_key(&self, key: &str) -> Result<Principal, CamelError> {
        self.store
            .lookup(key)
            .cloned()
            .ok_or_else(|| CamelError::Unauthenticated("invalid credential".into()))
    }

    pub async fn authenticate_exchange(
        &self,
        exchange: &mut camel_api::Exchange,
    ) -> Result<Principal, CamelError> {
        let key = exchange
            .input
            .header_ic(&self.header)
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                CamelError::Unauthenticated(format!("missing header: {}", self.header))
            })?;
        self.authenticate_api_key(key).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TokenAuthenticator;
    use crate::built_in::RolePolicy;
    use crate::built_in::ScopePolicy;
    use camel_api::security_policy::SecurityPolicy;
    use camel_api::{Exchange, Message};

    fn test_principal(subject: &str, roles: Vec<&str>, scopes: Vec<&str>) -> Principal {
        Principal {
            subject: subject.to_string(),
            issuer: "native".to_string(),
            audience: vec![],
            scopes: scopes.iter().map(|s| s.to_string()).collect(),
            roles: roles.iter().map(|s| s.to_string()).collect(),
            claims: serde_json::Value::Null,
        }
    }

    #[test]
    fn test_store_finds_matching_plaintext_credential() {
        let store = NativeCredentialStore::try_new(vec![NativeCredential {
            secret: NativeCredentialSecret::Plaintext {
                value: Zeroizing::new("secret-key-123".to_string()),
            },
            principal: test_principal("admin", vec!["admin"], vec![]),
        }])
        .unwrap();
        let found = store.lookup("secret-key-123");
        assert!(found.is_some());
        assert_eq!(found.unwrap().subject, "admin");
    }

    #[test]
    fn test_store_returns_none_on_no_match() {
        let store = NativeCredentialStore::try_new(vec![NativeCredential {
            secret: NativeCredentialSecret::Plaintext {
                value: Zeroizing::new("secret-key-123".to_string()),
            },
            principal: test_principal("admin", vec!["admin"], vec![]),
        }])
        .unwrap();
        let found = store.lookup("wrong-key");
        assert!(found.is_none());
    }

    #[test]
    fn test_store_returns_none_on_empty_input() {
        let store = NativeCredentialStore::try_new(vec![NativeCredential {
            secret: NativeCredentialSecret::Plaintext {
                value: Zeroizing::new("secret-key-123".to_string()),
            },
            principal: test_principal("admin", vec!["admin"], vec![]),
        }])
        .unwrap();
        let found = store.lookup("");
        assert!(found.is_none());
    }

    #[test]
    fn test_store_resolves_env_var() {
        let key = format!("TEST_NATIVE_AUTH_KEY_{}", std::process::id());
        // SAFETY: test-only env mutation; no concurrent tests touch this key.
        unsafe { std::env::set_var(&key, "env-secret-value") };
        let store = NativeCredentialStore::try_new(vec![NativeCredential {
            secret: NativeCredentialSecret::Env { name: key.clone() },
            principal: test_principal("env-user", vec!["user"], vec![]),
        }])
        .unwrap();
        let found = store.lookup("env-secret-value");
        assert!(found.is_some());
        assert_eq!(found.unwrap().subject, "env-user");
        // SAFETY: cleanup of test-only env var.
        unsafe { std::env::remove_var(&key) };
    }

    #[test]
    fn test_store_rejects_missing_env_var() {
        let result = NativeCredentialStore::try_new(vec![NativeCredential {
            secret: NativeCredentialSecret::Env {
                name: "SURELY_MISSING_ENV_VAR_XYZ_12345".to_string(),
            },
            principal: test_principal("bad", vec![], vec![]),
        }]);
        assert!(result.is_err());
    }

    #[test]
    fn test_store_rejects_empty_plaintext() {
        let result = NativeCredentialStore::try_new(vec![NativeCredential {
            secret: NativeCredentialSecret::Plaintext {
                value: Zeroizing::new("".to_string()),
            },
            principal: test_principal("bad", vec![], vec![]),
        }]);
        assert!(result.is_err());
    }

    #[test]
    fn test_store_accepts_plaintext_for_dev() {
        let store = NativeCredentialStore::try_new(vec![NativeCredential {
            secret: NativeCredentialSecret::Plaintext {
                value: Zeroizing::new("insecure".to_string()),
            },
            principal: test_principal("dev", vec![], vec![]),
        }])
        .unwrap();
        assert!(store.lookup("insecure").is_some());
    }

    #[tokio::test]
    async fn test_static_token_authenticator_valid_token() {
        let store = NativeCredentialStore::try_new(vec![NativeCredential {
            secret: NativeCredentialSecret::Plaintext {
                value: Zeroizing::new("my-bearer-token".to_string()),
            },
            principal: test_principal("svc-account", vec!["service"], vec![]),
        }])
        .unwrap();
        let auth = StaticTokenAuthenticator::new(store);
        let result = auth.authenticate_bearer("my-bearer-token").await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap().subject, "svc-account");
    }

    #[tokio::test]
    async fn test_static_token_authenticator_invalid_token() {
        let store = NativeCredentialStore::try_new(vec![NativeCredential {
            secret: NativeCredentialSecret::Plaintext {
                value: Zeroizing::new("my-bearer-token".to_string()),
            },
            principal: test_principal("svc-account", vec!["service"], vec![]),
        }])
        .unwrap();
        let auth = StaticTokenAuthenticator::new(store);
        let result = auth.authenticate_bearer("wrong-token").await;
        assert!(result.is_err());
        match result.unwrap_err() {
            CamelError::Unauthenticated(msg) => {
                assert!(msg.contains("invalid credential"))
            }
            e => panic!("expected Unauthenticated, got: {e:?}"),
        }
    }

    #[tokio::test]
    async fn test_api_key_authenticator_valid_key() {
        let store = NativeCredentialStore::try_new(vec![NativeCredential {
            secret: NativeCredentialSecret::Plaintext {
                value: Zeroizing::new("ak-12345".to_string()),
            },
            principal: test_principal("api-user", vec!["read"], vec!["api:read"]),
        }])
        .unwrap();
        let auth = ApiKeyAuthenticator::new("x-api-key".to_string(), store);
        let result = auth.authenticate_api_key("ak-12345").await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap().subject, "api-user");
    }

    #[tokio::test]
    async fn test_api_key_authenticator_invalid_key() {
        let store = NativeCredentialStore::try_new(vec![NativeCredential {
            secret: NativeCredentialSecret::Plaintext {
                value: Zeroizing::new("ak-12345".to_string()),
            },
            principal: test_principal("api-user", vec!["read"], vec![]),
        }])
        .unwrap();
        let auth = ApiKeyAuthenticator::new("x-api-key".to_string(), store);
        let result = auth.authenticate_api_key("wrong").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_api_key_authenticate_exchange() {
        let store = NativeCredentialStore::try_new(vec![NativeCredential {
            secret: NativeCredentialSecret::Plaintext {
                value: Zeroizing::new("ak-exchange".to_string()),
            },
            principal: test_principal("ex-user", vec!["read"], vec![]),
        }])
        .unwrap();
        let auth = ApiKeyAuthenticator::new("x-api-key".to_string(), store);
        let mut exchange = Exchange::new(Message::default());
        exchange.input.set_header("x-api-key", "ak-exchange");
        let result = auth.authenticate_exchange(&mut exchange).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap().subject, "ex-user");
    }

    #[tokio::test]
    async fn test_api_key_authenticate_exchange_missing_header() {
        let store = NativeCredentialStore::try_new(vec![NativeCredential {
            secret: NativeCredentialSecret::Plaintext {
                value: Zeroizing::new("ak-exchange".to_string()),
            },
            principal: test_principal("ex-user", vec!["read"], vec![]),
        }])
        .unwrap();
        let auth = ApiKeyAuthenticator::new("x-api-key".to_string(), store);
        let mut exchange = Exchange::new(Message::default());
        let result = auth.authenticate_exchange(&mut exchange).await;
        assert!(result.is_err());
    }

    #[test]
    fn test_api_key_authenticator_exposes_header() {
        let store = NativeCredentialStore::try_new(vec![]).unwrap();
        let auth = ApiKeyAuthenticator::new("x-api-key".to_string(), store);
        assert_eq!(auth.header(), "x-api-key");
    }

    #[tokio::test]
    async fn test_static_token_works_with_role_policy() {
        let store = NativeCredentialStore::try_new(vec![NativeCredential {
            secret: NativeCredentialSecret::Plaintext {
                value: Zeroizing::new("test-token".to_string()),
            },
            principal: test_principal("admin-user", vec!["admin"], vec![]),
        }])
        .unwrap();
        let authenticator: std::sync::Arc<dyn TokenAuthenticator> =
            std::sync::Arc::new(StaticTokenAuthenticator::new(store));
        let policy = RolePolicy::new(vec!["admin".to_string()], true, false, authenticator);
        let mut exchange = Exchange::new(Message::default());
        exchange
            .input
            .set_header("authorization", "Bearer test-token");
        let decision = policy.evaluate(&mut exchange).await.unwrap();
        assert!(matches!(
            decision,
            camel_api::security_policy::AuthorizationDecision::Granted { .. }
        ));
    }

    #[tokio::test]
    async fn test_static_token_works_with_scope_policy() {
        let store = NativeCredentialStore::try_new(vec![NativeCredential {
            secret: NativeCredentialSecret::Plaintext {
                value: Zeroizing::new("scoped-token".to_string()),
            },
            principal: test_principal("reader", vec![], vec!["api:read"]),
        }])
        .unwrap();
        let authenticator: std::sync::Arc<dyn TokenAuthenticator> =
            std::sync::Arc::new(StaticTokenAuthenticator::new(store));
        let policy = ScopePolicy::new(vec!["api:read".to_string()], true, false, authenticator);
        let mut exchange = Exchange::new(Message::default());
        exchange
            .input
            .set_header("authorization", "Bearer scoped-token");
        let decision = policy.evaluate(&mut exchange).await.unwrap();
        assert!(matches!(
            decision,
            camel_api::security_policy::AuthorizationDecision::Granted { .. }
        ));
    }

    #[tokio::test]
    async fn test_static_token_denied_by_role_policy() {
        let store = NativeCredentialStore::try_new(vec![NativeCredential {
            secret: NativeCredentialSecret::Plaintext {
                value: Zeroizing::new("user-token".to_string()),
            },
            principal: test_principal("user", vec!["user"], vec![]),
        }])
        .unwrap();
        let authenticator: std::sync::Arc<dyn TokenAuthenticator> =
            std::sync::Arc::new(StaticTokenAuthenticator::new(store));
        let policy = RolePolicy::new(vec!["admin".to_string()], true, false, authenticator);
        let mut exchange = Exchange::new(Message::default());
        exchange
            .input
            .set_header("authorization", "Bearer user-token");
        let decision = policy.evaluate(&mut exchange).await.unwrap();
        assert!(matches!(
            decision,
            camel_api::security_policy::AuthorizationDecision::Denied { .. }
        ));
    }
}
