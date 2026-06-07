use std::time::Duration;

use camel_api::CamelError;
use camel_component_api::{Component, ComponentContext, Endpoint, NetworkRetryPolicy, parse_uri};

use crate::config::{MasterComponentConfig, MasterUriConfig};
use crate::endpoint::MasterEndpoint;

pub struct MasterComponent {
    drain_timeout_ms: u64,
    /// Structured reconnection policy for delegate retry.
    reconnect: NetworkRetryPolicy,
}

impl MasterComponent {
    pub fn new(config: MasterComponentConfig) -> Self {
        // Bridge backward-compat field: if delegate_retry_max_attempts is set
        // and the explicit reconnect is still at its default (max_attempts=0),
        // override reconnect.max_attempts from the legacy field.
        let mut reconnect = config.reconnect;
        if let Some(max) = config.delegate_retry_max_attempts
            && reconnect.max_attempts == 0
        {
            reconnect.max_attempts = max;
        }
        Self {
            drain_timeout_ms: config.drain_timeout_ms,
            reconnect,
        }
    }
}

impl Default for MasterComponent {
    fn default() -> Self {
        Self::new(MasterComponentConfig::default())
    }
}

impl Component for MasterComponent {
    fn scheme(&self) -> &str {
        "master"
    }

    fn create_endpoint(
        &self,
        uri: &str,
        ctx: &dyn ComponentContext,
    ) -> Result<Box<dyn Endpoint>, CamelError> {
        let parsed = MasterUriConfig::parse(uri)?;
        let delegate_parts = parse_uri(&parsed.delegate_uri)?;
        let delegate_scheme = delegate_parts.scheme;
        let delegate_component = ctx
            .resolve_component(&delegate_scheme)
            .ok_or_else(|| CamelError::ComponentNotFound(delegate_scheme.clone()))?;

        Ok(Box::new(MasterEndpoint {
            uri: uri.to_string(),
            lock_name: parsed.lock_name,
            delegate_uri: parsed.delegate_uri,
            delegate_component,
            metrics: ctx.metrics(),
            platform_service: ctx.platform_service(),
            drain_timeout: Duration::from_millis(self.drain_timeout_ms),
            reconnect: self.reconnect.clone(),
        }))
    }
}
