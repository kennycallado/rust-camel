use std::fmt;
use std::sync::Arc;

use camel_auth::oauth2::TokenProvider;

use crate::admin_operation::AdminOperation;

#[derive(Clone)]
pub struct AdminEndpointConfig {
    pub server_url: String,
    pub target_realm: Option<String>,
    pub operation: AdminOperation,
    pub user_id: Option<String>,
    pub token_provider: Arc<dyn TokenProvider>,
    pub http: reqwest::Client,
}

impl fmt::Debug for AdminEndpointConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AdminEndpointConfig")
            .field("server_url", &self.server_url)
            .field("target_realm", &self.target_realm)
            .field("operation", &self.operation)
            .field("user_id", &self.user_id)
            .field("token_provider", &"REDACTED")
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use camel_auth::types::AuthError;

    #[derive(Debug, Clone)]
    struct MockTokenProvider;

    #[async_trait]
    impl TokenProvider for MockTokenProvider {
        async fn get_token(&self) -> Result<String, AuthError> {
            Ok("test-token".to_string())
        }
    }

    #[test]
    fn admin_config_debug_redacts_secrets() {
        let config = AdminEndpointConfig {
            server_url: "http://localhost:8080".into(),
            target_realm: Some("test".into()),
            operation: AdminOperation::GetUser,
            user_id: None,
            token_provider: Arc::new(MockTokenProvider),
            http: reqwest::Client::new(),
        };
        let debug_str = format!("{config:?}");
        assert!(debug_str.contains("server_url"));
        assert!(debug_str.contains("REDACTED"));
    }
}
