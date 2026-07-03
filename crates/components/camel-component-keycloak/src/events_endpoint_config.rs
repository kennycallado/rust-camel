use std::fmt;
use std::sync::Arc;

use camel_api::CamelError;
use camel_auth::oauth2::TokenProvider;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventType {
    Events,
    AdminEvents,
}

impl EventType {
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Result<Self, CamelError> {
        match s {
            "events" => Ok(Self::Events),
            "admin-events" => Ok(Self::AdminEvents),
            _ => Err(CamelError::InvalidUri(format!(
                "invalid eventType '{}', expected 'events' or 'admin-events'",
                s
            ))),
        }
    }

    pub fn api_path(&self) -> &str {
        match self {
            EventType::Events => "events",
            EventType::AdminEvents => "admin-events",
        }
    }
}

#[derive(Clone)]
pub struct EventsEndpointConfig {
    pub server_url: String,
    pub realm: String,
    pub event_type: EventType,
    pub poll_delay_ms: u64,
    pub max_results: u32,
    pub lookback_window_ms: u64,
    pub dedup_capacity: usize,
    pub max_auth_errors: u32,
    pub type_filter: Option<String>,
    pub client_filter: Option<String>,
    pub operation_types_filter: Option<String>,
    pub resource_path_filter: Option<String>,
    pub token_provider: Arc<dyn TokenProvider>,
    pub http: reqwest::Client,
}

impl fmt::Debug for EventsEndpointConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EventsEndpointConfig")
            .field("server_url", &self.server_url)
            .field("realm", &self.realm)
            .field("event_type", &self.event_type)
            .field("poll_delay_ms", &self.poll_delay_ms)
            .field("max_results", &self.max_results)
            .field("lookback_window_ms", &self.lookback_window_ms)
            .field("dedup_capacity", &self.dedup_capacity)
            .finish_non_exhaustive()
    }
}

impl EventsEndpointConfig {
    pub fn from_params(
        params: &std::collections::HashMap<String, String>,
        server_url: &str,
        token_provider: Arc<dyn TokenProvider>,
        http: reqwest::Client,
    ) -> Result<Self, CamelError> {
        let realm = params.get("realm").ok_or_else(|| {
            CamelError::InvalidUri("keycloak events endpoint requires 'realm' parameter".into())
        })?;
        let event_type_str = params.get("eventType").ok_or_else(|| {
            CamelError::InvalidUri("keycloak events endpoint requires 'eventType' parameter".into())
        })?;
        let event_type = EventType::from_str(event_type_str)?;

        let poll_delay_ms: u64 = params
            .get("pollDelay")
            .and_then(|v| v.parse().ok())
            .unwrap_or(5000);
        let max_results: u32 = params
            .get("maxResults")
            .and_then(|v| v.parse().ok())
            .unwrap_or(100);
        let lookback_window_ms: u64 = params
            .get("lookbackWindow")
            .and_then(|v| v.parse().ok())
            .unwrap_or(300_000);
        let dedup_capacity: usize = params
            .get("dedupCapacity")
            .and_then(|v| v.parse().ok())
            .unwrap_or(10_000);
        let max_auth_errors: u32 = params
            .get("maxAuthErrors")
            .and_then(|v| v.parse().ok())
            .unwrap_or(3);

        Ok(Self {
            server_url: server_url.to_string(),
            realm: realm.clone(),
            event_type,
            poll_delay_ms,
            max_results,
            lookback_window_ms,
            dedup_capacity,
            max_auth_errors,
            type_filter: params.get("type").cloned(),
            client_filter: params.get("client").cloned(),
            operation_types_filter: params.get("operationTypes").cloned(),
            resource_path_filter: params.get("resourcePath").cloned(),
            token_provider,
            http,
        })
    }

    pub fn build_url(&self, date_from: u64) -> String {
        let base = format!(
            "{}/admin/realms/{}/{}?direction=asc&first=0&max={}&dateFrom={}",
            self.server_url.trim_end_matches('/'),
            self.realm,
            self.event_type.api_path(),
            self.max_results,
            date_from,
        );

        let mut url = base;

        if let Some(ref t) = self.type_filter {
            url.push_str(&format!("&type={}", t));
        }
        if let Some(ref c) = self.client_filter {
            url.push_str(&format!("&client={}", c));
        }
        if let Some(ref ot) = self.operation_types_filter {
            url.push_str(&format!("&operationTypes={}", ot));
        }
        if let Some(ref rp) = self.resource_path_filter {
            url.push_str(&format!("&resourcePath={}", rp));
        }

        url
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

    fn mock_http() -> reqwest::Client {
        crate::hardened_http_client().expect("hardened client must build") // allow-unwrap
    }

    fn mock_provider() -> Arc<dyn TokenProvider> {
        Arc::new(MockTokenProvider)
    }

    fn make_params(pairs: &[(&str, &str)]) -> std::collections::HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn event_type_from_str_events() {
        assert_eq!(EventType::from_str("events").unwrap(), EventType::Events);
    }

    #[test]
    fn event_type_from_str_admin_events() {
        assert_eq!(
            EventType::from_str("admin-events").unwrap(),
            EventType::AdminEvents
        );
    }

    #[test]
    fn event_type_from_str_invalid() {
        assert!(EventType::from_str("bogus").is_err());
    }

    #[test]
    fn events_config_from_params_user_events() {
        let params = make_params(&[("realm", "my-realm"), ("eventType", "events")]);
        let config = EventsEndpointConfig::from_params(
            &params,
            "http://kc:8080",
            mock_provider(),
            mock_http(),
        )
        .unwrap();
        assert_eq!(config.realm, "my-realm");
        assert_eq!(config.event_type, EventType::Events);
        assert_eq!(config.poll_delay_ms, 5000);
        assert_eq!(config.max_results, 100);
        assert_eq!(config.lookback_window_ms, 300_000);
        assert_eq!(config.dedup_capacity, 10_000);
        assert_eq!(config.max_auth_errors, 3);
    }

    #[test]
    fn events_config_from_params_admin_events_with_overrides() {
        let params = make_params(&[
            ("realm", "master"),
            ("eventType", "admin-events"),
            ("pollDelay", "10000"),
            ("maxResults", "50"),
            ("lookbackWindow", "600000"),
            ("dedupCapacity", "5000"),
            ("maxAuthErrors", "5"),
            ("operationTypes", "CREATE,DELETE"),
        ]);
        let config = EventsEndpointConfig::from_params(
            &params,
            "http://kc:8080",
            mock_provider(),
            mock_http(),
        )
        .unwrap();
        assert_eq!(config.event_type, EventType::AdminEvents);
        assert_eq!(config.poll_delay_ms, 10000);
        assert_eq!(config.max_results, 50);
        assert_eq!(config.lookback_window_ms, 600_000);
        assert_eq!(config.dedup_capacity, 5000);
        assert_eq!(config.max_auth_errors, 5);
        assert_eq!(
            config.operation_types_filter.as_deref(),
            Some("CREATE,DELETE")
        );
    }

    #[test]
    fn events_config_missing_realm() {
        let params = make_params(&[("eventType", "events")]);
        let result = EventsEndpointConfig::from_params(
            &params,
            "http://kc:8080",
            mock_provider(),
            mock_http(),
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("realm"));
    }

    #[test]
    fn events_config_missing_event_type() {
        let params = make_params(&[("realm", "test")]);
        let result = EventsEndpointConfig::from_params(
            &params,
            "http://kc:8080",
            mock_provider(),
            mock_http(),
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("eventType"));
    }

    #[test]
    fn build_url_user_events() {
        let params = make_params(&[("realm", "test"), ("eventType", "events")]);
        let config = EventsEndpointConfig::from_params(
            &params,
            "http://kc:8080",
            mock_provider(),
            mock_http(),
        )
        .unwrap();
        let url = config.build_url(1700000000000);
        assert!(url.contains("/admin/realms/test/events?"));
        assert!(url.contains("direction=asc"));
        assert!(url.contains("first=0"));
        assert!(url.contains("max=100"));
        assert!(url.contains("dateFrom=1700000000000"));
    }

    #[test]
    fn build_url_admin_events_with_filters() {
        let params = make_params(&[
            ("realm", "master"),
            ("eventType", "admin-events"),
            ("operationTypes", "CREATE"),
            ("resourcePath", "users"),
        ]);
        let config = EventsEndpointConfig::from_params(
            &params,
            "http://kc:8080",
            mock_provider(),
            mock_http(),
        )
        .unwrap();
        let url = config.build_url(0);
        assert!(url.contains("/admin/realms/master/admin-events?"));
        assert!(url.contains("operationTypes=CREATE"));
        assert!(url.contains("resourcePath=users"));
    }
}
