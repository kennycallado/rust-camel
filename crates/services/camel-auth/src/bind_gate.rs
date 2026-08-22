//! Per-bind public-exposure gate (ADR-0061 Rule 4).
//!
//! Canonical home: `camel-auth`. The gate was born in `camel-core`
//! (`route_controller_trait`, Task 1.9) and moved here in Task 2.6 so the
//! MCP registry can enforce it too: components may not reference
//! `camel_core::` (hexagonal invariant, `xtask lint-component-deps`), while
//! both `camel-core` and the transports already depend on `camel-auth`.
//! `camel-core` re-exports both items from
//! [`crate::lifecycle::adapters::route_controller_trait`], so existing
//! callers (controller, CLI) keep their import paths.

use std::collections::HashMap;

use tracing::warn;

use camel_api::CamelError;
use camel_api::security_policy::{AccessMode, RouteSecurityPlan};

/// Operator acknowledgements for public exposure per bind address
/// (ADR-0061). Plain map — camel-core stays camel-config-free; the CLI
/// builds this from `CamelConfig.binds` and passes it into route staging.
#[derive(Debug, Default, Clone)]
pub struct BindExposureAcks(HashMap<String, bool>);

impl BindExposureAcks {
    pub fn new(map: HashMap<String, bool>) -> Self {
        Self(map)
    }

    /// Whether the operator acknowledged public exposure for `bind`
    /// (bind address string, e.g. `"0.0.0.0:8080"`). Absent → false.
    pub fn acknowledged(&self, bind: &str) -> bool {
        self.0.get(bind).copied().unwrap_or(false)
    }
}

/// Per-bind exposure gate (ADR-0061): a bind exposing `Public` routes on a
/// non-loopback address refuses to start unless the operator acknowledged
/// it. Loopback binds pass without acknowledgement. An acknowledged bind
/// still emits a permanent warning naming the bind and route count —
/// acknowledgement never silences the warning (ADR-0052 rule 3).
///
/// `bind_key` is the canonical key `[binds."<addr>"]` acks use (IP-literal
/// authority or hostname authority as written). Hostname authorities are
/// treated as non-loopback unless the host is `localhost` — no DNS lookup,
/// the decision stays deterministic and fail-closed.
pub fn enforce_bind_exposure_gate(
    bind_key: &str,
    is_loopback: bool,
    plans: &[(&str, &RouteSecurityPlan)],
    acked: bool,
) -> Result<(), CamelError> {
    if is_loopback {
        return Ok(());
    }
    let public_routes: Vec<&str> = plans
        .iter()
        .filter(|(_, plan)| matches!(plan.access_mode, AccessMode::Public))
        .map(|(route_id, _)| *route_id)
        .collect();
    if public_routes.is_empty() {
        return Ok(());
    }
    if acked {
        warn!(
            bind = %bind_key,
            public_routes = public_routes.len(),
            "public (unauthenticated) routes exposed on non-loopback bind per operator acknowledgement"
        );
        Ok(())
    } else {
        Err(CamelError::RouteError(format!(
            "bind {bind_key} exposes {} Public route(s) [{}] on a non-loopback address; \
             acknowledge via [binds.\"{bind_key}\"] allow_public_exposure = true",
            public_routes.len(),
            public_routes.join(", ")
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_api::security_policy::{CredentialSource, TransportId};

    fn public_plan() -> RouteSecurityPlan {
        RouteSecurityPlan {
            access_mode: AccessMode::Public,
            provider_ref: None,
            transport: TransportId::Mcp,
            credential_sources: vec![],
            audience_binding: None,
        }
    }

    fn authenticated_plan() -> RouteSecurityPlan {
        RouteSecurityPlan {
            access_mode: AccessMode::Authenticated,
            provider_ref: Some("idp-a".to_string()),
            transport: TransportId::Mcp,
            credential_sources: vec![CredentialSource::AuthorizationHeader],
            audience_binding: None,
        }
    }

    #[test]
    fn gate_refuses_public_without_ack() {
        let err =
            enforce_bind_exposure_gate("0.0.0.0:8080", false, &[("r1", &public_plan())], false)
                .unwrap_err();
        match err {
            CamelError::RouteError(msg) => {
                assert!(
                    msg.contains("0.0.0.0:8080"),
                    "error must name the bind: {msg}"
                );
                assert!(msg.contains("r1"), "error must name the route: {msg}");
            }
            other => panic!("expected RouteError, got {other}"),
        }
    }

    #[test]
    fn gate_passes_when_acked() {
        assert!(
            enforce_bind_exposure_gate("0.0.0.0:8080", false, &[("r1", &public_plan())], true)
                .is_ok()
        );
    }

    #[test]
    fn gate_passes_loopback_and_non_public_without_ack() {
        assert!(
            enforce_bind_exposure_gate("127.0.0.1:8080", true, &[("r1", &public_plan())], false)
                .is_ok()
        );
        assert!(
            enforce_bind_exposure_gate(
                "10.0.0.1:9000",
                false,
                &[("r1", &authenticated_plan())],
                false
            )
            .is_ok()
        );
    }
}
