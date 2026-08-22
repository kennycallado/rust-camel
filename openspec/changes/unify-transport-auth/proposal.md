# Proposal: unify-transport-auth

## Why

Server-component authentication lives in two wiring generations. Gen A
(http, mcp) passes headers into the Exchange and lets the core
`SecurityPolicyLayer` extract, authenticate, and authorize. Gen B (ws,
grpc) receives an injected `SecurityContext` via `set_security_context`
and authenticates at the transport boundary. Both generations work, but
they disagree on where authentication happens, how denial is rendered,
and what happens when no principal reaches the authorization layer.
Four bless rounds on `add-mcp-component` were rejected partly on the
seams this split creates (bd rc-fzgm).

Meanwhile main already ratified parts of the target contract
(`73076dbd`, bd rc-5u38): fail-closed `{{env:}}` placeholders, a named
security-provider registry with per-route `security_policy.provider`,
OIDC JWKS prefetch, N native credentials, and gRPC honoring
`credential_sources`. The framework lacks the one artifact that makes
these coherent: a compiled, per-route security plan with explicit
access modes and an unforgeable principal type.

## What Changes

Three delivery phases (expert-ruled, KB `rc-fzgm-e-opus-reruling`):

- **Phase 1 — Security kernel + per-bind gate.** ADR ratifying the
  shipped contract and reserving types; additive `AccessMode`,
  `AuthenticatedPrincipal` (unforgeable), `RouteSecurityPlan` (with
  audience-binding field reserved); routes declaring security are
  never downgraded — `Public` is the default only for routes that
  declare nothing, guarded by a per-bind exposure gate: non-loopback
  binds serving any `Public` route require operator
  `allow_public_exposure = true` (per-bind, `warn!` at every boot,
  never satisfying misconfigured siblings); pre-policy authentication
  enforcement — `SecurityPolicyLayer` fails closed on missing
  principal; cross-plane denial tests; `SecurityConfigFixture` builder
  for tests under fail-closed placeholders; compiled per-route
  `RouteSecurityPlan` with transport credential-capability validation.
- **Phase 2 — Transport convergence.** gRPC interceptor lifecycle
  repair (ordering — extraction + core invocation, not local authn);
  per-route dispatch enforcement with atomic late-registration
  revalidation; MCP alignment (dispatch-registry plan injection, typed
  `McpTlsConfig`, DSL listener fields flow to runtime mirroring
  `rest:`, RFC 9110 repeated-header normalization, `credential_sources`
  tests); WS convergence to the kernel preserving ADR-0051 redaction;
  http `set_security_context` last; delete duplicate Gen A/Gen B authn
  paths.
- **Phase 3 — Audience enforcement.** Provider registry entries carry
  audience/issuer; authn requests and cache keys include route
  audience + issuer + transport context; cross-transport substitution
  tests.

**Excluded:** per-transport CSRF policy machinery beyond the Cookie
acknowledgment gate already recorded; new credential sources;
WASM-policy changes; Keycloak provider behavior changes.

Affected crates: camel-api, camel-processor, camel-auth, camel-core,
camel-config, camel-dsl, camel-cli, camel-component-api,
camel-component-http, camel-ws, camel-component-grpc,
camel-component-mcp, camel-test. Bd: rc-fzgm (P2).

## Acceptance criteria

- A `RouteSecurityPlan` is compiled for every server route before
  listener construction; declared security is never downgraded to
  `Public`; non-loopback binds serving `Public` routes require
  `allow_public_exposure` and warn at startup.
- `SecurityPolicyLayer` fails closed on missing principal; additive
  role grants never create authentication; spoofed Exchange properties
  never authorize.
- One ADR documents the contract: transport = credential extraction +
  transport-idiomatic denial; core = authentication semantics +
  authorization.
- MCP matches `rest:` listener ownership (DSL fields flow to runtime;
  TOML/DSL conflicts are hard errors, no silent loss); `McpTlsConfig`
  is a typed struct; repeated headers normalize per RFC 9110.
- http, ws, grpc, mcp all consume the same kernel; the duplicate
  generation wiring is deleted.
- Audience/issuer reserved in Phase 1 types; enforced in Phase 3 with
  substitution tests.
- All AGENTS.md quality gates green on scoped crates.

## Risk budget

Acceptable: pre-1.0 breaking config changes (schema uses
`deny_unknown_fields`; audience reservation lands BEFORE any cache-key
ships). Public-by-default ships in the SAME phase as its per-bind
exposure gate — never one without the other (expert-ruled). Not
acceptable: auth bypass at any intermediate commit; each phase merges
green and independently revertable. Test-fixture fragility from
fail-closed placeholders is mitigated only by the sanctioned
`SecurityConfigFixture` builder — no env-optional escape hatch.
