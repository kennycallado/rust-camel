//! Host-edge kernel handshake for `wasm:` source routes
//! (`wasm-source-auth-kernel`, Task 2.2).
//!
//! Mirrors the boundary-auth model of the other kernel transports (grpc,
//! ws, http): the raw HTTP request exists only in the axum handler, so the
//! handshake decision runs there — BEFORE the request channel is touched.
//! A denial renders 401 and returns without `tx.send`: the guest never
//! wakes (`accept-http` never observes the request) and the body is never
//! read. Accepted requests keep the 202-immediate-ack semantics unchanged.

use axum::http::{HeaderMap, StatusCode, Uri};

use camel_api::security_policy::AccessMode;
use camel_auth::AuthenticatedPrincipal;

use crate::source_host::WasmSourceKernelAuth;

/// Result of the host-edge handshake for one inbound request.
pub(crate) enum EdgeAuthOutcome {
    /// No handshake required — classification absent (raw construction) or
    /// explicitly `Public`. Forward untouched.
    PassThrough,
    /// Handshake succeeded — forward with the minted principal threaded to
    /// `HttpMeta.principal`. Boxed: the principal dwarfs the unit variant
    /// (clippy::large_enum_variant) and this outcome moves per request.
    Authenticated(Box<AuthenticatedPrincipal>),
}

/// Decide one inbound request at the host edge.
///
/// The decision table is driven by the retained classification
/// (`plan_access`), never by kernel presence alone — `kernel = None` must
/// not conflate `Public` with incomplete wiring:
///
/// - `None` (no context ever set — raw construction) or
///   `Some(Public)` → [`EdgeAuthOutcome::PassThrough`] (no extraction; the
///   bind gate at `start()` still governs exposure).
/// - `Some(non-Public)` with kernel present: extract per the plan's
///   `credential_sources` (headers + URI query + cookie header — the same
///   [`camel_auth::extract_token_multi`] surface camel-http feeds), then
///   mint via [`camel_auth::kernel_authenticate`]. No token, a failed mint,
///   or a provider mismatch → 401.
/// - `Some(non-Public)` with kernel missing (plan-only context): 401 —
///   absent wiring never yields pass-through for non-Public plans
///   (fail-closed).
pub(crate) async fn authenticate_edge(
    plan_access: Option<&AccessMode>,
    kernel: Option<&WasmSourceKernelAuth>,
    headers: &HeaderMap,
    uri: &Uri,
) -> Result<EdgeAuthOutcome, StatusCode> {
    // Pass-through: no classification, or an explicit Public one.
    match plan_access {
        None | Some(AccessMode::Public) => return Ok(EdgeAuthOutcome::PassThrough),
        Some(_) => {}
    }

    // Fail-closed: a non-Public classification without kernel wiring can
    // never mint a principal — deny rather than degrade to Public.
    let Some(kernel) = kernel else {
        // log-policy: handler-owned — the misconfiguration belongs to the
        // route operator, the request sender just sees the denial.
        tracing::warn!("wasm source: non-Public route without auth wiring — denying");
        return Err(StatusCode::UNAUTHORIZED);
    };

    let Some(extracted) =
        camel_auth::extract_token_multi(headers, uri, &kernel.plan.credential_sources)
    else {
        // log-policy: handler-owned — a credential-free request is a client
        // property, not a system fault.
        tracing::warn!("wasm source: no credential found in any permitted source");
        return Err(StatusCode::UNAUTHORIZED);
    };

    match camel_auth::kernel_authenticate(&kernel.plan, &kernel.providers, &extracted).await {
        Ok(principal) => Ok(EdgeAuthOutcome::Authenticated(Box::new(principal))),
        Err(e) => {
            // log-policy: handler-owned — mint failure (bad token or
            // provider mismatch) is a client-visible denial.
            tracing::warn!(error = %e, "wasm source: request authentication failed");
            Err(StatusCode::UNAUTHORIZED)
        }
    }
}
