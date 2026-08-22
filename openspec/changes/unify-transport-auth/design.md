# Design: unify-transport-auth

## Approach

Split authentication from authorization along one contract line
(expert-ruled, KB `rc-fzgm-e-opus-reruling`):

- **Transport components** extract credentials and render denial in
  the transport idiom (HTTP status, ws close, `tonic::Status`,
  JSON-RPC error). They never implement authentication.
- **Core kernel** owns authentication semantics and authorization.

The kernel artifact is `RouteSecurityPlan`, compiled per route at
staging time:

```text
RouteSecurityPlan = (
    access_mode:       Public | Authenticated | Authorized(Arc<dyn SecurityPolicy>),
    provider_ref:      Option<ProviderId>,   // None only for Public
    credential_sources: Vec<CredentialSource>, // capability-checked per transport
    audience_binding:  Option<AudienceBinding>, // reserved Phase 1, enforced Phase 3
)
```

`security_policy.provider` (landed in 73076dbd) is the authn selector;
access mode is the enforcement predicate — orthogonal axes, jointly
required. `AuthenticatedPrincipal` is a minted, unforgeable type: the
layer checks it, policies receive it, and Exchange properties remain
advisory metadata only. `SecurityPolicyLayer` fails closed when the
principal is absent for non-Public routes; additive role grants never
conjure authentication; explicit denial overrides grants.

Access classification is `Public` by default — but ONLY for routes
that declare no security at all. A route declaring a policy or
provider is never downgraded by missing wiring: classification fails
loudly. The `Public` default is made safe by the per-bind exposure
gate shipping in the SAME phase: a non-loopback bind serving any
`Public` route requires operator `allow_public_exposure = true`
(per-bind TOML acknowledgment; `warn!` at every boot naming the bind
and exposed-route count; never satisfying misconfigured siblings —
any declaring route that fails classification still blocks startup).
Loopback binds permit `Public` silently. Transports extract
credentials and invoke the core authentication path; they never
implement authentication themselves.

Phase 2 repairs the remaining seams: per-route dispatch enforcement
with atomic late-registration revalidation, gRPC interceptor ordering
(plan compiled before interceptor construction; interceptor extracts
and invokes core authn), MCP aligned to `rest:` listener ownership
with typed `McpTlsConfig` and hard DSL/TOML conflict errors, WS
converged onto the kernel preserving ADR-0051 redaction, and http
migrated last. Duplicate Gen A/Gen B wiring is deleted only after
every transport consumes the kernel.

Phase 3 binds audience: provider registry entries carry accepted
issuer/audience; authentication requests and the authentication
cache (`AuthnCache`, distinct from the permission cache) are
provider-local — the authn cache key includes route audience +
issuer + transport context + provider identity, so providers sharing
audience, issuer, and transport still cannot read each other's
entries. The permission cache is untouched. Substitution tests prove
a token minted for one provider's route cannot authenticate
another's (per-provider signature keys; same-issuer fixtures so
issuer rejection cannot mask missing key isolation).

## Affected crates

- `camel-api`: `AccessMode`, `AuthPrincipal` read trait,
  `AuthContext`, `RouteSecurityPlan`, `AudienceBinding`,
  `TransportId` types; `SecurityPolicy::evaluate` receives
  `AuthContext`. The concrete `AuthenticatedPrincipal` lives in
  camel-auth (crate-private construction) so camel-api never names it.
- `camel-auth`: concrete `AuthenticatedPrincipal` (same-crate-only
  construction, zero public constructors); `kernel_authenticate` +
  carrier helpers + `enforce_dispatch`; provider registry gains
  audience/issuer fields (reserved Phase 1) and the Phase-3
  authentication cache slot.
- `camel-processor`: `SecurityPolicyLayer` fail-closed rework.
- `camel-core`: plan compilation in `route_controller`/
`route_compiler_ext`; dual exposure gates; `SecurityContext` rebuilt
as plan carrier.
- `camel-config`: audience/issuer schema in `[security.*]`.
- `camel-dsl` / `camel-cli`: surface plan fields; conflict errors.
- `camel-component-api`: `SecurityContext` carries the plan.
- `camel-component-http`, `camel-ws`, `camel-component-grpc`,
  `camel-component-mcp`: consume the plan; delete generation-local
  authn; MCP gains typed TLS + runtime-flowing listener fields.
- `camel-test`: `SecurityConfigFixture` builder (deterministic test
  secrets under fail-closed `{{env:}}` placeholders).

## Supersession and migration decisions

**ADR-0060 Rule 3 (TOML-owned runtime) — amended by ADR-0061.** The
`mcp:` DSL block becomes the runtime owner of listener configuration
(`bind`, TLS, caps) exactly as `rest:` owns `host`/`port`. TOML
`mcp.servers.<name>` remains the source for items with no DSL
counterpart (`allowed_hosts`) and the two must agree on overlapping
keys — divergence is a hard startup error, never silent TOML-wins.
The step-less catalog declaration (Rule 3's lowering semantics, bd
rc-23y2) is unchanged.

**ADR-0060 Rule 8 (mandatory MCP security_policy) — superseded.** The
presence-only bind gate is replaced by the kernel's per-bind exposure
gate with uniform semantics across all four transports: MCP no longer
carries a component-local gate. A non-loopback MCP bind serving
`Public` routes requires `allow_public_exposure`; the route-level
enforcement gate (headers copied to Exchange, route policy runs
before steps) is preserved as the per-route dispatch enforcement.

**Policy forms without provider.** The `ref`, `wasm`, and
`permission` forms are authorization-only today and reject the
`provider` field. Under the kernel they classify as `Authorized`;
compilation fails when no provider is configured, resolves the sole
provider into the plan's `provider_ref` when exactly one exists, and
fails when multiple providers exist and none is named. Extending
those forms with an optional `provider` field is in-scope for Phase
2 implementation but not required by this change. WASM policy
internals remain out of scope; only their classification changes.

**`rest:` security declaration (symmetry with `mcp:`).** Today
`rest:` lowering hardcodes `security_policy: None` (`rest.rs:357`)
— a REST-declared endpoint cannot be secured at the block level,
while `mcp:` copies its policy to every lowered route. The kernel
fixes the asymmetry: the `rest:` block gains an optional
`security_policy` (same `RouteDslSecurityPolicy` surface, same
load-time validation), and lowering copies it to every lowered
`http:` route exactly as `mcp:` does. Plan compilation then applies
uniformly: a secured `rest:` endpoint classifies `Authorized`, a
bare one stays `Public` under the per-bind exposure gate.

**`trust_upstream_principal` — removed (pre-1.0 breaking).**
Accepting an Exchange-property principal contradicts typed-principal
unforgeability, and no migration path can honor both the spec's
"never authorizes" invariant and the flag's current property-trust
behavior. Phase 1 deletes the property-evidence path AND the DSL
flag (`deny_unknown_fields` makes stale configs fail at load with an
error naming the field): flag-true property-only routes fail closed
from the first Phase-1 commit. The Bearer-token legacy fallback
(dual-read) is unaffected — it authenticates real tokens and is
deleted at the strict-mode task.

## Architecture boundaries

Respects ADR-0032 (data/control plane split): the plan is control
plane, compiled before listeners bind; credentials and principals
cross the boundary as typed values, never raw tokens. Respects the
component contract (ADR-0020-style): components stay metadata-driven;
`set_security_context` becomes plan injection. ADR-0033 fail-closed
posture extends to principal absence. ADR-0059 (extraction-path
divergence) is superseded by this change — documented in the ADR.
Languages and Functions crates are untouched.

## Phases

- **Phase 1 — Security kernel + per-bind gate.** ADR
  `0061-unified-transport-auth`; additive types (audience reserved);
  fail-closed layer + cross-plane denial tests + unforgeability
  tests; `SecurityConfigFixture`; compiled plan with transport
  capability validation; per-bind exposure gate with
  `allow_public_exposure` (Public-by-default and its gate ship
  together or not at all — expert-ruled). Exit criteria: every server
  route has a plan; declared security never downgrades;
  non-loopback Public bind without acknowledgment fails startup;
  missing-wiring test yields DENY; no behavior change for standard
  conformant routes — EXCEPT configurations containing the removed
  `trust_upstream_principal` field, which fail at LOAD with an error
  naming the field (property-only AND token-authenticated routes
  alike — the field is gone; documented pre-1.0 breaking change).
- **Phase 2 — Transport convergence.** Per-route dispatch enforcement
  + atomic late-registration revalidation; gRPC interceptor lifecycle;
  MCP alignment (typed TLS, listener ownership, RFC 9110 header
  normalization, credential_sources tests); WS convergence preserving
  ADR-0051 redaction; http `set_security_context` last; delete
  duplicate generations. Exit criteria: all four transports consume
  the kernel; MCP has no inert DSL fields.
- **Phase 3 — Audience enforcement.** Registry audience/issuer;
  cache-key binding; substitution tests. Exit criteria: token for
  provider A cannot authenticate provider B route; authn cache keys
  include provider identity + audience + issuer + transport
  (identically-configured providers still hold separate entries).
