use std::pin::Pin;
use std::task::{Context, Poll};

use camel_component_api::{Body, CamelError, Exchange, Message};

use crate::admin_endpoint_config::AdminEndpointConfig;

const USER_ID_HEADER: &str = "camel.keycloak.userId";

pub struct KeycloakAdminProducer {
    config: AdminEndpointConfig,
    http: reqwest::Client,
}

impl KeycloakAdminProducer {
    pub fn new(config: AdminEndpointConfig) -> Self {
        Self {
            http: config.http.clone(),
            config,
        }
    }

    async fn execute(&self, exchange: &mut Exchange) -> Result<(), CamelError> {
        let token = self.config.token_provider.get_token().await.map_err(|e| {
            CamelError::ProcessorError(format!("failed to acquire admin auth material: {e}"))
        })?;

        let user_id = self
            .config
            .user_id
            .as_deref()
            .or_else(|| exchange.property(USER_ID_HEADER).and_then(|v| v.as_str()));

        let target_realm = self.config.target_realm.as_deref().unwrap_or("");
        let path = self
            .config
            .operation
            .build_path(target_realm, user_id)
            .map_err(|e| CamelError::ProcessorError(format!("keycloak admin path error: {e}")))?;
        let url = format!("{}{}", self.config.server_url, path);

        let request_body = Self::extract_body(exchange);

        let mut builder = match self.config.operation.http_method() {
            "POST" => self.http.post(&url),
            "GET" => self.http.get(&url),
            "DELETE" => self.http.delete(&url),
            _ => unreachable!(),
        };

        builder = builder.bearer_auth(&token);

        if let Some(body) = request_body {
            builder = builder
                .header("Content-Type", "application/json")
                .body(body);
        }

        let response = builder.send().await.map_err(|e| {
            CamelError::ProcessorError(format!("keycloak admin request failed: {e}"))
        })?;

        let status = response.status();
        if status.is_success() {
            let response_body = response
                .text()
                .await
                .map_err(|e| CamelError::ProcessorError(format!("failed to read response: {e}")))?;
            if response_body.is_empty() {
                exchange.output = Some(Message::new(Body::Empty));
            } else {
                let body = if response_body.starts_with('{') || response_body.starts_with('[') {
                    let val: serde_json::Value =
                        serde_json::from_str(&response_body).map_err(|e| {
                            CamelError::ProcessorError(format!(
                                "failed to parse JSON response: {e}"
                            ))
                        })?;
                    Body::Json(val)
                } else {
                    Body::Text(response_body)
                };
                exchange.output = Some(Message::new(body));
            }
            Ok(())
        } else {
            let body = response.text().await.unwrap_or_default();
            Err(CamelError::ProcessorError(format!(
                "keycloak admin {} failed: {} {}",
                self.config.operation, status, body
            )))
        }
    }

    fn extract_body(exchange: &Exchange) -> Option<String> {
        match &exchange.input.body {
            Body::Text(s) => Some(s.clone()),
            Body::Json(v) => Some(serde_json::to_string(v).unwrap_or_default()),
            _ => None,
        }
    }
}

impl Clone for KeycloakAdminProducer {
    fn clone(&self) -> Self {
        Self {
            config: self.config.clone(),
            http: self.http.clone(),
        }
    }
}

impl tower::Service<Exchange> for KeycloakAdminProducer {
    type Response = Exchange;
    type Error = CamelError;
    type Future =
        Pin<Box<dyn std::future::Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, mut exchange: Exchange) -> Self::Future {
        let producer = self.clone();
        Box::pin(async move {
            producer.execute(&mut exchange).await?;
            Ok(exchange)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_body_from_text() {
        let exchange = Exchange::new(Message::new(Body::Text("hello".to_string())));
        let result = KeycloakAdminProducer::extract_body(&exchange);
        assert_eq!(result, Some("hello".to_string()));
    }

    #[test]
    fn extract_body_from_json() {
        let exchange = Exchange::new(Message::new(Body::Json(serde_json::json!({"key": "val"}))));
        let result = KeycloakAdminProducer::extract_body(&exchange);
        assert_eq!(result, Some(r#"{"key":"val"}"#.to_string()));
    }

    #[test]
    fn extract_body_from_empty() {
        let exchange = Exchange::new(Message::new(Body::Empty));
        let result = KeycloakAdminProducer::extract_body(&exchange);
        assert_eq!(result, None);
    }
}
