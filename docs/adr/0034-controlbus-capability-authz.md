# ADR-0034: ControlBus capability authorization

**Status:** Proposed
**Date:** 2026-07-03

## Context

The ControlBus component (`controlbus:route`) allows any route to stop,
start, suspend, resume, or restart any other route via exchange headers.
This is an intra-process privilege escalation (R4-H1, CRITICAL): an
adversary controlling untrusted exchange data can target critical routes
(auth gateway, leader election, etc.) or cause a self-restart DoS loop.

ADR-0032 (exchange-data trust boundary) establishes that exchange data
is adversary-controlled. The `CamelRouteId` header is exchange data —
trusting it for route-lifecycle commands violates the trust boundary.

## Decision

1. **Static route declaration:** The target `routeId` MUST be declared
   in the endpoint URI (`controlbus:route?routeId=target&action=stop`).
   The `CamelRouteId` header override is removed entirely.

2. **Authorized-routes allowlist:** The endpoint MUST declare
   `authorizedRoutes` (comma-separated). Only routes in this list can be
   targeted. If `authorizedRoutes` is absent, the endpoint fails closed
   (all commands rejected).

3. **Self-restart denial:** If the target `routeId` equals the calling
   route's ID, the command is rejected. This prevents self-restart DoS.

## Alternatives considered

- **Named capability tokens:** A separate capability registry. Rejected:
  too much ceremony for a single component; the allowlist is simpler and
  auditable.
- **Security-policy extension:** Route-level `SecurityPolicy` grants for
  control operations. Rejected: SecurityPolicy is exchange-level auth,
  not component-configuration-level authz. Mixing the two conflates
  concerns.
- **`allowDynamicRouteId` opt-in:** Permit header override with explicit
  opt-in. Rejected: even with opt-in, the header is untrusted data
  flowing into a control-plane action — the trust boundary should not be
  crossable regardless of opt-in.

## Consequences

- **Breaking change:** Existing configs using `CamelRouteId` header must
  declare `routeId` in the URI + `authorizedRoutes`.
- **`camel doctor` migration** (post-v1.0.0): A future `doctor` release
  will flag every `controlbus:` endpoint missing `authorizedRoutes`.
  Until then, the CHANGELOG section documents the migration.
- **No global disable:** Hardening cannot be turned off wholesale. Each
  `controlbus:` endpoint must declare its authorized targets.
