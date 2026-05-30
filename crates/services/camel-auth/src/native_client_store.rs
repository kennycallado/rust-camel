use camel_api::CamelError;
use std::collections::HashSet;
use std::fmt;
use tracing::warn;

pub struct M2mClient {
    pub client_id: String,
    pub secret: M2mClientSecret,
    pub roles: Vec<String>,
    pub scopes: Vec<String>,
}

#[derive(Clone)]
pub enum M2mClientSecret {
    Env { name: String },
    Plaintext { value: String },
}

struct ResolvedM2mClient {
    client_id: String,
    secret_value: String,
    roles: Vec<String>,
    scopes: Vec<String>,
}

pub struct M2mClientStore {
    clients: Vec<ResolvedM2mClient>,
}

pub struct M2mClientRef<'a> {
    pub client_id: &'a str,
    pub roles: &'a [String],
    pub scopes: &'a [String],
}

impl fmt::Debug for M2mClientSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            M2mClientSecret::Env { name } => write!(f, "Env {{ name: \"{name}\" }}"), // allow-secret
            M2mClientSecret::Plaintext { .. } => {
                write!(f, "Plaintext {{ value: \"[REDACTED]\" }}") // allow-secret
            }
        }
    }
}

impl fmt::Debug for M2mClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("M2mClient")
            .field("client_id", &self.client_id)
            .field("secret", &self.secret)
            .field("roles", &self.roles)
            .field("scopes", &self.scopes)
            .finish()
    }
}

impl fmt::Debug for M2mClientStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("M2mClientStore")
            .field("client_count", &self.clients.len())
            .field("secrets", &"[REDACTED]")
            .finish()
    }
}

impl M2mClientStore {
    pub fn try_new(clients: Vec<M2mClient>) -> Result<Self, CamelError> {
        let mut seen_ids = HashSet::new();

        for c in &clients {
            if !seen_ids.insert(c.client_id.clone()) {
                return Err(CamelError::Config(format!(
                    "duplicate client_id: '{}'",
                    c.client_id
                )));
            }
        }

        let mut resolved = Vec::with_capacity(clients.len());
        for c in clients {
            let secret_value = match &c.secret {
                M2mClientSecret::Env { name } => {
                    let val = std::env::var(name).map_err(|_| {
                        CamelError::Config(format!("M2M client env var not set: {name}"))
                    })?;
                    if val.is_empty() {
                        return Err(CamelError::Config(format!(
                            "M2M client env var is empty: {name}"
                        )));
                    }
                    val
                }
                M2mClientSecret::Plaintext { value } => {
                    if value.is_empty() {
                        return Err(CamelError::Config(
                            "M2M client plaintext secret is empty".into(),
                        ));
                    }
                    warn!(
                        "M2M client '{}' uses plaintext secret — use env vars in production",
                        c.client_id
                    );
                    value.clone()
                }
            };
            resolved.push(ResolvedM2mClient {
                client_id: c.client_id,
                secret_value,
                roles: c.roles,
                scopes: c.scopes,
            });
        }

        Ok(Self { clients: resolved })
    }

    pub fn lookup(&self, client_id: &str, client_secret: &str) -> Option<M2mClientRef<'_>> {
        use sha2::{Digest, Sha256};

        let secret_hash = Sha256::digest(client_secret.as_bytes());
        for c in &self.clients {
            if c.client_id != client_id {
                continue;
            }
            let stored_hash = Sha256::digest(c.secret_value.as_bytes());
            let acc = secret_hash
                .iter()
                .zip(stored_hash.iter())
                .fold(0u8, |acc, (a, b)| acc | (a ^ b));
            if acc == 0 {
                return Some(M2mClientRef {
                    client_id: &c.client_id,
                    roles: &c.roles,
                    scopes: &c.scopes,
                });
            }
        }
        None
    }

    pub fn get(&self, client_id: &str) -> Option<M2mClientRef<'_>> {
        self.clients
            .iter()
            .find(|c| c.client_id == client_id)
            .map(|c| M2mClientRef {
                client_id: &c.client_id,
                roles: &c.roles,
                scopes: &c.scopes,
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_rejects_duplicate_client_ids() {
        let result = M2mClientStore::try_new(vec![
            M2mClient {
                client_id: "worker".into(),
                secret: M2mClientSecret::Plaintext {
                    value: "secret-a".into(),
                },
                roles: vec!["read".into()],
                scopes: vec!["api:read".into()],
            },
            M2mClient {
                client_id: "worker".into(),
                secret: M2mClientSecret::Plaintext {
                    value: "secret-b".into(),
                },
                roles: vec!["write".into()],
                scopes: vec!["api:write".into()],
            },
        ]);
        let err = result.unwrap_err();
        assert!(format!("{err}").contains("duplicate client_id"));
    }

    #[test]
    fn store_rejects_empty_secret() {
        let result = M2mClientStore::try_new(vec![M2mClient {
            client_id: "worker".into(),
            secret: M2mClientSecret::Plaintext { value: "".into() },
            roles: vec![],
            scopes: vec![],
        }]);
        let err = result.unwrap_err();
        assert!(format!("{err}").contains("empty"));
    }

    #[test]
    fn store_lookup_valid_client_constant_time() {
        let store = M2mClientStore::try_new(vec![M2mClient {
            client_id: "billing".into(),
            secret: M2mClientSecret::Plaintext {
                value: "secret-123".into(),
            },
            roles: vec!["billing".into()],
            scopes: vec!["orders:read".into(), "orders:write".into()],
        }])
        .unwrap();
        let client = store.lookup("billing", "secret-123").unwrap();
        assert_eq!(client.client_id, "billing");
        assert_eq!(client.roles, vec!["billing"]);
    }

    #[test]
    fn store_lookup_wrong_secret_returns_none() {
        let store = M2mClientStore::try_new(vec![M2mClient {
            client_id: "billing".into(),
            secret: M2mClientSecret::Plaintext {
                value: "secret-123".into(),
            },
            roles: vec![],
            scopes: vec![],
        }])
        .unwrap();
        assert!(store.lookup("billing", "wrong").is_none());
    }

    #[test]
    fn store_lookup_unknown_client_returns_none() {
        let store = M2mClientStore::try_new(vec![]).unwrap();
        assert!(store.lookup("unknown", "secret").is_none());
    }

    #[test]
    fn store_resolves_env_secret() {
        // SAFETY: test-scoped env mutation used to verify env-backed secret resolution.
        unsafe { std::env::set_var("TEST_M2M_SECRET", "env-secret-value") };
        let store = M2mClientStore::try_new(vec![M2mClient {
            client_id: "worker".into(),
            secret: M2mClientSecret::Env {
                name: "TEST_M2M_SECRET".into(),
            },
            roles: vec![],
            scopes: vec![],
        }])
        .unwrap();
        assert!(store.lookup("worker", "env-secret-value").is_some());
        // SAFETY: revert test-scoped env mutation before test exits.
        unsafe { std::env::remove_var("TEST_M2M_SECRET") };
    }

    #[test]
    fn store_rejects_missing_env_var() {
        let result = M2mClientStore::try_new(vec![M2mClient {
            client_id: "worker".into(),
            secret: M2mClientSecret::Env {
                name: "NONEXISTENT_VAR_XYZ".into(),
            },
            roles: vec![],
            scopes: vec![],
        }]);
        let err = result.unwrap_err();
        assert!(format!("{err}").contains("NONEXISTENT_VAR_XYZ"));
    }

    #[test]
    fn store_debug_redacts_secrets() {
        let store = M2mClientStore::try_new(vec![M2mClient {
            client_id: "worker".into(),
            secret: M2mClientSecret::Plaintext {
                value: "super-secret".into(),
            },
            roles: vec![],
            scopes: vec![],
        }])
        .unwrap();
        let debug = format!("{store:?}");
        assert!(!debug.contains("super-secret"));
        assert!(debug.contains("[REDACTED]"));
    }
}
