# ADR-0052: Diagnostic endpoint exposure posture

**Date:** 2026-08-06
**Status:** Accepted
**Amends:** none
**References:** ADR-0009 (HTTP co-hosting of API and static routes — data plane), ADR-0032 (exchange data trust boundary), ADR-0033 (safe defaults and fail-closed validation at startup), ADR-0051 (credential redaction at diagnostic boundaries)
**Origin:** audit of `camel-prometheus`, finding `F-camel-prometheus-I1` (`FC-METRICS-EXPOSURE`, bd `rc-asm9`); shared surface with `camel-health`.

## Decision

**Diagnostic endpoints** — `/metrics` from `camel-prometheus` and `/healthz`, `/readyz`, `/startupz`, `/health` from `camel-health` — follow the Prometheus scrape model: **unauthenticated by default**, with TLS and authentication as **optional hooks**, and with **loopback bind preferred** by default. Network isolation (NetworkPolicy, firewall) is the operator's responsibility.

A diagnostic endpoint is an HTTP endpoint that exposes operational metadata (route names, error types, traffic volumes, queue depth, circuit-breaker state, liveness and readiness signals) for consumption by observability systems. It is **not** data plane: it does not process business messages and does not cross the ADR-0032 trust boundary.

### Rules

1. **Unauthenticated by convention.** The endpoint mounts no authentication layer by default. This follows the Prometheus scrape model, where the canonical protection is network policy, not application-level authorization. It is not a business-authz gap: operational metadata is not the surface ADR-0010 protects (pre-pipeline route authorization).

2. **TLS and authentication are opt-in hooks.** The crate exposes an extension point to wrap the router with TLS (the `axum_server::tls_rustls` pattern, as in `camel-http`, `camel-grpc`, `camel-ws`) and/or a bearer-token middleware (`axum::middleware::from_fn`). Neither is active by default. An operator who runs on an untrusted network enables them explicitly.

3. **Loopback bind preferred.** The bind default should favor `127.0.0.1`. A bind to a non-loopback interface (`0.0.0.0`) is an explicit operator decision and MUST emit a `warn!` at startup. The warning states that the endpoint is reachable from all interfaces with no application layer restricting it.

4. **Diagnostic metadata carries no credential bytes.** Per ADR-0051, metric bodies and labels and health bodies never leak secrets. This ADR does not relax that rule. Unauthenticated endpoint exposure is acceptable **precisely because** its content is operational metadata, not credential material.

### Scope

This posture binds the service crates that expose diagnostic endpoints (`camel-prometheus`, `camel-health`). It does **not** apply to data-plane components (`camel-http`, `camel-grpc`, `camel-ws`). Those are business inbound and DO mount TLS with certificate hot-reload (see CONTEXT-MAP "TLS cert hot-reload"). The distinction is deliberate: the data plane carries business payload and crosses the trust boundary; the diagnostic plane carries operational metadata and does not.

## Context

`camel-prometheus` builds its axum router with no auth or TLS layer. Its default host is `0.0.0.0:9090` (`crates/camel-config/src/config.rs`, `default_prometheus_host`). `camel-health` shares that surface: its `health_router` mounts `/healthz`, `/readyz`, `/startupz`, `/health` with no auth, no TLS, and no middleware. The config-driven path requires `enabled = true` (default `false`), but the programmatic path (`PrometheusService::new`, as the README Quick Start shows) does not inherit that guard.

No prior ADR governs **how** diagnostic endpoints are exposed. Before we freeze v1.0 we need a recorded decision. Otherwise we ship an information surface with no declared posture. The Prometheus convention (unauthenticated, network isolation owned by the operator) is legitimate and widely adopted. But legitimate is not the same as documented. Without this ADR, a reviewer cannot tell "unauthenticated exposure by design" from "forgot to authenticate".

## Options considered

### Application-level authentication by default

Rejected. It breaks the Prometheus scrape model. Standard scrapers (Prometheus server, agents) expect `/metrics` unauthenticated, or with an auth scheme configured on the scraper side, not imposed by the target. Default auth creates operational friction with no real security benefit when network isolation is already present.

### Mandatory TLS on diagnostic endpoints

Rejected. It imposes TLS termination overhead and certificate management on single-node and development deployments, where the endpoint sits behind loopback or behind a service mesh that already terminates TLS. The untrusted-network case is covered by the opt-in hook (rule 2), not by a global mandate.

### Documented posture with opt-in hooks (chosen)

Chosen. It records the decision (unauthenticated by scrape convention), provides extension points for deployments that need TLS or auth, and prefers loopback bind with a warning on the opt-out. It makes the posture readable in review and leaves the choice to the operator per deployment, with no re-architecture.

## Consequences

- The diagnostic endpoints of `camel-prometheus` and `camel-health` document their unauthenticated posture as a recorded decision, not as an omission.
- The bind default should move to `127.0.0.1`. A non-loopback bind requires explicit opt-in and emits a startup `warn!` (code work, correction stream, bd `rc-asm9`).
- Crates that expose diagnostic endpoints in the future inherit this posture by default and declare any TLS/auth hooks they provide.
- The diagnostic-versus-data-plane distinction is fixed. The data plane mounts TLS with hot-reload (inbound components). The diagnostic plane does not authenticate by convention and offers optional TLS.
- The ADR-0051 redaction rule stays in force. Unauthenticated exposure is acceptable only while the content is operational metadata with no credential bytes.

## Self-grill record

**Questions generated:**

1. [glossary] Does "diagnostic endpoint" collide with the "HTTP co-hosting" of ADR-0009 or with the trust boundary of ADR-0032?
2. [sharpen] Does "unauthenticated by default" contradict ADR-0033 (fail-closed defaults)?
3. [scenario] If an operator binds to `0.0.0.0` on an untrusted network, what protects them under this posture?
4. [cross-ref] Does any existing ADR already cover diagnostic endpoint exposure, so this should be an amendment rather than a new ADR?

**Answers:**

1. [glossary] No collision. ADR-0009 governs the data plane (API routes `http:` plus static mounts `http-static:` that carry business payload and dispatch precedence). ADR-0032 governs untrusted exchange data crossing into control or resource decisions. A diagnostic endpoint processes no business payload and no exchange data. It exposes read-only operational metadata. It is a distinct third category.
2. [sharpen] No contradiction. ADR-0033 fails closed on the security choices the operator MUST declare explicitly (dynamic SQL query, per-world WASM capability, gRPC TLS). Unauthenticated exposure of operational metadata is not one of those choices. The canonical protection of the scrape model is the network, not application auth. What this ADR does adopt from the spirit of ADR-0033 is the preferred loopback bind with an explicit warning on the opt-out to non-loopback: the operator chooses to expose more widely, visibly.
3. [scenario] Under this posture, they are protected by: (a) the preferred loopback bind default, which requires explicit opt-in for `0.0.0.0`; (b) the startup `warn!` that flags the wider exposure; (c) the opt-in TLS or bearer-token hook the operator enables for that case. The posture does not authenticate by default, but it provides the mechanisms and the signal for untrusted-network deployment. The network (NetworkPolicy or firewall) stays the primary defense by scrape convention.
4. [cross-ref] None covers this. ADR-0009 is data plane (API routes plus statics). ADR-0033 is startup validation of config opt-ins, not diagnostic surface exposure. ADR-0051 is credential redaction in representation, and it states explicitly that metrics carry no credentials. The decision is genuinely new: irreversible (v1.0 ships the surface), surprising (an unauthenticated endpoint in a security framework deserves a record), and with a real trade-off (scrape model versus application auth). It is a new ADR, not an amendment.

**Outcome:** approve as new ADR (0052). Unauthenticated posture by scrape convention, TLS/auth as opt-in hooks, loopback bind preferred with a warning on opt-out. Code execution (bind default, warning, hooks) is delegated to the correction stream (bd `rc-asm9`).
**Self-grill mode:** manual (4 L6 principles: consistency with CONTEXT-MAP, conflict with existing ADRs, redundancy with implicit ADRs, correct numbering — 0052 is the next free after 0051).
