# Proposal: mcp-list-auth

## Why

Observer finding 9a (`rc-6ymfr`, p2 security): on a bind that declares a
`security_policy`, `tools/list` and `resources/list` are served without any
authentication. The adapter authenticates `tools/call` and `resources/read`
through the per-route kernel gate (`adapter/server.rs:212-223`, `:292-303`),
but the catalog surfaces (`list_tools` `:167-189`, `list_resources` `:256-274`)
read the registry directly. An unauthenticated caller can therefore enumerate
every tool and resource name, its input schema, and by omission learn which
names are protected. The gate exists, but the advertisement path bypasses it.

ADR-0060 rule 8 and `openspec/specs/mcp-component/spec.md:337` describe the
enforcement gate as route-level dispatch only. The gap is in the contract as
well as the code, so the fix amends both.

## What Changes

- **Catalog visibility follows caller authentication.** `tools/list` and
  `resources/list` filter the ready catalog through the same per-route kernel
  authentication path dispatch uses: a route with no plan or a `Public` plan
  is always listed; a non-Public route is listed only when the request's
  credential authenticates against that route's plan. When the plan carries
  an authorization policy, the entry is listed only when that same policy
  object grants the minted principal **and** the policy declares itself
  independent of request content (new `SecurityPolicy::is_request_independent`,
  default `false`, `true` for `RolePolicy`/`ScopePolicy`). A denial, an
  evaluation error, or a request-content-dependent policy omits the entry
  (fail closed). No component-local re-reading of scopes or roles: the
  `SecurityPolicy` object stays the single interpreter.
- **The registration snapshot carries its plan.** The per-listener registries
  store the compiled `RouteSecurityPlan` with each tool/resource entry
  (registered by the owning consumer). The list path classifies the entry
  from that snapshot, so a route stopped between snapshot and decision can
  never fall back to "no plan ⇒ public" (`registry.rs:190-196`).
- **Policy-less binds are unchanged.** No plans on the bind → every ready
  entry is listed exactly as today.
- **Dispatch denial is unchanged.** `tools/call` / `resources/read` keep the
  existing kernel denial, so hiding from the list never becomes the only
  gate.
- **Contract updates.** A new `mcp-component` requirement (catalog visibility
  follows authentication) and an ADR-0060 rule 8 amendment documenting
  catalog filtering separately from dispatch enforcement; ADR-0061 remains
  authoritative for bind exposure.
- **Registry surface.** The registries expose crate-private ready snapshots
  with the serving `route_id` and plan; existing `list_ready()` behavior is
  preserved.

Explicitly excluded: pagination, caching changes, per-tool visibility flags,
route-policy semantics, and any change to the DSL or route compilation.

## Acceptance criteria

- On a policy-protected bind, an unauthenticated `tools/list` returns only
  Public tools (or an empty list when none is Public); scoped tools are
  absent.
- An authenticated caller with the required scope sees the scoped tool; an
  authenticated caller without it does not.
- A policy that grants on an empty Exchange but denies dispatch content is
  never listed (request-content-dependent policies are hidden).
- A policy-less loopback bind lists every ready tool, credentials or not.
- An unauthenticated `tools/call` for a protected tool omitted from discovery
  returns the existing kernel denial and does not invoke the route. The call
  uses schema-valid arguments.
- `resources/list` applies the same rule; `resources/read` denial is
  unchanged.
- The four required test names exist and pass:
  `list_tools_hides_scoped_tools_from_unauthenticated_caller`,
  `list_tools_shows_scoped_tools_to_authorized_caller`,
  `list_tools_policy_less_bind_lists_everything`,
  `list_tools_omission_does_not_weaken_call_denial`.

## Risk budget

Accepted: one kernel authentication pass per non-Public catalog entry per list
request (the `max_tools`/`max_resources` caps are operator-configurable,
default 128; the provider authn cache is optional and caches grants only, so
repeated denials re-hit the provider); `RolePolicy`/`ScopePolicy` grants are
rendered at list time from the same principal dispatch would use;
request-content-dependent policies (permission, WASM, custom refs) are hidden
from discovery while remaining callable when dispatch grants them. Out of
bounds: inventing a second scope/role interpretation, hiding behind a new
config switch, changing dispatch denial semantics, reviving ADR-0060's
superseded bind-presence gate, or letting a hidden entry become
callable/deniable differently than before.

Affected crates: `camel-component-mcp` (adapter + registry + tests),
`camel-api` (additive `SecurityPolicy::is_request_independent` with a
fail-closed default), `camel-auth` (Role/Scope opt in), plus
`docs/adr/0060-mcp-first-class-component.md` and the `mcp-component` spec
delta.
