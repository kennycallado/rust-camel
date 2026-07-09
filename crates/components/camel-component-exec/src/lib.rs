//! camel-component-exec — fail-closed system command execution component.

pub mod audit;
pub mod bundle;
pub mod config;
pub mod endpoint;
pub mod error;
pub mod headers;
pub mod policy;
pub mod process;
pub mod producer;

pub use audit::ExecAuditEvent;
pub use bundle::ExecBundle;
pub use config::{ArgPolicy, EnvPolicy, ExecGlobalConfig, ExecProfile, OutputMode, Sandbox};
pub use endpoint::ExecEndpoint;
pub use error::ExecError;
pub use producer::{ExecProducer, ExecResult};

use std::sync::Arc;

use async_trait::async_trait;
use camel_component_api::{CamelError, Component, ComponentContext, Endpoint};

/// Factory for `exec:{profile-name}` endpoints.
///
/// Holds the validated global config (including startup-pinned canonical
/// workspace root) and creates `ExecEndpoint` instances that resolve to
/// the named profile.
pub struct ExecComponent {
    config: Arc<ExecGlobalConfig>,
}

impl ExecComponent {
    /// Create a new ExecComponent from global config.
    ///
    /// The config should already be validated via
    /// `ExecGlobalConfig::validate()` (the `ExecBundle` does this during
    /// `from_toml`).
    pub fn new(config: ExecGlobalConfig) -> Self {
        Self {
            config: Arc::new(config),
        }
    }
}

#[async_trait]
impl Component for ExecComponent {
    fn scheme(&self) -> &str {
        "exec"
    }

    fn create_endpoint(
        &self,
        uri: &str,
        _ctx: &dyn ComponentContext,
    ) -> Result<Box<dyn Endpoint>, CamelError> {
        // URI form: exec:{profile-name}
        let profile_name = uri.strip_prefix("exec:").ok_or_else(|| {
            CamelError::InvalidUri(format!("exec uri must start with 'exec:': {uri}"))
        })?;

        // Strip optional query string
        let profile_name = profile_name
            .split_once('?')
            .map(|(p, _)| p)
            .unwrap_or(profile_name)
            .to_string();

        if profile_name.is_empty() {
            return Err(CamelError::InvalidUri("exec: profile name required".into()));
        }

        // Fail-fast: verify the profile exists at endpoint creation time,
        // not deferred to producer call.
        if self.config.profile(&profile_name).is_none() {
            return Err(CamelError::EndpointCreationFailed(format!(
                "exec profile {profile_name:?} not configured"
            )));
        }

        Ok(Box::new(ExecEndpoint::new(
            uri.to_string(),
            profile_name,
            Arc::clone(&self.config),
        )))
    }
}
