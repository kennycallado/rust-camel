//! `template:` scheme Component for the external template component
//! (ADR-0047 Stage 2, Phase 4 / Task 4.3).
//!
//! [`TemplateComponent`] is the factory registered under the URI scheme
//! `template`. It is a thin parse-only adapter: it parses the URI, resolves
//! the operator-supplied resource limits, and constructs a
//! [`TemplateEndpoint`] — it performs NO filesystem access. The actual
//! template acquisition and compilation happen inside the endpoint's
//! lifecycle `start()` (Task 4.4).
//!
//! [`TemplateEndpoint`]: crate::endpoint::TemplateEndpoint

use camel_api::CamelError;
use camel_component_api::{Component, ComponentContext, Endpoint};
use camel_language_api::MinijinjaLimitsConfig;

use crate::config::ExternalTemplateLimitsConfig;
use crate::endpoint::TemplateEndpoint;
use crate::uri;

/// Factory for `template:` scheme Endpoints (ADR-0047 Stage 2).
///
/// Constructible from operator-supplied configs via [`TemplateComponent::new`];
/// `Default` derives `MinijinjaLimitsConfig::default()` per the spec. Used by
/// the bundle (Task 4.5) to register the `template` scheme into a
/// `CamelContext`.
#[derive(Debug, Default)]
pub struct TemplateComponent {
    limits: ExternalTemplateLimitsConfig,
    render_limits: MinijinjaLimitsConfig,
}

impl TemplateComponent {
    /// Build a component from operator-supplied resource-limit configs.
    pub fn new(limits: ExternalTemplateLimitsConfig, render_limits: MinijinjaLimitsConfig) -> Self {
        Self {
            limits,
            render_limits,
        }
    }
}

impl Component for TemplateComponent {
    fn scheme(&self) -> &str {
        "template"
    }

    /// Parse-only endpoint creation. The URI is validated and the
    /// operator-supplied limits are resolved (a zero value fails closed at
    /// this seam), but the filesystem is NOT touched — the heavy lifting
    /// happens in the endpoint's `lifecycle().start()` (Task 4.4).
    fn create_endpoint(
        &self,
        uri_str: &str,
        ctx: &dyn ComponentContext,
    ) -> Result<Box<dyn Endpoint>, CamelError> {
        let config = uri::parse_template_uri(uri_str, self.limits.clone())?;
        // Fail closed on a zero operator-supplied limit. `resolve()`
        // returns `TemplateReloadError`, which has a `From` impl that maps
        // to `CamelError::TemplateReload(_)` (see `crate::error`).
        let limits = config.limits.resolve()?;
        let route_id = ctx.route_id().unwrap_or("template-endpoint").to_string();
        Ok(Box::new(TemplateEndpoint::new(
            uri_str.to_string(),
            config.entry_abs_path,
            limits,
            self.render_limits.clone(),
            route_id,
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The component must register under the URI scheme `template`.
    /// Anything else breaks the bundle (Task 4.5) and every `to:`
    /// `template:file:///...` route.
    #[test]
    fn component_scheme_is_template() {
        let component = TemplateComponent::default();
        assert_eq!(component.scheme(), "template");
    }
}
