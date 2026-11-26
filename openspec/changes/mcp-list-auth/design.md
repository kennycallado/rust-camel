# Design: mcp-list-auth

## Approach

Make the advertisement seam use the **same** gate dispatch already uses, and
remove both ways that gate could be bypassed at list time (missing request
content, stale plan snapshot).

1. **Request-independence contract on `SecurityPolicy`.** `evaluate` may read
   the Exchange, so a policy can grant on an empty Exchange and deny the real
   dispatch. Discovery has no tool arguments and no resource body, and the
   API cannot report whether a grant depended on them. Add an additive trait
   method with a fail-closed default:

   ```rust
   /// True only when `evaluate` depends solely on the authenticated
   /// principal and transport — never on Exchange content. Catalog
   /// discovery may render a grant only for such policies; the default
   /// `false` keeps content-reading policies out of discovery.
   fn is_request_independent(&self) -> bool { false }
   ```

   `RolePolicy` and `ScopePolicy` (principal-only today) override to `true`.
   Permission, WASM, `Ref`, and custom policies keep the `false` default until
   their author opts in. Object safety is preserved (method takes `&self`,
   returns `bool`).

2. **The registry entry carries its plan.** `ToolEntry` / `ResourceEntry` gain
   `plan: Option<RouteSecurityPlan>`, captured at registration. The consumer,
   which already holds the compiled plan (`consumer.rs:233-270`), registers
   through a new `register_with_plan(...)`; the existing `register(...)`
   delegates with `None` and keeps its documented "no plan ⇒ public
   pass-through" meaning. `ready_tools()` / `ready_resources()` return
   `ReadyTool { name, input_schema, route_id, plan }` /
   `ReadyResource { uri, route_id, plan }`; `list_ready()` delegates, so its
   behavior and tests are untouched.

   This removes the stale-snapshot hole: a plan stopped between the registry
   snapshot and the decision cannot turn into "no plan". A route stopped
   before the snapshot is gone from it; a route stopped after it still
   carries its non-Public plan and is filtered accordingly. Entry ownership
   already proves route identity (ADR-0068 winner re-assertion), so the
   carried plan is the entry's plan.

3. **One kernel authentication path.** Split `kernel_authn` into
   `kernel_authn_for(security, route_id, plan, headers)` (the actual resolve →
   extract → `kernel_authenticate` steps) and a thin dispatch wrapper
   `kernel_authn(security, route_id, headers)` that resolves
   `security.plan_for(route_id)` and delegates. `call_tool` and
   `read_resource` keep calling `kernel_authn` unchanged; the list path calls
   `kernel_authn_for` with the snapshot's plan. Same extraction
   (`extract_token_multi` over normalized headers), same minting, same
   fail-closed error text.

4. **Visibility decision per ready entry** (`catalog_entry_visible`):
   - `plan: None` or `AccessMode::Public` → visible.
   - non-Public → `kernel_authn_for`; `Denied` → hidden.
   - `Granted(principal)` with `Authorized(policy)` → if
     `policy.is_request_independent()`, evaluate that same policy object via
     `SecurityPolicy::evaluate` with an `AuthContext` carrying the principal
     and `plan.transport`, on a minimal Exchange with the minted carrier
     installed; `Granted` → visible, `Denied`/error/not-independent → hidden.
   - `Granted(principal)` with `Authenticated` → visible (any valid principal).

   No component-local scope/role reading. The list path performs no route
   dispatch and creates no Exchange that reaches a route; the evaluation
   Exchange is local and dropped.

5. **Adapter wiring.** `list_tools` / `list_resources` compute
   `normalized_headers(&context)` once, filter the registry snapshot through
   `catalog_entry_visible`, and keep readiness gating, TTL/cache metadata, and
   the reply shape unchanged.

6. **Tests** drive the real listener over Streamable HTTP. Per-route plans and
   a test `TokenAuthenticator` are registered on the handle's
   `McpBindSecurity`; entries are registered with their plan; authenticated
   requests use the rmcp client's `auth_header` (adds `Bearer `),
   unauthenticated ones send no header. A policy that grants on an empty
   Exchange but denies real content uses the default `false` independence and
   must stay hidden.

No new config key, no visibility flag, no change to the bind exposure gate,
route compilation, or the caller-visible denial.

## Affected crates

- `camel-api`: additive `SecurityPolicy::is_request_independent` (default
  `false`), doc contract only.
- `camel-auth`: `RolePolicy` / `ScopePolicy` opt in with `true`.
- `camel-component-mcp`: `src/registry.rs` (entry plan + ready snapshots),
  `src/consumer.rs` (register with the plan), `src/adapter/server.rs` (kernel
  split, visibility helper, list wiring), `tests/server_tool_dispatch_test.rs`
  and `tests/server_resource_dispatch_test.rs` (new tests).
- `docs/adr/0060-mcp-first-class-component.md`: rule 8 amendment documenting
  catalog filtering separately from dispatch enforcement; ADR-0061 remains
  authoritative for bind exposure.
- `docs/src/components/mcp.md` and `crates/components/camel-component-mcp/CONTEXT.md`:
  one sentence/bullet each stating the discovery rule (docs alignment).
- `openspec/specs/mcp-component/`: new "catalog visibility" requirement
  (delta in this change).

## Architecture boundaries

- **Components layer.** The MCP change stays inside the component; MCP types
  do not cross into `camel-api`/`camel-component-api` (ADR-0060 rule 9). The
  camel-api addition is a policy-contract method, not an MCP type.
- **ADR-0061 rule 1 (transports extract, the kernel authenticates).** The
  adapter keeps extracting; `kernel_authenticate` keeps minting. Discovery
  adds no second extraction or claims interpretation.
- **Data/control plane.** Discovery never sends an Exchange to a route. The
  evaluation Exchange is local to the decision (headers + carrier, empty
  body) and is never dispatched.
- **Single source of truth for authorization.** The adapter reads no scopes
  or roles; only `SecurityPolicy::evaluate` decides, gated by the policy's own
  request-independence declaration. A new policy variant inherits the
  fail-closed discovery default automatically.

## Alternatives considered

- **Authentication-only filter** (list any non-Public entry to any
  authenticated caller). Rejected: does not meet "an authenticated caller
  sees the tools its scope allows"; a wrong-scope caller would enumerate
  protected names and schemas.
- **Evaluate every policy on an empty Exchange.** Rejected by review: a
  content-reading policy can grant on empty input and deny dispatch; the API
  cannot report the dependence. Replaced by the explicit
  request-independence contract.
- **Parse scopes/roles in the adapter** from the declaration or config.
  Rejected: a second interpretation of the policy.
- **Per-tool `public: true` config flag.** Rejected: new surface duplicating
  the plan; the plan already classifies Public vs non-Public.
- **Resolve the plan for each listed entry from `McpBindSecurity`** (no plan
  in the entry). Rejected by review: a stop between snapshot and lookup can
  yield `None` and expose a protected entry. The entry-carried plan is
  atomic with the registration.
- **Evaluate authorization in the adapter at call time too.** Rejected:
  changes dispatch semantics and duplicates the pipeline's authority; the
  list must follow dispatch, not redefine it.

## Test harness facts

- `McpServerRegistry::global()` is process-global and keyed by bind string;
  each test binds a distinct loopback IP. New binds: `127.0.0.71`–`.80`
  (files in use stop at `.70`; 6 tool tests, 3 resource tests, 1 consumer
  wiring test).
- `handle.security.register_plan(route_id, plan, Some(providers), owner)`
  installs the dispatch plan; `register_with_plan(...)` installs the same
  plan on the entry. Both use the same live `owner` token.
- The test authenticator implements `TokenAuthenticator::authenticate_bearer`
  only; with no audience binding the trait's default `authenticate` delegates
  to it. Principals carry `scopes: ["secret:read"]` for the authorized token.
- Plans under test: `Authorized(ScopePolicy(["secret:read"]))` for the scoped
  entry, `Public` for the advertised entry, no plan for the policy-less
  regression bind, and a content-reading always-grant-on-empty policy
  (`is_request_independent` default) for the fail-closed test.
- Denial tests use schema-valid arguments: `validate_arguments` runs before
  the kernel gate (`server.rs:207-223`), so invalid args would never reach
  the denial being pinned.
