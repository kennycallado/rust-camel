use super::*;
use crate::provider::{HealthReport, ProviderError};
use camel_api::function::*;
use std::time::Duration;

pub struct ProtocolClient {
    http: reqwest::Client,
}

impl Default for ProtocolClient {
    fn default() -> Self {
        Self::new()
    }
}

impl ProtocolClient {
    pub fn new() -> Self {
        Self {
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .expect("reqwest client"),
        }
    }

    pub async fn health(&self, endpoint: &str) -> Result<HealthReport, ProviderError> {
        let url = format!("{}/health", endpoint);
        let resp = self.http.get(&url).send().await.map_err(|e| {
            ProviderError::HealthFailed(format!("health request failed: {e}"))
        })?;
        if resp.status().is_success() {
            let health: HealthResponse = resp.json().await.map_err(|e| {
                ProviderError::HealthFailed(format!("decode health response: {e}"))
            })?;
            if health.status == "ready" {
                Ok(HealthReport::Healthy)
            } else {
                Ok(HealthReport::Unhealthy(health.status))
            }
        } else {
            Ok(HealthReport::Unhealthy(format!("status {}", resp.status())))
        }
    }

    pub async fn register(
        &self,
        endpoint: &str,
        def: &FunctionDefinition,
    ) -> Result<(), ProviderError> {
        let url = format!("{}/register", endpoint);
        let body = RegisterRequest {
            function_id: def.id.0.clone(),
            runtime: def.runtime.clone(),
            source: def.source.clone(),
            timeout_ms: def.timeout_ms,
        };
        let resp = self.http.post(&url).json(&body).send().await.map_err(|e| {
            ProviderError::RegisterFailed(format!("register request failed: {e}"))
        })?;
        if resp.status().is_success() {
            Ok(())
        } else {
            let err_resp: ErrorResponse = resp.json().await.map_err(|e| {
                ProviderError::RegisterFailed(format!("decode register error: {e}"))
            })?;
            Err(ProviderError::RegisterFailed(format!("{}: {}", err_resp.kind, err_resp.error)))
        }
    }

    pub async fn invoke(
        &self,
        endpoint: &str,
        id: &FunctionId,
        exchange: &camel_api::Exchange,
        timeout: Duration,
    ) -> Result<InvokeResponse, ProviderError> {
        let url = format!("{}/invoke", endpoint);
        let wire = ExchangeWire {
            function_id: id.0.clone(),
            correlation_id: exchange.correlation_id.clone(),
            body: BodyWire::from_body(&exchange.input.body),
            headers: exchange.input.headers.clone(),
            properties: exchange.properties.clone(),
            timeout_ms: timeout.as_millis() as u64,
        };
        let resp = self.http.post(&url).json(&wire).timeout(timeout).send().await.map_err(|e| {
            ProviderError::InvokeFailed(format!("invoke request failed: {e}"))
        })?;
        let invoke_resp: InvokeResponse = resp.json().await.map_err(|e| {
            ProviderError::InvokeFailed(format!("decode invoke response: {e}"))
        })?;
        Ok(invoke_resp)
    }

    pub async fn shutdown(&self, endpoint: &str) -> Result<(), ProviderError> {
        let url = format!("{}/shutdown", endpoint);
        let resp = self.http.post(&url).send().await.map_err(|e| {
            ProviderError::ShutdownFailed(format!("shutdown request failed: {e}"))
        })?;
        if resp.status().is_success() {
            Ok(())
        } else {
            Err(ProviderError::ShutdownFailed(format!("status {}", resp.status())))
        }
    }
}
