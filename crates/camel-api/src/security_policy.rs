use serde::{Deserialize, Serialize};

/// Represents an authenticated principal extracted from token claims.
///
/// Provider-neutral: the `ClaimsMapper` trait in `camel-auth` is responsible
/// for mapping provider-specific claim shapes into this structure.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct Principal {
    pub subject: String,
    pub roles: Vec<String>,
    pub scopes: Vec<String>,
    pub claims: serde_json::Value,
}

impl Principal {
    /// Check if the principal has a specific role.
    pub fn has_role(&self, role: &str) -> bool {
        self.roles.iter().any(|r| r == role)
    }

    /// Check if the principal has a specific scope.
    pub fn has_scope(&self, scope: &str) -> bool {
        self.scopes.iter().any(|s| s == scope)
    }
}
