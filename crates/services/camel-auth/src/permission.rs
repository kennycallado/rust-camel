//! Permission evaluation contracts for authorization decisions.
//!
//! [`PermissionRequest`] captures who is asking, what resource, and what action.
//! [`PermissionEvaluator`] is the async trait that evaluates requests into
//! [`PermissionDecision`] results.

use async_trait::async_trait;
use camel_api::security_policy::Principal;
use serde::Deserialize;
use std::fmt;

use crate::types::AuthError;

/// A request to evaluate whether a principal may perform an action on a resource.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct PermissionRequest {
    pub principal: Principal,
    pub resource: String,
    pub action: String,
    #[serde(default)]
    pub requested_scopes: Vec<String>,
    #[serde(default)]
    pub context: serde_json::Value,
}

/// The outcome of a permission evaluation.
#[derive(Debug, Clone, PartialEq)]
pub enum PermissionDecision {
    Granted,
    Denied { reason: String },
}

impl fmt::Display for PermissionDecision {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Granted => write!(f, "Permission granted"),
            Self::Denied { reason } => write!(f, "Permission denied: {reason}"),
        }
    }
}

/// Async trait for evaluating permission requests.
#[async_trait]
pub trait PermissionEvaluator: Send + Sync {
    async fn evaluate(&self, request: PermissionRequest) -> Result<PermissionDecision, AuthError>;
}

/// Describes where a permission value comes from.
#[derive(Debug, Clone, PartialEq)]
pub enum PermissionValueSource {
    Literal(String),
    Header(String),
    Property(String),
}

/// Configuration for building permission evaluation context.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PermissionContextConfig {
    pub include_headers: Vec<String>,
    pub include_properties: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn test_principal() -> Principal {
        Principal {
            subject: "alice".into(),
            issuer: "https://keycloak.example.com/realms/test".into(),
            audience: vec!["camel-api".into()],
            roles: vec!["admin".into()],
            scopes: vec!["read".into()],
            claims: json!({}),
        }
    }

    #[test]
    fn permission_request_deserializes_with_context() {
        let json = r#"{
            "principal":{"subject":"alice","issuer":"test","audience":[],"scopes":[],"roles":[],"claims":{}},
            "resource":"/orders/123",
            "action":"read",
            "requested_scopes":["read"],
            "context":{"source":"api"}
        }"#;
        let req: PermissionRequest =
            serde_json::from_str(json).expect("deserialization should succeed");
        assert_eq!(req.resource, "/orders/123");
        assert_eq!(req.action, "read");
        assert_eq!(req.requested_scopes, vec!["read".to_string()]);
    }

    #[test]
    fn permission_request_deserializes_with_defaults() {
        let json = r#"{
            "principal":{"subject":"bob","issuer":"test","audience":[],"scopes":[],"roles":[],"claims":{}},
            "resource":"/orders",
            "action":"write"
        }"#;
        let req: PermissionRequest =
            serde_json::from_str(json).expect("deserialization should succeed");
        assert!(req.requested_scopes.is_empty());
        assert!(req.context.is_null());
    }

    #[test]
    fn permission_decision_granted_display() {
        let decision = PermissionDecision::Granted;
        assert_eq!(format!("{decision}"), "Permission granted");
    }

    #[test]
    fn permission_decision_denied_display() {
        let decision = PermissionDecision::Denied {
            reason: "insufficient scope".into(),
        };
        assert_eq!(
            format!("{decision}"),
            "Permission denied: insufficient scope"
        );
    }

    struct AlwaysGrant;

    #[async_trait]
    impl PermissionEvaluator for AlwaysGrant {
        async fn evaluate(
            &self,
            _request: PermissionRequest,
        ) -> Result<PermissionDecision, AuthError> {
            Ok(PermissionDecision::Granted)
        }
    }

    #[tokio::test]
    async fn mock_evaluator_grants() {
        let evaluator = AlwaysGrant;
        let request = PermissionRequest {
            principal: test_principal(),
            resource: "/orders".into(),
            action: "read".into(),
            requested_scopes: vec![],
            context: json!({}),
        };
        let result = evaluator.evaluate(request).await.expect("should succeed");
        assert_eq!(result, PermissionDecision::Granted);
    }
}
