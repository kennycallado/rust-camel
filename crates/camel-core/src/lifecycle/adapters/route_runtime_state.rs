use camel_api::security_policy::SecurityPolicyConfig;
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compiled_route_holds_security_artifacts() {
        let compiled = CompiledRoute {
            security_policy: None,
            security_authenticator: None,
        };
        assert!(compiled.security_policy.is_none());
        assert!(compiled.security_authenticator.is_none());
    }
}
