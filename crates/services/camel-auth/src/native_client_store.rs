use camel_api::CamelError;
use std::collections::HashSet;
use std::fmt;
use tracing::warn;
use zeroize::Zeroizing;

/// Compare two byte slices without short-circuiting on early mismatch.
/// Returns `true` if both slices have equal length and identical bytes.
///
/// The length check leaks the length of the inputs, which is acceptable
/// because client IDs are typically fixed-length or their length is not
/// secret. The byte-by-byte comparison runs in constant time relative to
/// the length of the inputs.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

pub struct M2mClient {
    pub client_id: String,
    pub secret: M2mClientSecret,
    pub roles: Vec<String>,
    pub scopes: Vec<String>,
}

#[derive(Clone)]
pub enum M2mClientSecret {
    Env { name: String },
    Plaintext { value: Zeroizing<String> },
}

struct ResolvedM2mClient {
    client_id: String,
    secret_value: Zeroizing<String>,
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
                    Zeroizing::new(val)
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
            if !constant_time_eq(c.client_id.as_bytes(), client_id.as_bytes()) {
                continue;
            }
            let stored_hash = Sha256::digest(c.secret_value.as_bytes());
            if constant_time_eq(&secret_hash, &stored_hash) {
                return Some(M2mClientRef {
                    client_id: &c.client_id,
                    roles: &c.roles,
                    scopes: &c.scopes,
                });
            }
            // Client ID matched but secret didn't — no point scanning further
            // (dedup guarantees at most one entry per client_id).
            break;
        }
        None
    }

    pub fn get(&self, client_id: &str) -> Option<M2mClientRef<'_>> {
        self.clients
            .iter()
            .find(|c| constant_time_eq(c.client_id.as_bytes(), client_id.as_bytes()))
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
    fn constant_time_eq_same_slice_returns_true() {
        assert!(constant_time_eq(b"hello", b"hello"));
        assert!(constant_time_eq(b"", b""));
        assert!(constant_time_eq(b"a", b"a"));
    }

    #[test]
    fn constant_time_eq_different_length_returns_false() {
        assert!(!constant_time_eq(b"hello", b"world!"));
        assert!(!constant_time_eq(b"a", b""));
    }

    #[test]
    fn constant_time_eq_differs_in_last_byte_returns_false() {
        assert!(!constant_time_eq(b"client-aaa1", b"client-aaa2"));
        assert!(!constant_time_eq(b"aaa", b"aab"));
    }

    #[test]
    fn constant_time_eq_differs_in_first_byte_returns_false() {
        assert!(!constant_time_eq(b"xabc", b"yabc"));
    }

    #[test]
    fn store_lookup_wrong_secret_for_similar_client_id() {
        let store = M2mClientStore::try_new(vec![
            M2mClient {
                client_id: "client-aaa1".into(),
                secret: M2mClientSecret::Plaintext {
                    value: Zeroizing::new("secret-1".into()),
                },
                roles: vec![],
                scopes: vec![],
            },
            M2mClient {
                client_id: "client-aaa2".into(),
                secret: M2mClientSecret::Plaintext {
                    value: Zeroizing::new("secret-2".into()),
                },
                roles: vec![],
                scopes: vec![],
            },
        ])
        .unwrap();
        // Wrong secret for client-aaa1 should return None
        assert!(store.lookup("client-aaa1", "wrong-secret").is_none());
        // Right secret should succeed
        assert!(store.lookup("client-aaa1", "secret-1").is_some());
        assert!(store.lookup("client-aaa2", "secret-2").is_some());
    }

    #[test]
    fn store_rejects_duplicate_client_ids() {
        let result = M2mClientStore::try_new(vec![
            M2mClient {
                client_id: "worker".into(),
                secret: M2mClientSecret::Plaintext {
                    value: Zeroizing::new("secret-a".into()),
                },
                roles: vec!["read".into()],
                scopes: vec!["api:read".into()],
            },
            M2mClient {
                client_id: "worker".into(),
                secret: M2mClientSecret::Plaintext {
                    value: Zeroizing::new("secret-b".into()),
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
            secret: M2mClientSecret::Plaintext {
                value: Zeroizing::new("".into()),
            },
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
                value: Zeroizing::new("secret-123".into()),
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
                value: Zeroizing::new("secret-123".into()),
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
                value: Zeroizing::new("super-secret".into()),
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
