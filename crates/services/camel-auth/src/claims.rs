use async_trait::async_trait;
use camel_api::security_policy::Principal;
use serde::{Deserialize, Serialize};

use crate::types::AuthError;

#[async_trait]
pub trait ClaimsMapper: Send + Sync {
    fn to_principal(&self, claims: &serde_json::Value) -> Result<Principal, AuthError>;
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct ClaimPaths {
    pub subject: String,
    pub roles: Vec<String>,
    pub scopes: Option<String>,
}

pub struct JsonPointerClaimsMapper {
    subject_path: String,
    role_paths: Vec<String>,
    scope_path: Option<String>,
}

impl JsonPointerClaimsMapper {
    pub fn new(paths: ClaimPaths) -> Self {
        Self {
            subject_path: paths.subject,
            role_paths: paths.roles,
            scope_path: paths.scopes,
        }
    }
}

impl ClaimsMapper for JsonPointerClaimsMapper {
    fn to_principal(&self, claims: &serde_json::Value) -> Result<Principal, AuthError> {
        let subject = claims
            .pointer(&self.subject_path)
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                AuthError::TokenInvalid(format!(
                    "missing subject at JSON pointer {}",
                    self.subject_path
                ))
            })?
            .to_string();

        let mut roles: Vec<String> = Vec::new();
        for path in &self.role_paths {
            if let Some(arr) = claims.pointer(path).and_then(|v| v.as_array()) {
                roles.extend(arr.iter().filter_map(|v| v.as_str()).map(String::from));
            }
        }
        roles.sort();
        roles.dedup();

        let scopes = self
            .scope_path
            .as_ref()
            .and_then(|p| claims.pointer(p).and_then(|v| v.as_str()))
            .map(|s| s.split_whitespace().map(String::from).collect())
            .unwrap_or_default();

        Ok(Principal {
            subject,
            roles,
            scopes,
            claims: claims.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn mapper(paths: ClaimPaths) -> JsonPointerClaimsMapper {
        JsonPointerClaimsMapper::new(paths)
    }

    fn default_paths() -> ClaimPaths {
        ClaimPaths {
            subject: "/sub".into(),
            roles: vec!["/groups".into()],
            scopes: None,
        }
    }

    #[test]
    fn extracts_subject() {
        let claims = json!({"sub": "user-1"});
        let principal = mapper(default_paths()).to_principal(&claims).unwrap();
        assert_eq!(principal.subject, "user-1");
    }

    #[test]
    fn missing_subject_returns_error() {
        let claims = json!({"no_sub": "x"});
        let result = mapper(default_paths()).to_principal(&claims);
        assert!(result.is_err());
    }

    #[test]
    fn extracts_roles_from_single_path() {
        let claims = json!({
            "sub": "u",
            "groups": ["admin", "user"]
        });
        let principal = mapper(default_paths()).to_principal(&claims).unwrap();
        assert!(principal.has_role("admin"));
        assert!(principal.has_role("user"));
    }

    #[test]
    fn extracts_roles_from_multiple_paths_and_deduplicates() {
        let paths = ClaimPaths {
            subject: "/sub".into(),
            roles: vec!["/groups".into(), "/app_roles".into()],
            scopes: None,
        };
        let claims = json!({
            "sub": "u",
            "groups": ["admin"],
            "app_roles": ["admin", "editor"]
        });
        let principal = mapper(paths).to_principal(&claims).unwrap();
        assert_eq!(principal.roles, vec!["admin", "editor"]);
    }

    #[test]
    fn no_role_paths_produces_empty_roles() {
        let paths = ClaimPaths {
            subject: "/sub".into(),
            roles: vec![],
            scopes: None,
        };
        let claims = json!({"sub": "u"});
        let principal = mapper(paths).to_principal(&claims).unwrap();
        assert!(principal.roles.is_empty());
    }

    #[test]
    fn extracts_scopes_from_space_separated_string() {
        let paths = ClaimPaths {
            subject: "/sub".into(),
            roles: vec![],
            scopes: Some("/scope".into()),
        };
        let claims = json!({"sub": "u", "scope": "read write"});
        let principal = mapper(paths).to_principal(&claims).unwrap();
        assert_eq!(principal.scopes, vec!["read", "write"]);
    }

    #[test]
    fn claims_stored_in_principal() {
        let claims = json!({"sub": "u", "custom": "value"});
        let principal = mapper(default_paths()).to_principal(&claims).unwrap();
        assert_eq!(principal.claims["custom"], "value");
    }

    #[test]
    fn custom_subject_path() {
        let paths = ClaimPaths {
            subject: "/preferred_username".into(),
            roles: vec![],
            scopes: None,
        };
        let claims = json!({"preferred_username": "alice"});
        let principal = mapper(paths).to_principal(&claims).unwrap();
        assert_eq!(principal.subject, "alice");
    }
}
