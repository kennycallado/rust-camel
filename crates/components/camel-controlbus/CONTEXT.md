# ControlBus component

This file defines crate-local terms and invariants for the ControlBus EIP. See `../CONTEXT.md` and
`../../camel-api/CONTEXT.md` for shared Component, Exchange, and RuntimeBus terms.

## Language

**ControlBus EIP**:
A Producer-only Endpoint that sends Route lifecycle commands and status queries through the
RuntimeBus. It does not expose a Consumer or an administrative network API.

**`authorizedRoutes`**:
A URI-declared capability allowlist for target Route IDs. Absence denies every command.

**Self-target denial**:
A ControlBus Producer cannot target the Route that owns it. This prevents self-restart denial of
service.

## Authorization invariants

`ControlBusProducer::authorize` applies all three ADR-0034 gates:

1. The Endpoint URI must declare the target `routeId`.
2. `authorizedRoutes` must exist and contain that target.
3. The target must differ from the calling Route ID.

The `CamelRouteId` Exchange header cannot select or override the target. ADR-0032 classifies
Exchange data as untrusted, so only operator configuration can drive this control-plane action.
Authorization failures return `CamelError::Unauthorized`.

## Producer contract

`execute_runtime_action` maps `restart` to `RuntimeCommand::ReloadRoute`. ADR-0004 defines this as
an atomic Pipeline swap without drain semantics. `status` asks `RuntimeQuery::GetRouteStatus` and
sets `Body::Text` to the lifecycle status. Other successful actions set `Body::Empty`.

An unexpected status response or future `RouteAction` variant returns
`CamelError::ProcessorError`. Errors from the RuntimeHandle pass through unchanged. Runtime support
for suspend and resume can vary by component. Status returns only the lifecycle string, not Route
statistics.

## `#[non_exhaustive]` posture

This crate declares no public enums. It consumes the contract enums `RouteAction`,
`RuntimeCommand`, and `CamelError` from `camel-api`. Those enums follow ADR-0049, and this crate
uses fallback match arms for future variants where required.
