# ADR-0061: Unified Transport Authentication Kernel

**Date:** 2026-08-20
**Status:** Accepted
**Amends:** ADR-0060 (Rule 3 amended, Rule 8 superseded)
**Supersedes:** ADR-0059 (in part: the retained extraction divergence)
**Cross-refs:** ADR-0010, ADR-0032, ADR-0033, ADR-0051, ADR-0052, ADR-0059,
ADR-0060
**Origin:** OpenSpec change `unify-transport-auth` (bd rc-fzgm)

## Context

Server-component authentication lived in two wiring generations. Gen A
(http, mcp) passed headers into the Exchange and let the core
`SecurityPolicyLayer` extract, authenticate, and authorize. Gen B (ws,
grpc) received an injected `SecurityContext` and authenticated at the
transport boundary. Both worked. They disagreed on where authentication
happens, how denial is rendered, and what happens when no principal
reaches the authorization layer.

Main had already ratified parts of the target contract (`73076dbd`,
bd rc-5u38): fail-closed `{{env:}}` placeholders, a named
security-provider registry with per-route `security_policy.provider`,
OIDC JWKS prefetch, N native credentials, and gRPC honoring
`credential_sources`. The framework lacked the one artifact that makes
these coherent. That artifact is a compiled per-route security plan with
explicit access modes and an unforgeable principal type.

This ADR records the decided architecture. Delivery has three phases
(expert-ruled, KB `rc-fzgm-e-opus-reruling`). Phase 1 (kernel types,
fail-closed layer, compiled plan, per-bind exposure gate) has landed.
Phase 2 (transport convergence) and Phase 3 (audience enforcement) are
planned.

## Decision

### Rule 1: Transports extract, the kernel authenticates

Transport components never implement authentication. They extract
credentials per the plan and render denial in the transport idiom (HTTP
status, ws close, `tonic::Status`, JSON-RPC error). The kernel in
`camel-auth` owns authentication semantics and authorization:
`kernel_authenticate` verifies credentials against the registered
provider, `install_carrier` places the typed principal on the Exchange,
and `enforce_dispatch` checks the route binding before dispatch.

This supersedes the divergence ADR-0059 retained (layer-authenticated
HTTP vs boundary-authenticated components). ADR-0059's extraction
standardization (`credential_sources`, first-match-wins source order)
carries forward unchanged as the extraction input.

The plan is control plane (ADR-0032). It compiles before listeners
bind. Credentials and principals cross the boundary as typed values,
never raw tokens.

### Rule 2: Kernel types

`camel-api` (`security_policy.rs`) owns the plan vocabulary:

- `TransportId`: Http, Ws, Grpc, Mcp.
- `AccessMode`: `Public`, `Authenticated`, `Authorized(Arc<dyn
  SecurityPolicy>)`.
- `AuthPrincipal`: read trait over a verified principal.
- `AuthContext`: carries `TransportId` and the principal into
  `SecurityPolicy::evaluate`.
- `RouteSecurityPlan`: compiled per route at staging. Fields: access
  mode, provider ref (`None` only for `Public`), credential sources
  (capability-checked per transport), audience binding.
- `AudienceBinding`: reserved field. Enforcement lands in Phase 3.

The concrete `AuthenticatedPrincipal` lives in `camel-auth`.
`camel-api` never names it.

### Rule 3: Provider selects authentication, access mode decides enforcement

`security_policy.provider` is the authentication selector. The access
mode is the enforcement predicate. The two axes are orthogonal and
jointly required. A route declaring a policy or a provider is never
downgraded to `Public` by missing wiring. Classification fails loudly
instead.

The core `SecurityPolicyLayer` fails closed when the principal is
absent for non-Public routes (ADR-0033 posture extended to principal
absence). Additive role grants never conjure authentication. Explicit
denial overrides grants.

### Rule 4: Public by default, gated per bind

`Public` is the default access mode only for routes that declare no
security at all. The default and its gate ship in the same phase
(expert-ruled additive layering). A non-loopback bind that serves any
`Public` route requires operator acknowledgment:
`allow_public_exposure = true` under `[binds."<bind-address>"]` in
`Camel.toml`. Every boot emits a `warn!` naming the bind and the
exposed-route count. The acknowledgment is permanent. It never silences
the warning (ADR-0052 rule 3). It never satisfies misconfigured
siblings: any declaring route that fails classification still blocks
startup. Loopback binds permit `Public` silently.

### Rule 5: Policy-form provider resolution

Policy forms (`ref`, `wasm`, `permission`) are authorization-only today
and reject the `provider` field. They classify as `Authorized`. Plan
compilation resolves the provider as follows:

- A named provider must resolve in the registry.
- An unnamed form with exactly one registered provider resolves that
  sole provider into the plan.
- An unnamed form with zero providers is a compilation error.
- An unnamed form with more than one provider and no name is a
  compilation error.

The resolution never yields a `Public` downgrade. Extending these forms
with an optional `provider` field is in scope for Phase 2 and not
required by this change.

### Rule 6: `trust_upstream_principal` removed

Removed (pre-1.0 breaking). Accepting an Exchange-property principal
contradicts typed-principal unforgeability. No migration path can honor
both the "never authorizes" invariant and the flag's property-trust
behavior. Phase 1 deletes the property-evidence path and the DSL flag.
Stale configs fail at load with an error naming the field
(`deny_unknown_fields`). Property evidence has no authorization path
from the first Phase-1 commit. The Bearer-token legacy dual-read is
unaffected. It is deleted at the Phase 2 strict-mode task.

### Rule 7: Per-provider isolation (decision now, enforcement Phase 3)

Each provider validates independently. Authentication requests and the
authentication cache (`AuthnCache`, distinct from the permission cache)
are provider-local. The cache key includes route audience, issuer,
transport context, and provider identity. Providers that share
audience, issuer, and transport still hold separate entries.
Substitution tests prove that a token minted for one provider's route
cannot authenticate another's. Fixtures use per-provider signature keys
and the same issuer, so issuer rejection cannot mask missing key
isolation.

### Rule 8: The principal seal (honest statement)

`AuthenticatedPrincipal` construction is same-crate-only (`camel-auth`
`kernel.rs`). The type has zero public constructors, zero feature-gated
constructors, and zero test-only constructors. Cargo feature
unification would make a `cfg(test)` or feature-gated constructor
unsound. Tests mint through the real `kernel_authenticate` path, never
a shortcut.

The seal guards against accidental construction and property spoofing
from every other crate. Against hostile code inside `camel-auth`
itself, the seal holds at review level only. This is the documented
in-process trust boundary: the type system prevents accidents. It does
not sandbox in-process code.

### Rule 9: ADR-0060 amendments

**Rule 3 (TOML-owned runtime), amended.** The `mcp:` DSL block becomes
the runtime owner of listener configuration (`bind`, TLS, caps), as
`rest:` owns `host`/`port`. TOML `mcp.servers.<name>` remains the
source for items with no DSL counterpart (`allowed_hosts`). Overlapping
keys must agree. Divergence is a hard startup error, never silent
TOML-wins. The step-less catalog declaration (Rule 3 lowering
semantics, bd rc-23y2) is unchanged. The `rest:` block gains an
optional `security_policy` with the same surface and the same
load-time validation, so both declaration forms classify under the
same kernel.

**Rule 8 (mandatory MCP `security_policy`), superseded.** The
presence-only bind gate is replaced by the kernel's per-bind exposure
gate with uniform semantics across all four transports. The
component-local MCP gate is removed when Phase 2 converges the MCP
registry onto the kernel gate. The route-level enforcement gate
(headers copied to the Exchange, route policy before steps) is
preserved as per-route dispatch enforcement.

## Consequences

- All four server transports converge on one kernel. Duplicate Gen A /
  Gen B authentication is deleted after convergence (Phase 2).
- Declared security never silently downgrades. Missing wiring fails
  classification, and a non-Public route without a principal denies.
- Non-loopback `Public` exposure requires per-bind acknowledgment and
  warns at every boot, forever.
- Stale `trust_upstream_principal` configs fail at load with an error
  naming the field.
- ADR-0059's divergence decision is superseded. Its extraction
  standardization carries forward.
- ADR-0060 Rule 3 gains DSL listener ownership with hard conflict
  errors. ADR-0060 Rule 8's presence gate is gone.
- `AudienceBinding` is reserved in the types. Its enforcement, the
  provider-local authn cache, and the substitution tests land in
  Phase 3.
- Phase 3 note (2026-08-21): enforcement is live. Per-request
  audience and issuer sets are enforced on every provider
  (REPLACEMENT semantics); the provider-local authn cache keys
  entries by provider, binding, transport, and token hash; and the
  cross-transport substitution E2E suite
  (`crates/camel-test/tests/audience_substitution_test.rs`) pins
  cross-provider, issuer, transport, and audience isolation over
  real http and ws routes.
