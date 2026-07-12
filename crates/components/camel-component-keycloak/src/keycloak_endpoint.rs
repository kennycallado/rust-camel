use std::sync::Arc;

use camel_api::CamelError;
use camel_auth::oauth2::TokenProvider;
use camel_component_api::{
    BoxProcessor, Consumer, Endpoint, ProducerContext, RuntimeObservability,
};

use crate::admin_endpoint_config::AdminEndpointConfig;
use crate::admin_operation::AdminOperation;
use crate::events_endpoint_config::EventsEndpointConfig;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeycloakEndpointKind {
    Admin,
    Events,
}

impl KeycloakEndpointKind {
    pub fn from_uri_prefix(prefix: &str) -> Option<Self> {
        match prefix {
            "admin" => Some(Self::Admin),
            "events" => Some(Self::Events),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub enum KeycloakEndpointConfig {
    Admin(AdminEndpointConfig),
    Events(EventsEndpointConfig),
}

impl KeycloakEndpointConfig {
    pub fn from_uri(
        uri: &str,
        server_url: &str,
        allow_internal: bool,
        token_provider: Arc<dyn TokenProvider>,
        http: reqwest::Client,
    ) -> Result<Self, CamelError> {
        // Set allow_internal=true on KeycloakRealmConfig for local development.
        crate::validate_server_url(server_url, allow_internal)?;
        let components = camel_endpoint::parse_uri(uri)?;

        let kind = KeycloakEndpointKind::from_uri_prefix(&components.path).ok_or_else(|| {
            CamelError::InvalidUri(format!(
                "unknown keycloak endpoint kind: '{}'",
                components.path
            ))
        })?;

        match kind {
            KeycloakEndpointKind::Admin => {
                let operation_str = components.params.get("operation").ok_or_else(|| {
                    CamelError::InvalidUri(
                        "keycloak admin endpoint requires 'operation' parameter".into(),
                    )
                })?;

                let operation: AdminOperation = operation_str.parse()?;
                let target_realm = components.params.get("realm").cloned();
                let user_id = components.params.get("userId").cloned();

                Ok(Self::Admin(AdminEndpointConfig {
                    server_url: server_url.to_string(),
                    target_realm,
                    operation,
                    user_id,
                    token_provider,
                    http,
                }))
            }
            KeycloakEndpointKind::Events => {
                let events_config = EventsEndpointConfig::from_params(
                    &components.params,
                    server_url,
                    token_provider,
                    http,
                )?;
                Ok(Self::Events(events_config))
            }
        }
    }
}

pub struct KeycloakEndpoint {
    uri: String,
    config: KeycloakEndpointConfig,
}

impl KeycloakEndpoint {
    pub fn new(uri: String, config: KeycloakEndpointConfig) -> Self {
        Self { uri, config }
    }

    pub fn config(&self) -> &KeycloakEndpointConfig {
        &self.config
    }
}

impl Endpoint for KeycloakEndpoint {
    fn uri(&self) -> &str {
        &self.uri
    }

    fn create_consumer(
        &self,
        rt: Arc<dyn RuntimeObservability>,
    ) -> Result<Box<dyn Consumer>, CamelError> {
        match &self.config {
            KeycloakEndpointConfig::Events(config) => Ok(Box::new(
                crate::keycloak_consumer::KeycloakEventConsumer::new(config.clone(), rt),
            )),
            KeycloakEndpointConfig::Admin(_) => Err(CamelError::EndpointCreationFailed(
                "keycloak admin endpoint does not support consumers".into(),
            )),
        }
    }

    fn create_producer(
        &self,
        rt: Arc<dyn RuntimeObservability>,
        _ctx: &ProducerContext,
    ) -> Result<BoxProcessor, CamelError> {
        match &self.config {
            KeycloakEndpointConfig::Admin(config) => Ok(BoxProcessor::new(
                crate::keycloak_producer::KeycloakAdminProducer::new(config.clone(), rt),
            )),
            KeycloakEndpointConfig::Events(_) => Err(CamelError::EndpointCreationFailed(
                "keycloak events endpoint does not support producers".into(),
            )),
        }
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

    fn mock_provider() -> Arc<dyn TokenProvider> {
        Arc::new(MockTokenProvider)
    }

    fn mock_http() -> reqwest::Client {
        crate::hardened_http_client().expect("hardened client must build")
    }

    #[test]
    fn endpoint_kind_from_prefix_admin() {
        assert_eq!(
            KeycloakEndpointKind::from_uri_prefix("admin"),
            Some(KeycloakEndpointKind::Admin)
        );
    }

    #[test]
    fn endpoint_kind_from_prefix_events() {
        assert_eq!(
            KeycloakEndpointKind::from_uri_prefix("events"),
            Some(KeycloakEndpointKind::Events)
        );
    }

    #[test]
    fn endpoint_kind_from_prefix_unknown() {
        assert_eq!(KeycloakEndpointKind::from_uri_prefix("bogus"), None);
    }

    #[test]
    fn endpoint_config_from_uri_admin_valid() {
        // allow_internal=true opts into localhost for the test environment
        let config = KeycloakEndpointConfig::from_uri(
            "keycloak:admin?operation=createUser&realm=test",
            "http://localhost:8080",
            true,
            mock_provider(),
            mock_http(),
        )
        .unwrap();

        match config {
            KeycloakEndpointConfig::Admin(admin) => {
                assert_eq!(admin.server_url, "http://localhost:8080");
                assert_eq!(admin.target_realm, Some("test".to_string()));
                assert_eq!(admin.operation, AdminOperation::CreateUser);
            }
            KeycloakEndpointConfig::Events(_) => panic!("expected Admin config"),
        }
    }

    #[test]
    fn endpoint_config_from_uri_admin_with_user_id() {
        let config = KeycloakEndpointConfig::from_uri(
            "keycloak:admin?operation=getUser&realm=test&userId=user-123",
            "http://localhost:8080",
            true,
            mock_provider(),
            mock_http(),
        )
        .unwrap();

        match config {
            KeycloakEndpointConfig::Admin(admin) => {
                assert_eq!(admin.user_id, Some("user-123".to_string()));
            }
            KeycloakEndpointConfig::Events(_) => panic!("expected Admin config"),
        }
    }

    #[test]
    fn endpoint_config_from_uri_admin_missing_operation() {
        let result = KeycloakEndpointConfig::from_uri(
            "keycloak:admin?realm=test",
            "http://localhost:8080",
            true,
            mock_provider(),
            mock_http(),
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("operation"));
    }

    #[test]
    fn endpoint_config_from_uri_events_valid() {
        let config = KeycloakEndpointConfig::from_uri(
            "keycloak:events?realm=test&eventType=events",
            "http://localhost:8080",
            true,
            mock_provider(),
            mock_http(),
        )
        .unwrap();

        match config {
            KeycloakEndpointConfig::Events(events) => {
                assert_eq!(events.realm, "test");
            }
            KeycloakEndpointConfig::Admin(_) => panic!("expected Events config"),
        }
    }

    #[test]
    fn endpoint_config_from_uri_events_missing_realm() {
        let result = KeycloakEndpointConfig::from_uri(
            "keycloak:events?eventType=events",
            "http://localhost:8080",
            true,
            mock_provider(),
            mock_http(),
        );
        assert!(result.is_err());
    }

    #[test]
    fn endpoint_config_from_uri_unknown_kind() {
        let result = KeycloakEndpointConfig::from_uri(
            "keycloak:bogus?realm=test",
            "http://localhost:8080",
            true,
            mock_provider(),
            mock_http(),
        );
        assert!(result.is_err());
    }
}
