//! Route security-plan compilation — moved out of
//! [`route_compiler_ext`](super::route_compiler_ext) per the 1k-line rule
//! (bd rc-ivro). Classifies a route's transport, resolves its security
//! provider, and builds the [`RouteSecurityPlan`] the security gate
//! enforces. Classification is fail-closed: a route declaring any security
//! never downgrades to `Public`; a route declaring nothing classifies
//! `Public`.

use std::sync::Arc;

use camel_api::CamelError;
use camel_api::security_policy::{
    AccessMode, AudienceBinding, CredentialSource, RouteSecurityPlan, TransportId,
};
use camel_auth::{ProviderEntry, ProviderRegistry};

use crate::lifecycle::application::route_definition::RouteDefinition;

/// Canonical server-scheme classifier — the ONE place a `from` URI scheme
/// maps to a [`TransportId`] (rc-xmsf). Plan compilation and every
/// transport-set question derive from this; adding a server scheme means
/// editing this match only. The bind gate (`bind_key_from_uri` in
/// `route_controller_trait.rs`) deliberately does NOT use this set — see
/// its comment.
fn scheme_transport(uri: &str) -> Option<TransportId> {
    let scheme = uri
        .split([':', '?'])
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    match scheme.as_str() {
        "http" | "https" => Some(TransportId::Http),
        "ws" | "wss" => Some(TransportId::Ws),
        "grpc" => Some(TransportId::Grpc),
        "mcp" => Some(TransportId::Mcp),
        "wasm" => Some(TransportId::Wasm),
        _ => None,
    }
}

/// Map a route's `from` URI scheme to a [`TransportId`].
///
/// Server transports (`http`, `ws`, `grpc`, `mcp`, `wasm`) map to their
/// canonical transport id. Non-server schemes (e.g. `timer`, `mock`) have no
/// transport semantics and fall back to `Http`, matching the pre-kernel
/// hardcoded default in `SecurityPolicyLayer`.
pub(crate) fn transport_from_uri(uri: &str) -> TransportId {
    scheme_transport(uri).unwrap_or(TransportId::Http)
}

/// Consumer-backed server schemes that receive a [`RouteSecurityPlan`].
/// Everything else (`timer:`, `direct:`, `mock:`, …) is not a server
/// consumer and gets no plan.
fn consumer_transport_from_uri(uri: &str) -> Option<TransportId> {
    scheme_transport(uri)
}

fn transport_name(transport: TransportId) -> &'static str {
    match transport {
        TransportId::Http => "http",
        TransportId::Ws => "ws",
        TransportId::Grpc => "grpc",
        TransportId::Mcp => "mcp",
        TransportId::Wasm => "wasm",
    }
}

/// Credential-source capability per transport.
///
/// - Http allows all four sources.
/// - Ws/Mcp allow `AuthorizationHeader`/`Header`/`Cookie` (no `QueryParam`).
/// - Grpc allows `AuthorizationHeader`/`Header` (no `Cookie`, no `QueryParam`).
/// - Wasm allows all four sources: a `wasm:` source route carries a full
///   HTTP listener, so its capability set matches Http.
fn credential_source_allowed(transport: TransportId, source: &CredentialSource) -> bool {
    match (transport, source) {
        (TransportId::Http, _) | (TransportId::Wasm, _) => true,
        (TransportId::Ws, CredentialSource::QueryParam { .. })
        | (TransportId::Mcp, CredentialSource::QueryParam { .. }) => false,
        (TransportId::Ws, _) | (TransportId::Mcp, _) => true,
        (
            TransportId::Grpc,
            CredentialSource::AuthorizationHeader | CredentialSource::Header { .. },
        ) => true,
        (TransportId::Grpc, _) => false,
    }
}

/// Resolve the plan's provider: a declared name wins (and must exist);
/// otherwise the sole registered provider is used. Zero providers or
/// multiple unnamed providers fail loudly — never a `Public` downgrade.
fn resolve_plan_provider(
    route_id: &str,
    declared: Option<&str>,
    providers: &ProviderRegistry,
) -> Result<(String, Arc<ProviderEntry>), CamelError> {
    let names = providers.names();
    match declared {
        Some(name) => match providers.resolve(name) {
            Some(entry) => Ok((name.to_string(), entry)),
            None => Err(CamelError::RouteError(format!(
                "route '{route_id}' declares security provider '{name}' but it is not registered (available: [{}])",
                names.join(", ")
            ))),
        },
        None => match names.len() {
            0 => Err(CamelError::RouteError(format!(
                "route '{route_id}' declares security but no authentication provider is registered; \
                 register a provider or declare security_policy.provider"
            ))),
            1 => {
                let name = names.into_iter().next().expect("len checked == 1"); // allow-unwrap
                let entry = providers
                    .resolve(&name)
                    .expect("sole provider must resolve"); // allow-unwrap
                Ok((name, entry))
            }
            _ => Err(CamelError::RouteError(format!(
                "route '{route_id}' declares security but multiple providers are registered \
                 ([{}]); declare security_policy.provider to select one",
                names.join(", ")
            ))),
        },
    }
}

/// Compile the [`RouteSecurityPlan`] for a route definition.
///
/// `Ok(None)` means "not a server consumer" (`timer:`, `direct:`, …): the
/// every-server-route invariant applies only to consumer-backed routes the
/// controller starts, so non-consumer schemes skip plan attachment.
///
/// Classification is fail-closed: a route declaring any security (policy,
/// authenticator, or provider name) never downgrades to `Public` — a missing
/// or ambiguous provider is a [`CamelError::RouteError`] naming the route.
/// A route declaring nothing classifies `Public`.
pub fn compile_route_security_plan(
    definition: &RouteDefinition,
    providers: &ProviderRegistry,
) -> Result<Option<RouteSecurityPlan>, CamelError> {
    let Some(transport) = consumer_transport_from_uri(definition.from_uri()) else {
        return Ok(None);
    };
    let route_id = definition.route_id();
    let policy = definition.security_policy_config();
    let declared_provider = definition.security_provider();

    // No security declaration at all → Public (no provider, no extraction).
    if policy.is_none()
        && declared_provider.is_none()
        && definition.security_authenticator().is_none()
    {
        return Ok(Some(RouteSecurityPlan {
            access_mode: AccessMode::Public,
            provider_ref: None,
            transport,
            credential_sources: Vec::new(),
            audience_binding: None,
        }));
    }

    // Provider resolution is uniform across Access modes: declared name wins,
    // else sole provider; zero or ambiguous → error naming the route.
    let (provider_ref, entry) = resolve_plan_provider(route_id, declared_provider, providers)?;

    // Route-level audiences override the provider's audiences; issuers always
    // come from the provider. Absent override → provider binding verbatim.
    let audience_binding = match definition.security_audiences() {
        Some(audiences) => Some(AudienceBinding {
            issuers: entry
                .audience_binding
                .as_ref()
                .map(|b| b.issuers.clone())
                .unwrap_or_default(),
            audiences: audiences.to_vec(),
        }),
        None => entry.audience_binding.clone(),
    };

    let (access_mode, credential_sources) = match policy {
        Some(sp) => (
            AccessMode::Authorized(Arc::clone(&sp.policy)),
            sp.credential_sources.clone(),
        ),
        // Authenticate-only: principal required, no authorization policy.
        // Fail-closed default extraction source (ADR-0033).
        None => (
            AccessMode::Authenticated,
            vec![CredentialSource::AuthorizationHeader],
        ),
    };

    // Capability check: reject sources the transport cannot carry.
    for source in &credential_sources {
        if !credential_source_allowed(transport, source) {
            return Err(CamelError::RouteError(format!(
                "route '{route_id}': credential source {} is not supported on {} transport", // allow-secret
                source.variant_name(),
                transport_name(transport)
            )));
        }
    }

    Ok(Some(RouteSecurityPlan {
        access_mode,
        provider_ref: Some(provider_ref),
        transport,
        credential_sources,
        audience_binding,
    }))
}

/// Task 1.8 — plan compilation tests. The module name IS the test filter:
/// `cargo test -p camel-core compile_route_security`.
#[cfg(test)]
#[path = "route_compiler_ext_tests.rs"]
mod compile_route_security;
