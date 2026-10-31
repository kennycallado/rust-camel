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

use crate::config::AutoescapeMode;
use crate::config::ExternalTemplateLimitsConfig;
use crate::endpoint::TemplateEndpoint;
use crate::root::RootAnchor;
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
    default_autoescape: Option<AutoescapeMode>,
    root: Option<std::path::PathBuf>,
}

impl TemplateComponent {
    /// Build a component from operator-supplied resource-limit configs.
    pub fn new(limits: ExternalTemplateLimitsConfig, render_limits: MinijinjaLimitsConfig) -> Self {
        Self {
            limits,
            render_limits,
            default_autoescape: None,
            root: None,
        }
    }

    /// Set the configured entry default applied when a template has no
    /// top-level `{% autoescape %}` statement.
    ///
    /// Precedence is: an explicit top-level `{% autoescape %}` block wins, then
    /// this configured default applies, and if neither is present compilation
    /// is rejected.
    #[must_use]
    pub fn with_default_autoescape(mut self, mode: AutoescapeMode) -> Self {
        self.default_autoescape = Some(mode);
        self
    }

    /// Set the confinement root that anchors every `template:` endpoint.
    ///
    /// Relative entry URIs are resolved against this root; absolute entries
    /// must land inside it.
    #[must_use]
    pub fn with_root(mut self, root: std::path::PathBuf) -> Self {
        self.root = Some(root);
        self
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
        // A relative entry needs a configured root to resolve against. This
        // must run before `resolve()` so a relative URI combined with an
        // invalid limit still reports the missing root, matching pre-change
        // behavior (the relative URI was rejected at parse time).
        if matches!(config.entry, uri::TemplateEntry::Relative(_)) && self.root.is_none() {
            return Err(CamelError::Config(
                "relative template URI requires [components.template] root to be configured".into(),
            ));
        }
        let root_anchor = self
            .root
            .clone()
            .map(RootAnchor::ConfiguredRoot)
            .unwrap_or(RootAnchor::EntryParent);
        // Fail closed on a zero operator-supplied limit. `resolve()`
        // returns `TemplateReloadError`, which has a `From` impl that maps
        // to `CamelError::TemplateReload(_)` (see `crate::error`).
        let limits = config.limits.resolve()?;
        let route_id = ctx.route_id().unwrap_or("template-endpoint").to_string();
        Ok(Box::new(TemplateEndpoint::new(
            uri_str.to_string(),
            config.entry,
            root_anchor,
            limits,
            self.render_limits.clone(),
            route_id,
            self.default_autoescape,
        )))
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    /// The component must register under the URI scheme `template`.
    /// Anything else breaks the bundle (Task 4.5) and every `to:`
    /// `template:file:///...` route.
    #[test]
    fn component_scheme_is_template() {
        let component = TemplateComponent::default();
        assert_eq!(component.scheme(), "template");
    }

    #[test]
    fn with_root_builder_sets_root() {
        let component = TemplateComponent::new(Default::default(), Default::default())
            .with_root(PathBuf::from("t"));
        assert_eq!(component.root, Some(PathBuf::from("t")));
    }

    #[test]
    fn component_default_has_no_root() {
        let component = TemplateComponent::default();
        assert!(component.root.is_none());
    }

    #[test]
    fn create_endpoint_relative_without_root_is_config_error() {
        let component = TemplateComponent::default();
        let ctx = camel_component_api::NoOpComponentContext;

        let result = component.create_endpoint("template:file:page.html", &ctx);

        match result {
            Err(CamelError::Config(_)) => {}
            Err(other) => panic!("expected CamelError::Config, got {other:?}"),
            Ok(_) => panic!("relative URI without a root must be rejected"),
        }
    }

    /// A relative entry with a configured root constructs directly: endpoint
    /// creation is parse-only, so a non-existent root must NOT be touched
    /// here (filesystem validation happens at lifecycle start).
    #[test]
    fn create_endpoint_relative_with_root_constructs() {
        let component =
            TemplateComponent::default().with_root(PathBuf::from("/tmp/whatever-nonexistent"));
        let ctx = camel_component_api::NoOpComponentContext;

        let result = component.create_endpoint("template:file:page.html", &ctx);

        assert!(
            result.is_ok(),
            "relative entry under a configured root must construct, got: {:?}",
            result.err()
        );
    }

    #[test]
    fn create_endpoint_relative_with_zero_limit_is_config_error() {
        let limits = ExternalTemplateLimitsConfig {
            max_include_count: Some(0),
            ..Default::default()
        };
        let component = TemplateComponent::new(limits, Default::default());
        let ctx = camel_component_api::NoOpComponentContext;

        let result = component.create_endpoint("template:file:page.html", &ctx);

        match result {
            Err(CamelError::Config(message)) => assert_eq!(
                message,
                "relative template URI requires [components.template] root to be configured"
            ),
            Err(other) => panic!("expected CamelError::Config, got {other:?}"),
            Ok(_) => panic!("relative URI without a root must be rejected"),
        }
    }

    #[test]
    fn create_endpoint_absolute_zero_limit_still_bound_error() {
        let limits = ExternalTemplateLimitsConfig {
            max_include_count: Some(0),
            ..Default::default()
        };
        let component = TemplateComponent::new(limits, Default::default());
        let ctx = camel_component_api::NoOpComponentContext;

        let result = component.create_endpoint("template:file:///srv/t/page.html", &ctx);

        match result {
            Err(err) => assert!(
                !matches!(err, CamelError::Config(_)),
                "absolute URI must reach limit validation, got {err:?}"
            ),
            Ok(_) => panic!("zero limit must still be rejected for absolute URIs"),
        }
    }
}
