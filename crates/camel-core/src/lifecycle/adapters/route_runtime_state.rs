use camel_api::security_policy::{RouteSecurityPlan, SecurityPolicyConfig};
use camel_auth::TokenAuthenticator;
use std::sync::Arc;

/// Immutable-by-nature compilation artifacts for a running route.
///
/// Holds the security artifacts captured at add time. Does NOT hold the
/// pipeline — the runtime pipeline lives in `ManagedRoute.pipeline` as a
/// `SharedPipeline` (`Arc<ArcSwap<PipelineAssembly>>`) so it can be hot-swapped.
pub(crate) struct CompiledRoute {
    pub(crate) security_policy: Option<SecurityPolicyConfig>,
    pub(crate) security_authenticator: Option<Arc<dyn TokenAuthenticator>>,
    /// Named authentication providers, injected into the consumer
    /// `SecurityContext` at start/resume so Phase-2 transports can resolve them.
    pub(crate) provider_registry: Option<Arc<camel_auth::ProviderRegistry>>,
    /// Compiled security plan (Task 1.8). `None` for non-consumer routes
    /// (`timer:`, `direct:`, …) which get no plan; consumer-backed routes
    /// (`http:`, `ws:`, `grpc:`, `mcp:`) always hold one — compilation
    /// failure aborts staging instead of leaving `None`.
    pub(crate) security_plan: Option<RouteSecurityPlan>,
    /// Route-level business error to HTTP status mapping
    /// ([`camel_api::HttpErrorMapping`], business-error-model Task 2.2).
    /// Immutable snapshot captured at staging; the controller reads it on
    /// BOTH the start and the resume registration paths so
    /// `suspend_route → resume_route` preserves the mapping. `Some(_)` marks
    /// a seam route (rest lowering always sets it — review C1).
    pub(crate) http_error_mapping: Option<camel_api::HttpErrorMapping>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compiled_route_holds_security_artifacts() {
        let compiled = CompiledRoute {
            security_policy: None,
            security_authenticator: None,
            provider_registry: None,
            security_plan: None,
            http_error_mapping: None,
        };
        assert!(compiled.security_policy.is_none());
        assert!(compiled.security_authenticator.is_none());
        assert!(compiled.security_plan.is_none());
        assert!(compiled.http_error_mapping.is_none());
    }
}
