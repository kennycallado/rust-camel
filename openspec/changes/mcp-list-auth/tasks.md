# Tasks: mcp-list-auth

Single-phase change (design.md has no `## Phases` section). Order: policy
contract → auth opt-ins → registry snapshot → leak reproduction (t1 fails) →
adapter filter → remaining integration tests → ADR amendment → verification.
The reproduction step runs against the snapshot from the registry task, so
the leak (protected name listed) is observed before the filter exists.

**Scenario coverage map** (10 scenarios → owning tasks):

| Spec scenario | Owning task(s) |
| --- | --- |
| unauthenticated list shows only public tools | 3.1 (t1) |
| scoped tool is hidden from an authenticated caller without the scope | 3.1 (t5) |
| authorized caller sees the scoped tool | 3.1 (t2) |
| content-dependent policy is hidden even from an authorized caller | 1.2 (unit), 3.1 (t6) |
| policy-less bind lists everything | 3.1 (t3) |
| list omission does not weaken call denial | 3.1 (t4) |
| resources/list applies the same filter | 3.2 (r1) |
| authorized caller sees the scoped resource | 3.2 (r1) |
| policy-less bind lists every resource | 3.2 (r2) |
| resource list omission does not weaken read denial | 3.2 (r3) |

**Harness facts** (verified against the worktree):

- `McpServerRegistry::global()` is process-global, keyed by bind string.
  `server_tool_dispatch_test.rs` occupies `127.0.0.2`–`.7`; the new tool
  tests use `127.0.0.71`–`.76`. `server_resource_dispatch_test.rs` occupies
  `.21`–`.25`; the new resource tests use `127.0.0.77`–`.79`. The consumer
  wiring test in `server_consumer_test.rs` uses `127.0.0.80`. No sibling file
  uses `.71`–`.80`.
- `handle.security.register_plan(route_id, plan, Some(providers), owner)`
  installs the dispatch plan used by `kernel_authn`. The list path reads the
  plan captured on the entry, so tests register the entry through
  `register_with_plan(..., Some(plan.clone()))` as the consumer does.
- The test authenticator implements `TokenAuthenticator::authenticate_bearer`
  only. With no `audience_binding` the trait's default `authenticate`
  delegates to it (camel-auth `token_authenticator.rs:36-38`). Tokens:
  `sec-token` → scopes `["secret:read"]`, `other-token` → `["other:read"]`.
- The rmcp client's `StreamableHttpClientTransportConfig::auth_header(value)`
  calls reqwest `bearer_auth`, which sends `Authorization: Bearer <value>`
  (rmcp-3.9.0 `transport/common/reqwest/streamable_http_client.rs:224-225`).
- `validate_arguments` runs BEFORE the kernel gate
  (`adapter/server.rs:207-223`): denial tests must send schema-valid
  arguments (`{"id":"42"}` against `id_schema()`), or they fail validation
  and never reach the denial.
- Denial text: `CamelError::Unauthenticated` displays as
  `Unauthenticated: {message}` (`camel-api/src/error.rs:371`); the
  no-credential message is
  `no credential found for route '<route_id>' (credential sources exhausted)`
  (`adapter/server.rs:445-447`). Assert both substrings in the tool-result
  text; the tool denial arrives as an `isError` tool result, NOT a JSON-RPC
  error (`McpToolResult.is_error == true`).
- `assert_no_invocations` already exists in both test files; `try_recv`
  erroring proves zero dispatches.
- `McpBindSecurity::new` becomes `pub(crate)` so the adapter unit tests can
  build a security book without a listener.

## camel-api (policy contract)

### Task 1.1: `SecurityPolicy::is_request_independent` with a fail-closed default

**Files:**
- `crates/camel-api/src/security_policy.rs` (modified)

**Steps:**
1. Extend the `SecurityPolicy` trait with a provided method:
   `fn is_request_independent(&self) -> bool { false }`. Doc comment must
   state: `evaluate` may read the Exchange; this method returns `true` only
   when the decision depends solely on the authenticated principal and
   transport. Catalog discovery (`tools/list`, `resources/list`) may render a
   grant only for `true`; the `false` default is the fail-closed answer for
   policies that read Exchange content. Mention that object safety is
   preserved and that no implementor is forced to change.
2. Do not touch `evaluate`, `AuthContext`, `AuthorizationDecision`, or any
   existing impl.

**Tests:** (in-module `#[cfg(test)]` in `security_policy.rs`)
- `security_policy_default_is_not_request_independent`
  - setup: `struct ContentPolicy;` implementing only `evaluate` (returns
    `AuthorizationDecision::Granted` with a stub principal).
  - action: `let policy: &dyn SecurityPolicy = &ContentPolicy;`
  - assert: `policy.is_request_independent() == false`.
  - command: `cargo test -p camel-api --lib security_policy`
  - expected: FAILS to compile before the method exists (trait has no such
    method); PASSES after.
- Existing `security_policy` unit tests remain green.

**Acceptance:**
- `cargo test -p camel-api --lib security_policy` exits 0.
- `cargo clippy -p camel-api --all-targets -- -D warnings` exits 0.
- `cargo xtask lint-missing-docs-ratchet` exits 0.

- [x] 1.1

## camel-auth (opt-ins)

### Task 1.2: Role/Scope declare independence; permission policy stays content-dependent

**Files:**
- `crates/services/camel-auth/src/built_in.rs` (modified)
- `crates/services/camel-auth/src/permission_policy.rs` (modified — tests only)

**Steps:**
1. In `impl SecurityPolicy for RolePolicy`, add
   `fn is_request_independent(&self) -> bool { true }` with a one-line doc
   comment: the decision reads only `auth.principal()`.
2. Same for `ScopePolicy`.
3. Do NOT override it in `PermissionPolicy`, `WasmSecurityPolicy`, or any
   `Ref`/custom policy — the default `false` is the contract.

**Tests:**
- `role_policy_is_request_independent`: construct
  `RolePolicy::new(vec!["admin".into()], false)` → assert
  `is_request_independent() == true`.
  command: `cargo test -p camel-auth --lib built_in`
- `scope_policy_is_request_independent`: construct
  `ScopePolicy::new(vec!["read".into()], true)` → assert `true`.
  command: `cargo test -p camel-auth --lib built_in`
- `permission_policy_is_not_request_independent`: construct
  `PermissionPolicy` with the existing `GrantEvaluator` test helper (see
  `permission_policy.rs` tests) → assert `is_request_independent() == false`.
  command: `cargo test -p camel-auth --lib permission_policy`
- expected: the first two FAIL to compile/pass before step 1-2; pass after.
  The permission assertion passes with the default from Task 1.1.

**Acceptance:**
- `cargo test -p camel-auth --lib` exits 0.
- `cargo clippy -p camel-auth --all-targets -- -D warnings` exits 0.

- [x] 1.2

## camel-component-mcp (registry snapshot)

### Task 2.1: entry-carried plan + crate-private ready snapshots

**Files:**
- `crates/components/camel-component-mcp/src/registry.rs` (modified)
- `crates/components/camel-component-mcp/src/consumer.rs` (modified)
- `crates/components/camel-component-mcp/tests/server_consumer_test.rs` (modified)

**Steps:**
1. Add `pub plan: Option<RouteSecurityPlan>` to `ToolEntry` and
   `ResourceEntry`, documented as the compiled plan captured at registration
   (catalog classification; `None` = public pass-through registration). Copy
   it in both manual `Clone` impls.
2. Add
   `pub fn register_with_plan(&self, name: String, route_id: String, sender: mpsc::Sender<McpToolInvocation>, input_schema: serde_json::Value, owner: Weak<()>, plan: Option<RouteSecurityPlan>) -> Result<(), McpError>`
   carrying the `register` body and storing `plan`; make the existing
   `register(...)` delegate with `None`. Reword its security note: an entry
   registered through this API carries no plan and is LISTED to every caller;
   dispatch still follows the bind plan book, so a direct-drive route with a
   registered dispatch plan stays authenticated at call time. Document the new
   pub fn (crate-level rustdoc: catalog classification contract, `None` =
   listed to every caller) for `lint-missing-docs-ratchet`.
3. Same for `McpResourceRegistry::register_with_plan(uri, route_id, sender,
   owner, plan)` with `register` delegating `None`.
4. Add `pub(crate) struct ReadyTool { pub name: String, pub input_schema: serde_json::Value, pub route_id: String, pub plan: Option<RouteSecurityPlan> }`
   and `pub(crate) struct ReadyResource { pub uri: String, pub route_id: String, pub plan: Option<RouteSecurityPlan> }`,
   plus `pub(crate) fn ready_tools(&self) -> Vec<ReadyTool>` /
   `ready_resources(&self) -> Vec<ReadyResource>` (prune dead, filter ready,
   clone the plan). Make `list_ready()` delegate to them and keep its exact
   return shape. Document every new item (crate-level rustdoc) for
   `lint-missing-docs-ratchet`. The `route_id`/`plan` fields are unread by
   non-test code until Task 2.2; mark each with
   `#[cfg_attr(not(test), expect(dead_code, reason = "read by the catalog filter in Task 2.2"))]`
   (the `not(test)` guard is required: the unit tests below read the fields,
   so an unconditional `expect` would be unfulfilled under `--all-targets`).
   Task 2.2 step 1 removes the markers.
5. Make `McpBindSecurity::new` `pub(crate)` (adapter unit tests build one).
6. In `consumer.rs` start(), both registration calls use
   `register_with_plan(..., Some(plan.clone()))` (the local `plan` already
   exists at `:239-249` and is cloned for `register_plan`).
7. Do not change dispatch (`resolve`, sender behavior, readiness, caps).

**Tests:** (in-module `#[cfg(test)] mod` in `registry.rs`)
- `ready_snapshot_carries_route_id_and_plan`
  - setup: `McpToolRegistry::new(8)`, live `owner = Arc::new(())`, plan
    `RouteSecurityPlan { access_mode: AccessMode::Public, provider_ref: None,
    transport: TransportId::Mcp, credential_sources: vec![],
    audience_binding: None }`, `register_with_plan("t", "route-t", tx, json!({}), Arc::downgrade(&owner), Some(plan))`,
    `mark_ready("t")`.
  - action: `ready_tools()`.
  - assert: one entry; `route_id == "route-t"`; `plan.is_some()`; access mode
    matches `Public`.
  - command: `cargo test -p camel-component-mcp --lib registry`
- `register_without_plan_carries_none`
  - setup: `register("t", "route-t", tx, json!({}), weak)` + `mark_ready`.
  - assert: `ready_tools()[0].plan.is_none()`.
- `ready_resources_snapshot_carries_route_id_and_plan`: mirror for the
  resource registry.
- `list_ready_still_projects_name_and_schema`: existing projection unchanged
  (one entry; name/schema equal to registration).
- expected: FAIL to compile before steps 1-4; pass after.
- (in `tests/server_consumer_test.rs`)
  `consumer_start_captures_plan_on_tool_and_resource_entries` (bind
  `127.0.0.80:0`, existing helpers only: `server_config`,
  `component_with_server`, `consumer_for`, `test_context`, `security_plan`):
  - setup: start a tool consumer (`mcp:crm/tool/t?schema=...`) and a resource
    consumer (`mcp:crm/resource/r?uri=crm://x`), each with
    `set_security_context(SecurityContext::from_plan(security_plan("plan-a")))`.
  - action: `handle.tool_registry.resolve("t")` and
    `handle.resource_registry.resolve("crm://x")`.
  - assert: each entry's `plan` is `Some`, `access_mode` is
    `AccessMode::Authenticated`, `provider_ref == Some("plan-a".to_string())`
    — proves the production wiring (consumer step 6) is exercised, not just
    the test hand-registration.
  - command: `cargo test -p camel-component-mcp --test server_consumer_test consumer_start_captures_plan`
  - expected: FAILS until step 6 (entries carry `None`), passes after; a
    dropped step 6 cannot stay green.

**Acceptance:**
- `cargo test -p camel-component-mcp --lib registry` exits 0 (unit tests).
- `cargo test -p camel-component-mcp --test server_consumer_test consumer_start_captures_plan`
  exits 0 (production wiring).
- `cargo test -p camel-component-mcp` exits 0 — the existing
  `server_registry_test.rs` / `server_consumer_test.rs` suites that call
  `list_ready()` are unchanged and still green.
- `cargo clippy -p camel-component-mcp --all-targets -- -D warnings` exits 0.

- [x] 2.1

## camel-component-mcp (adapter filter)

### Task 2.2: same kernel path, per-entry visibility, both list handlers

**Files:**
- `crates/components/camel-component-mcp/src/adapter/server.rs` (modified)

**Steps:**
1. **Reproduce the leak before implementing.** With Task 2.1 committed, add
   the Task 3.1 step-1 helpers and step-2 test body (bind `127.0.0.71:0`:
   `open` public, `secret` scoped via `register_with_plan`, both ready) to
   `tests/server_tool_dispatch_test.rs` and run it. It MUST FAIL showing both
   names listed. Keep the test body; step 8 turns it green. Remove the
   `#[cfg_attr(not(test), expect(dead_code, ...))]` markers from Task 2.1
   when the filter below starts reading `route_id`/`plan`.
2. Split the existing `kernel_authn` into:
   - `async fn kernel_authn_for(security: &McpBindSecurity, route_id: &str, plan: &RouteSecurityPlan, headers: &http::HeaderMap) -> KernelAuthn`
     holding the current body after plan resolution (Public short-circuit;
     `providers()`; `extract_token_multi` over normalized headers;
     `kernel_authenticate`; same error strings, including the
     `// allow-secret` marker on the "no credential found" message, which
     moves with the string).
   - `async fn kernel_authn(security, route_id, headers) -> KernelAuthn`
     resolving `security.plan_for(route_id)` (None → `KernelAuthn::Public`)
     and delegating. `call_tool` and `read_resource` keep calling
     `kernel_authn` with no other edit.
3. Add `fn catalog_exchange(headers: &http::HeaderMap) -> Exchange`: build
   `Exchange::new(Message::new(CamelBody::Empty))` and set one message header
   per `mcp_request_headers(headers)` entry
   (`serde_json::Value::String`). Import
   `use camel_api::Body as CamelBody;` — `Body` is already taken by
   `axum::body::Body` at `server.rs:47` (E0252 otherwise).
4. Add
   `async fn catalog_policy_grants(policy: &dyn SecurityPolicy, transport: TransportId, principal: &AuthenticatedPrincipal, headers: &http::HeaderMap) -> bool`:
   return `false` when `!policy.is_request_independent()`; otherwise build
   the exchange, `install_carrier(&mut exchange, principal)`, call
   `policy.evaluate(&mut exchange, &AuthContext { principal, transport })`,
   and return `matches!(result, Ok(AuthorizationDecision::Granted { .. }))`
   (`Denied`, `Err`, and non-exhaustive future variants are `false`).
5. Add an inherent `impl McpServerAdapter` method
   `async fn catalog_entry_visible(&self, route_id: &str, plan: Option<&RouteSecurityPlan>, headers: &http::HeaderMap) -> bool`:
   `None` or `Public` → `true`; otherwise `kernel_authn_for(..., plan, headers)`;
   `Denied` → `false`; `Granted(principal)` → `Authorized(policy)` delegates
   to `catalog_policy_grants`, `Authenticated` → `true`; `Public` outcome →
   `true` (defensive).
6. `list_tools`: rename the `_context` parameter to `context`, compute
   `let headers = normalized_headers(&context);` then iterate
   `self.tools.ready_tools()` in an awaited `for` loop, pushing each entry
   whose `catalog_entry_visible(...)` is `true` into the `Tool` vector.
   Readiness, schema projection, TTL, and cache scope unchanged.
7. `list_resources`: same awaited loop over `ready_resources()` (rename its
   `_context` parameter too).
8. Imports: add `camel_api::Body as CamelBody`, `install_carrier`,
   `AuthContext`, `AuthorizationDecision`, `SecurityPolicy`,
   `RouteSecurityPlan`, `Exchange`, `Message`, `TransportId` as needed; keep
   `AccessMode`. Run the Task 3.1 suite: all filter tests now pass.

**Tests:** (in-module `#[cfg(test)] mod tests` in `server.rs`; define a local
`fn public_plan() -> RouteSecurityPlan` here — the integration helper lives in
another file)
- `catalog_exchange_carries_request_headers`: `HeaderMap` with
  `authorization: Bearer tok` and `x-trace-id: t1` → `catalog_exchange`
  yields a message whose
  `header("authorization") == Some(&json!("Bearer tok"))` and
  `header("x-trace-id") == Some(&json!("t1"))`.
- `catalog_exchange_body_is_empty`: body matches `CamelBody::Empty`.
- `catalog_entry_without_plan_is_visible`: adapter with
  `Arc::new(McpBindSecurity::new())`; `catalog_entry_visible("r", None, &HeaderMap::new())`
  is `true`.
- `catalog_entry_with_public_plan_is_visible`: `Some(&public_plan())` → true.
- `catalog_entry_with_authorized_plan_and_no_provider_snapshot_is_hidden`:
  `Some(&RouteSecurityPlan { access_mode: AccessMode::Authorized(Arc::new(ScopePolicy::new(vec!["secret:read".into()], false))), provider_ref: Some("idp".into()), transport: TransportId::Mcp, credential_sources: vec![CredentialSource::AuthorizationHeader], audience_binding: None })`,
  empty headers, `providers()` is `None` → `false` (fail closed).
- command: `cargo test -p camel-component-mcp --lib adapter::server`
- expected: FAIL to compile before steps 2-5 (methods absent); pass after.
  The step-1 integration test flips from FAIL (both names listed) to PASS.

**Acceptance:**
- `cargo test -p camel-component-mcp --lib` exits 0.
- `cargo clippy -p camel-component-mcp --all-targets -- -D warnings` exits 0.
- No behavior change to `call_tool` / `read_resource` diffs (reviewer checks
  the diff touches only the `kernel_authn` split there).

- [x] 2.2

## camel-component-mcp (integration tests — tools)

### Task 3.1: four required tool tests + two coverage tests

**Files:**
- `crates/components/camel-component-mcp/tests/server_tool_dispatch_test.rs` (modified)

**Steps:**
1. Add helpers at the bottom of the file (test-only; Task 2.2 step 1 adds
   the reproduction subset first, these definitions are the contract):
   - `fn test_principal(scopes: &[&str]) -> camel_api::security_policy::Principal`
     (subject `tester`, issuer `test`, audience `[]`, empty roles,
     `claims: json!({})`).
   - `struct TestAuthenticator { tokens: Vec<(String, Principal)> }`
     implementing `camel_auth::TokenAuthenticator::authenticate_bearer`:
     look the presented token up in `tokens`; hit → `Ok(principal.clone())`,
     miss → `Err(CamelError::Unauthenticated("invalid credential".into()))`;
     `#[async_trait::async_trait]`. One authenticator accepts every fixture
     token, because a bind keeps only the LAST provider snapshot
     (`registry.rs:77-79,119-124`) — never register two single-token
     registries on one bind.
   - `fn provider_registry(tokens: &[(&str, Principal)]) -> Arc<ProviderRegistry>`:
     `ProviderRegistry::new()`, `register("test-idp", ProviderEntry { authenticator: Arc::new(TestAuthenticator{..}), audience_binding: None })`,
     return `Arc`.
   - `fn scoped_plan(scopes: &[&str]) -> RouteSecurityPlan`:
     `AccessMode::Authorized(Arc::new(camel_auth::ScopePolicy::new(scopes.iter().map(|s| s.to_string()).collect(), false)))`,
     `provider_ref: Some("test-idp".into())`, `transport: TransportId::Mcp`,
     `credential_sources: vec![CredentialSource::AuthorizationHeader]`,
     `audience_binding: None`.
   - `fn public_plan() -> RouteSecurityPlan` (Public, no provider, empty
     sources, `TransportId::Mcp`).
   - `fn plain_server_cfg(bind: &str) -> McpServerConfig`
     (`serde_json::from_value(json!({"bind": bind}))`).
   - `async fn spawn_plain_server(bind: &str) -> (SocketAddr, Arc<McpListenerHandle>)`:
     `McpServerRegistry::global().get_or_spawn(bind, &plain_server_cfg(bind))`
     — the existing `spawn_server` hardcodes `server_cfg`, so policy-less
     tests use this helper, not `spawn_server`.
   - `async fn rmcp_client_with_token(addr, token: &str)` — copy `rmcp_client`
     and add `.auth_header(token)` on the transport config.
   - `fn register_tool(handle: &McpListenerHandle, owner: &Arc<()>, name: &str, route: &str, plan: Option<RouteSecurityPlan>, providers: Option<Arc<ProviderRegistry>>) -> mpsc::Receiver<McpToolInvocation>`
     — build the channel, call `register_with_plan(..., plan.clone())` and
     `mark_ready`, and when `plan` is `Some` also call
     `handle.security.register_plan(route, plan.clone(), providers, Arc::downgrade(owner))`;
     return the receiver so the test can assert (no) dispatch.
2. Test `list_tools_hides_scoped_tools_from_unauthenticated_caller` (bind
   `127.0.0.71:0`):
   - setup: listener; tool `open` (Public plan), tool `secret` (scoped
     plan `["secret:read"]`, providers for `sec-token`); both ready.
   - action: `rmcp_client(addr).list_tools(None)` (no credential).
   - assert: names `== ["open"]` (sorted); `secret` absent.
   - expected: FAILS with both names listed before Task 2.2's filter (its
     step 6); PASSES after Task 2.2 completes.
   - command: `cargo test -p camel-component-mcp --test server_tool_dispatch_test list_tools_hides_scoped_tools_from_unauthenticated_caller`
3. Test `list_tools_shows_scoped_tools_to_authorized_caller` (bind
   `127.0.0.72:0`): same catalog; `rmcp_client_with_token(addr, "sec-token")`;
   assert names `== ["open", "secret"]` (sorted).
4. Test `list_tools_policy_less_bind_lists_everything` (bind
   `127.0.0.73:0`): `spawn_plain_server` (NOT `spawn_server`, which hardcodes
   `server_cfg`); two tools via plain `register`
   (no plans) + `mark_ready`; no `register_plan` calls. Assert
   (a) unauthenticated client lists both; (b) a client with
   `auth_header("garbage")` also lists both.
5. Test `list_tools_omission_does_not_weaken_call_denial` (bind
   `127.0.0.74:0`): tool `secret` with scoped plan + providers + a live
   `mpsc::Receiver<McpToolInvocation>`; unauthenticated list lacks `secret`;
   `component_client(addr).call_tool("secret", json!({"id":"42"}))` returns
   `is_error == true` and its text contains `Unauthenticated` and
   `no credential found`; `assert_no_invocations(&mut rx, ...)`.
6. Test `list_tools_hides_scoped_tool_from_authenticated_caller_without_scope`
   (bind `127.0.0.75:0`): providers for `other-token` with scopes
   `["other:read"]`; client with `other-token`; assert `secret` absent,
   `open` present.
7. Test `list_tools_hides_request_dependent_policy_from_authorized_caller`
   (bind `127.0.0.76:0`): `struct EmptyGrantPolicy` implementing
   `SecurityPolicy` — `evaluate` returns `Granted` when
   `matches!(exchange.input.body, CamelBody::Empty)`, else
   `Denied { reason: "request content is not permitted".into(), required: vec![], actual: vec![] }`;
   no `is_request_independent` override (default `false`). Share
   `let policy = Arc::new(EmptyGrantPolicy);` between the route plan
   (`AccessMode::Authorized(policy.clone())`) and the route task.
   (a) Client with `sec-token` → the tool is absent from `tools/list`.
   (b) Call with the SAME authenticated rmcp client — `component_client`
   cannot send credentials (`adapter/client.rs:84-87`):
   `rmcp_client_with_token(addr, "sec-token").call_tool(CallToolRequestParams::new("secret").with_arguments(json!({"id":"42"}).as_object().cloned().expect("object")))`.
   (c) The spawned route task receives the invocation under
   `timeout(Duration::from_secs(2), rx.recv())` (join it with
   `// allow-test-wait: route task join bounded by inner 2s recv timeout` —
   `lint-unbounded-wait` ceiling is 0); assert `invocation.principal` is
   `Some(p)`. Build the pipeline-shaped Exchange
   (`Message::new(CamelBody::Json(json!({"id":"42"})))`),
   `install_carrier(&mut exchange, &p)`, then evaluate the shared `policy` with
   `AuthContext { principal: &p, transport: TransportId::Mcp }`; assert
   `AuthorizationDecision::Denied` and reply the component type
   `McpToolResult { content: json!({"error": reason}), is_error: true }`
   (`McpToolInvocation.reply` is `oneshot::Sender<McpToolResult>`, NOT rmcp's
   `CallToolResult` — E0308 if confused).
   (d) Assert the wire result has `is_error == Some(true)` and read the text
   via `result.content[0].as_text().expect("text content block").text`; it
   contains `request content is not permitted`. Comment that the adapter's
   kernel path granted the valid principal unchanged; the content-dependent
   denial is pipeline-owned, and this test exercises the same policy object at
   that boundary. Add only `CallToolRequestParams` to the `rmcp::model`
   imports (no `CallToolResult`/`ContentBlock` — unused imports fail
   `-D warnings`).

**Acceptance:**
- `cargo test -p camel-component-mcp --test server_tool_dispatch_test` exits 0
  (6 existing + 6 new).
- The four required test names match exactly.
- `cargo clippy -p camel-component-mcp --all-targets -- -D warnings` exits 0.

- [x] 3.1

## camel-component-mcp (integration tests — resources)

### Task 3.2: resource parity tests

**Files:**
- `crates/components/camel-component-mcp/tests/server_resource_dispatch_test.rs` (modified)

**Steps:**
1. Mirror the Task 3.1 helpers this file needs (`test_principal`,
   multi-token `TestAuthenticator`, `provider_registry`,
   `scoped_plan`/`public_plan`, `plain_server_cfg` + `spawn_plain_server`,
   `rmcp_client_with_token`, `register_resource` returning the
   `mpsc::Receiver<McpResourceRead>` and taking
   `Option<RouteSecurityPlan>` + `Option<Arc<ProviderRegistry>>`).
2. Test `list_resources_filters_scoped_resources_by_authentication` (bind
   `127.0.0.77:0`): resource `crm://open` (Public plan), `crm://secret`
   (scoped `["secret:read"]`). ONE provider registry accepts both
   `sec-token` → `["secret:read"]` and `other-token` → `["other:read"]`
   (one registry per bind; the last snapshot wins). Unauthenticated client →
   only `crm://open`; `sec-token` client → both; `other-token` client → only
   `crm://open`.
3. Test `list_resources_policy_less_bind_lists_everything` (bind
   `127.0.0.78:0`): `spawn_plain_server`; two resources via plain `register`
   + `mark_ready`; unauthenticated and garbage-credential clients list both.
4. Test `list_resources_omission_does_not_weaken_read_denial` (bind
   `127.0.0.79:0`): protected resource with a live
   `mpsc::Receiver<McpResourceRead>`; unauthenticated list lacks it;
   `component_client(addr).read_resource("crm://secret")` yields content
   containing `Unauthenticated` and `no credential found`; the resource route
   saw zero reads (`assert_no_invocations`).

**Acceptance:**
- `cargo test -p camel-component-mcp --test server_resource_dispatch_test`
  exits 0 (5 existing + 3 new).
- `cargo clippy -p camel-component-mcp --all-targets -- -D warnings` exits 0.

- [x] 3.2

## docs (ADR amendment)

### Task 4.1: ADR-0060 Rule 8 amendment — discovery filtering

**Files:**
- `docs/adr/0060-mcp-first-class-component.md` (modified)
- `docs/src/components/mcp.md` (modified)
- `crates/components/camel-component-mcp/CONTEXT.md` (modified)
- `crates/components/camel-component-mcp/src/adapter/server.rs` (modified — doc comment only)
- `crates/components/camel-component-mcp/tests/server_resource_dispatch_test.rs` (modified — module doc only)

**Steps:**
1. Append at the end of the file (after the self-grill record, matching the
   repo's amendment convention):
   `## Amendment 2026-11-26 (rc-6ymfr): catalog filtering follows authentication`.
2. Body (English, STE-tight):
   - Rule 8's dispatch-only enforcement is extended: `tools/list` and
     `resources/list` filter the ready catalog through the same per-route
     kernel authentication path dispatch uses (compiled plan, normalized
     header extraction, `kernel_authenticate`).
   - `Public`/plan-less entries stay listed; non-Public entries are listed
     only when the request authenticates against the entry's plan; a plan's
     authorization policy is rendered for discovery only when the policy
     declares itself independent of request content (`RolePolicy`/
     `ScopePolicy`); content-dependent policies stay hidden.
   - No second interpretation of scopes or roles; the `SecurityPolicy` object
     decides.
   - ADR-0061 remains authoritative for bind exposure; the superseded Rule 8
     presence gate is not reinstated.
   - Reference the `mcp-component` requirement "MCP catalog visibility
     follows caller authentication".
3. `docs/src/components/mcp.md` (Server/Consumer security paragraph): after
   "installs the typed principal before the pipeline." add one sentence:
   "`tools/list` and `resources/list` use the same kernel gate: a Public entry
   is listed to every caller, a protected entry only when the caller
   authenticates (and, for request-independent policies, authorization
   grants); an omitted entry stays denied at call time."
4. `crates/components/camel-component-mcp/CONTEXT.md` (Consumer section): add
   one bullet after the resource bullet: "`tools/list` / `resources/list`
   list only what the caller may see: Public entries to every caller,
   protected entries only after kernel authentication (and authorization for
   request-independent policies)."
5. Review-mandated comment pass: in `adapter/server.rs`, tighten the
   `catalog_exchange` doc comment — drop "boundedly content-aware policies"
   and state the Exchange carries the request headers for parity with
   dispatch while `is_request_independent` is the contract guard that a
   discovery-granted policy must not read Exchange content. In
   `tests/server_resource_dispatch_test.rs`, extend the module doc header
   with "and Task 3.2 catalog auth-filtering parity".
6. Do not rewrite existing Rule 8 text (the amendment records the delta).

**Tests:**
- `rg -Fxc '## Amendment 2026-11-26 (rc-6ymfr): catalog filtering follows authentication' docs/adr/0060-mcp-first-class-component.md`
  prints `1` (exact heading, one occurrence; `-F` fixed string, `-x` whole
  line, `-c` count — quoting-safe).
- `cargo xtask lint-context-citations` exits 0 (it scans `CONTEXT.md` /
  `CONTEXT-MAP.md`, so it validates the component CONTEXT.md sentence added in
  step 4; the ADR itself is outside that scan).
- expected: count `0` before steps 1-2; `1` after.

**Acceptance:**
- The amendment section exists and contains the five bullets above.
- The two doc sentences from steps 3-4 are present (exact wording attached to
  the named anchors).
- `git diff --check` clean.
- `cargo xtask lint-context-citations` exits 0.

- [x] 4.1

## verification

### Task 5.1: crate + workspace verification

**Files:** none (verification only).

**Steps:**
1. Run the touched-crate suites (lane-faithful — `--lib` skips `tests/`, rc-2co7z):
   `cargo test -p camel-component-mcp`
   `cargo test -p camel-api`
   `cargo test -p camel-auth`
2. Run the workspace substitutes:
   `cargo test --workspace --lib`
   `cargo test -p camel-core --test hexagonal_architecture_boundaries_test`
3. Run `cargo fmt --check --all` and the full local sweep
   `bash scripts/gates-local.sh` (no `GATES_ONLY` narrowing — it parses the
   AGENTS.md registry and covers the extra clippy lines for excluded crates).
4. Run `openspec validate mcp-list-auth --type change --json` (exit 0).

**Tests:** the commands above; expected all exit 0.

**Acceptance:**
- Every command exits 0.
- The four required test names appear in the
  `server_tool_dispatch_test` run output as `ok`.
- `openspec validate` reports `valid: true`.

- [x] 5.1
