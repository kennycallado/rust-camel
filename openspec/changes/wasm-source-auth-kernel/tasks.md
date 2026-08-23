# Tasks: wasm-source-auth-kernel

## Phase 1: Security wiring + transport enumeration + bind governance

### camel-api

#### Task 1.1: Add `TransportId::Wasm` variant

**Files:**
- `crates/camel-api/src/security_policy.rs` (modified)

**Steps:**
1. Add `Wasm` variant to `pub enum TransportId` (security_policy.rs:86), after `Mcp`.
2. Update the enum doc comment: the closed set now reads `http:`, `ws:`, `grpc:`, `mcp:`, and `wasm:` (source routes hosting an http-listener).
3. Update the in-crate exhaustive test (security_policy.rs:609, currently `transport_id_derives_all_four` — rename to `transport_id_derives_all` to reflect five variants) to cover `TransportId::Wasm => "wasm"` and assert it.
4. Fix any other in-crate exhaustive matches the new variant breaks (compiler-driven; keep arms trivial).

**Tests:**
- `transport_id_wasm_name`: enum has 5 variants → `name(TransportId::Wasm)` returns `"wasm"` → assert_eq passes. Command: `cargo test -p camel-api --lib transport_id`. Expected: fails before step 1-3 (no Wasm variant), passes after.

**Acceptance:**
- `cargo test -p camel-api --lib` exits 0.
- `cargo clippy -p camel-api -- -D warnings` exits 0.

- [x] 1.1

### camel-core

#### Task 1.2: Classify `wasm:` routes and deliver plans to server-route consumers

**Files:**
- `crates/camel-core/src/lifecycle/adapters/route_compiler_ext.rs` (modified)
- `crates/camel-core/src/lifecycle/adapters/route_compiler_ext_tests.rs` (modified)
- `crates/camel-core/src/lifecycle/adapters/route_controller_trait.rs` (modified)
- `crates/components/camel-component-api/src/consumer.rs` (modified — `SecurityContext` home; adjust if it lives in a sibling module)
- `crates/camel-core/src/lifecycle/adapters/route_controller_trait_tests.rs` (modified)

**Steps:**
1. In the total scheme→`TransportId` mapping (route_compiler_ext.rs:171, `transport_from_uri`, falls back to `Http`) add `"wasm" => TransportId::Wasm`.
2. In the `Option`-returning mapping (route_compiler_ext.rs:189, `consumer_transport_from_uri` — the form `compile_route_security_plan` uses) add `"wasm" => Some(TransportId::Wasm)`.
3. In the exhaustive `transport_name` match (route_compiler_ext.rs:195-202) add `TransportId::Wasm => "wasm"`.
4. In the exhaustive `credential_source_allowed` match (route_compiler_ext.rs:209-224) add `(TransportId::Wasm, _) => true` — `wasm:` source routes carry a full HTTP listener, so all four `CredentialSource` variants (AuthorizationHeader, Header, Cookie, QueryParam) are permitted (blessed spec: MODIFIED "Transport credential capability validation").
5. Make plan-only contexts representable: change `SecurityContext.policy` from `Arc<dyn SecurityPolicy>` to `Option<Arc<dyn SecurityPolicy>>` (camel-component-api — pre-1.0 breaking is acceptable), and add `pub fn from_plan(plan: RouteSecurityPlan) -> Self` (policy None, providers None, empty credential sources). Compatibility sweep is compiler-driven: `from_arc` and every builder set `Some(...)`; every `ctx.policy` read becomes `ctx.policy.as_ref()`/`as_deref()` (http/ws/grpc/mcp transports read it — mechanical `.as_ref()` insertion, no behavior change since all existing construction paths keep policy Some).
6. Controller classification delivery — at BOTH injection sites in `route_controller_trait.rs` (:303 and :710, start + late-registration paths): after the existing `if let (Some(sp_config), Some(_))` block, add an else-if branch: when `managed.compiled.security_plan` is `Some(plan)` and the sp_config path did not run, call `consumer.set_security_context(SecurityContext::from_plan(plan.clone()))` (attach providers via `.with_providers(...)` when `provider_registry` is present). This delivers every staged server route's classification to its consumer.
7. Tests in `route_compiler_ext_tests.rs` following the existing grpc/mcp pattern (direct `RouteDefinition::new("wasm://...")` + `compile_route_security_plan`, no full staging) and in `route_controller_trait_tests.rs` for the injection extension.

**Tests:**
- `wasm_route_with_security_classifies`: `RouteDefinition::new("wasm://guest-fixture.wasm?bind=127.0.0.1:0")` with a security policy → `compile_route_security_plan` returns a plan (not rejected, not downgraded to Public). Command: `cargo test -p camel-core --lib wasm_route_with_security`.
- `wasm_route_without_security_stays_public`: same route minus the policy → staging succeeds with Public default. Command: `cargo test -p camel-core --lib wasm_route_without_security`.
- `wasm_transport_name_and_all_sources_allowed`: `transport_name(TransportId::Wasm) == "wasm"` and `credential_source_allowed` returns true for all four variants. Command: `cargo test -p camel-core --lib wasm_transport_name`.
- `undeclared_server_route_receives_plan_only_context` (route_controller_trait_tests.rs): stage a server route without security policy (use an http: route for the shared path) → the consumer's set_security_context was invoked with a context whose plan is the compiled Public plan and no providers. Command: `cargo test -p camel-core --lib undeclared_server_route_receives_plan`.
- `declared_route_context_unchanged` (route_controller_trait_tests.rs): route with policy + provider → context carries plan + providers exactly as before the change (regression guard). Command: `cargo test -p camel-core --lib declared_route_context_unchanged`.

**Acceptance:**
- `cargo test -p camel-core --lib` exits 0.
- `cargo test -p camel-component-api --lib` exits 0.
- `cargo test -p camel-processor --lib` exits 0 (policy-field compat sweep compiles).
- `cargo clippy -p camel-core -p camel-component-api -p camel-processor -- -D warnings` exits 0.

- [x] 1.2

### camel-component-wasm

#### Task 1.3: Capture SecurityContext as `WasmSourceKernelAuth`

**Files:**
- `crates/components/camel-component-wasm/src/source_host.rs` (modified)
- `crates/components/camel-component-wasm/src/source_consumer.rs` (modified)
- `crates/components/camel-component-wasm/src/security_policy.rs` (modified)
- `crates/components/camel-component-wasm/tests/source_auth.rs` (new)

**Steps:**
1. Define `pub(crate) struct WasmSourceKernelAuth { pub(crate) plan: RouteSecurityPlan, pub(crate) providers: Arc<ProviderRegistry> }` in `source_host.rs`, modeled on `GrpcKernelAuth` (camel-component-grpc/src/server.rs:51-72), with `pub(crate) fn from_security_context(ctx: &SecurityContext) -> Option<Self>` returning `None` unless both plan and providers are present.
2. Add field `kernel: Option<Arc<WasmSourceKernelAuth>>` AND field `plan: Option<RouteSecurityPlan>` to `WasmSourceConsumer` (source_consumer.rs), both defaulting to `None` in `WasmSourceConsumer::new`. `plan` retains a CLONE of the whole classification plan from `ctx.plan` (not just the access mode — the full plan is needed for the bind-gate snapshot in Task 1.4); `plan_access` used by later tasks derives as `self.plan.as_ref().map(|p| p.access_mode.clone())` (`AccessMode` is not `Copy` — clone from the shared reference).
3. Implement `Consumer::set_security_context` override on `WasmSourceConsumer` (the no-op default lives in camel-component-api `consumer.rs`): set `self.plan = ctx.plan.clone()`; then store `WasmSourceKernelAuth::from_security_context(&ctx)` into `self.kernel` (None without providers). Compile-time credential-source capability for `wasm:` is enforced centrally in `credential_source_allowed` (Task 1.2) — do NOT re-validate here.
4. Add a `#[doc(hidden)] pub fn plan_access_mode(&self) -> Option<AccessMode>` test accessor on `WasmSourceConsumer` (integration tests are off-crate and cannot read `pub(crate)` fields).
5. Fix the exhaustive `TransportId` match in `crates/components/camel-component-wasm/src/security_policy.rs:50-55`: add the `Wasm` arm mapping to `"wasm"` (same string as `transport_name`).
6. New integration test file `tests/source_auth.rs`; for the SecurityContext-with-providers fixture, follow the grpc precedent (`start_secured_consumer` in camel-component-grpc tests) or ws `kernel_auth_test.rs` — NOT `tests/security_policy.rs`, which has no SecurityContext/ProviderRegistry helpers.

**Tests:**
- `set_security_context_captures_kernel`: build `WasmSourceConsumer`, call `set_security_context` with a context carrying plan + providers (grpc-precedent fixture) → `plan_access_mode()` returns the plan's mode. Command: `cargo test -p camel-component-wasm --test source_auth set_security_context_captures`.
- `set_security_context_none_without_plan`: same consumer, context missing plan → `plan_access_mode()` returns `None`. Command: `cargo test -p camel-component-wasm --test source_auth set_security_context_none_without_plan`.
- `plan_only_context_captures_classification_without_kernel`: context with a non-Public plan but NO providers (plan-only, via `SecurityContext::from_plan`) → `plan_access_mode()` returns the non-Public mode (classification captured) while the handshake stays unwired. Command: `cargo test -p camel-component-wasm --test source_auth plan_only_context`.

**Acceptance:**
- `cargo test -p camel-component-wasm --test source_auth` exits 0.
- `cargo clippy -p camel-component-wasm -- -D warnings` exits 0.

- [x] 1.3

#### Task 1.4: Bind governance — precedence, conflict failure, exposure gate

**Files:**
- `crates/components/camel-component-wasm/src/source_consumer.rs` (modified)
- `crates/components/camel-component-wasm/src/endpoint.rs` (modified)
- `crates/components/camel-component-wasm/src/lib.rs` (modified)
- `crates/components/camel-component-wasm/tests/source_integration.rs` (modified — constructor call sites)
- `crates/components/camel-component-wasm/tests/source_stream_integration.rs` (modified — constructor call sites)
- `crates/components/camel-component-wasm/tests/fixtures/conflicting-bind-guest/` (new guest fixture: `Cargo.toml`, `src/lib.rs`)
- `crates/components/camel-component-wasm/tests/source_bind_gate.rs` (new)
- `scripts/xtask/allowlist-ignore.txt` (modified)
- `.github/workflows/ci.yml` (modified)

**Steps:**
1. Plumb the endpoint URI into the consumer: `WasmSourceConsumer` gains field `endpoint_uri: String`; `WasmSourceConsumer::new` gains a `uri: impl Into<String>` parameter (or the component factory in `endpoint.rs` — which already parses the full `wasm:...` URI when constructing the consumer — passes it); update EVERY constructor call site: the component factory in `endpoint.rs`, the in-crate tests (e.g. `stop_does_not_wait_for_runtime_owned_run_task` at source_consumer.rs:380), and the integration tests `tests/source_integration.rs` and `tests/source_stream_integration.rs`. The URI is the naming key for gate errors and bind-conflict errors (the consumer has no other route identity).
2. Add a crate-global ack store in `lib.rs` mirroring the mcp shape exactly (`McpServerRegistry`, camel-component-mcp/src/registry.rs:145-167): a `pub struct WasmSourceBindAcks` holding `OnceLock<RwLock<HashMap<String, bool>>>` with `pub fn global() -> &'static Self` (init-once) and `pub fn set(&self, acks: HashMap<String, bool>)` that REPLACES the map (interior mutability, no `reset_for_tests` — tests replace the map or use distinct ephemeral ports). Add `pub fn acknowledged(&self, bind: &str) -> bool`.
2. In `WasmSourceConsumer::start()`, after guest `configure()` reveals `listener_spec.bind` (source_consumer.rs:156) and BEFORE `TcpListener::bind` (:192), resolve the effective bind:
   - Parse operator `bind` from `self.guest_config` (entries captured by `parse_guest_config`, :353) if present.
   - If operator bind exists and equals the guest `listener_spec.bind` after `SocketAddr` parse-and-compare, use it.
   - If operator bind exists and differs, return `Err(CamelError::RouteError(format!(...)))` naming the route URI, the operator bind, and the guest bind; do NOT bind.
   - If no operator bind, use the guest bind.
3. Run the exposure gate on the resolved bind: `camel_auth::enforce_bind_exposure_gate(bind_key, is_loopback, plans, acked)` where `bind_key` is the resolved address string, `is_loopback = bind_addr.is_loopback()`, `plans` is a one-element slice `(&[(route_key, plan)])` with `route_key` = the consumer's endpoint URI string. The gate plan is the EXACT retained classification plan: `self.kernel` present → `kernel.plan.clone()`; else `self.plan.clone()` when captured; else a `Public`-mode plan built the same way camel-http builds undeclared-route plans for the gate (a non-Public classification without wiring yields NO Public routes, so the gate passes — the fail-closed denial happens per-request in Task 2.2, not at the gate). On `Err`, propagate (startup fails before the socket is bound).
4. Keep the existing synchronous bind-before-spawn ordering (comment at :188) — the gate call slots immediately before it.
5. New guest fixture `tests/fixtures/conflicting-bind-guest/` whose declared `listener_spec.bind` is INDEPENDENT of the operator `bind` config entry (hardcode the bind string in the guest's `configure()`; do not read the `bind` key from guest config — the existing webhook guest derives its bind from that key, guest/src/lib.rs:55-60, which makes the conflict path unreachable with it). Build it with the same `cargo build --target wasm32-wasip2` helper used by `tests/source_integration.rs` (ephemeral-port pattern at :173-193).
6. New integration test file `tests/source_bind_gate.rs`. All its tests start real guest fixtures → mark each `#[ignore]` (ADR-0054 CI contract) and add `crates/components/camel-component-wasm/tests/source_bind_gate.rs` to `scripts/xtask/allowlist-ignore.txt` (the allowlist accepts whole targets). Acceptance commands use `-- --ignored`.
7. CI guest-build path: add `tests/fixtures/conflicting-bind-guest/` to the hardcoded guest build list in `.github/workflows/ci.yml` (the list at ci.yml:282-287 building `examples/...` guests with `--target wasm32-wasip2`), so the fixture exists when CI runs the ignored target.

**Tests:** (all `#[ignore]`d — run with `-- --ignored`; they start real guests)
- `matching_binds_produce_one_listener`: webhook-style guest fixture declaring `listener_spec.bind = "127.0.0.1:<ephemeral>"` + URI `?bind=127.0.0.1:<same port>` → `start()` succeeds; assert exactly one listener exists by connecting once. Command: `cargo test -p camel-component-wasm --test source_bind_gate -- --ignored matching_binds`.
- `conflicting_binds_fail_before_socket`: `conflicting-bind-guest` declaring `127.0.0.1:<portA>` (hardcoded ephemeral) + URI `?bind=127.0.0.1:<portB>` → `start()` returns `Err` whose message contains both addresses; assert `TcpStream::connect` to portB fails (nothing bound). Command: `cargo test -p camel-component-wasm --test source_bind_gate -- --ignored conflicting_binds`.
- `non_loopback_public_gate_with_and_without_ack` (single sequential test — the ack store is one process-global map, so the ack-mutating phases must not run in parallel with each other): phase 1 — guest declaring `0.0.0.0:<portA>` (Public, no security wiring), ack map empty → `start()` returns `Err` naming the bind; phase 2 — fresh consumer, same guest shape on `<portB>`, `WasmSourceBindAcks::global().set(map)` acknowledging `"0.0.0.0:<portB>"` → `start()` succeeds AND a `warn!` naming the bind and its exposed Public-route count is emitted — capture it with the `tracing_subscriber` make-writer + `set_default` dispatcher pattern used in `crates/camel-core/src/lifecycle/adapters/route_controller_trait_tests.rs`; assert the captured output contains the bind string and the route count. Command: `cargo test -p camel-component-wasm --test source_bind_gate -- --ignored non_loopback_public_gate`.
- `loopback_public_needs_no_ack`: guest declaring `127.0.0.1:<port>`, ack map empty → `start()` succeeds. Command: `cargo test -p camel-component-wasm --test source_bind_gate -- --ignored loopback_public`.

**Acceptance:**
- `cargo test -p camel-component-wasm --test source_bind_gate -- --ignored` exits 0.
- `scripts/xtask/allowlist-ignore.txt` contains the target; `.github/workflows/ci.yml` guest build list contains the fixture.
- `cargo clippy -p camel-component-wasm -- -D warnings` exits 0.

- [x] 1.4

### camel-cli

#### Task 1.5: Wire CLI bind acks into the wasm ack store

**Files:**
- `crates/camel-cli/src/commands/run.rs` (modified)

**Steps:**
1. In `run.rs` where `bind_acks` is built (run.rs:256-267), next to the existing `#[cfg(feature = "mcp")]`-guarded `McpServerRegistry::global().set_bind_exposure_acks(...)` call (run.rs:267), add `camel_component_wasm::WasmSourceBindAcks::global().set(bind_acks.clone())` guarded by `#[cfg(feature = "wasm")]` — `camel-component-wasm` is an OPTIONAL dependency of camel-cli behind the `wasm` feature (camel-cli Cargo.toml:71,119).
2. Place the call inside the same scope the mcp call lives in so `bind_acks` borrows stay valid.

**Tests:**
- `wasm_bind_acks_wired_from_config`: unit test in run.rs's test module, `#[cfg(feature = "wasm")]`-guarded: build the ack map → invoke the wiring statements' containing helper (or inline the same calls) → `WasmSourceBindAcks::global().acknowledged("<addr>")` returns what the config set. Command: `cargo test -p camel-cli --features wasm wasm_bind_acks`.

**Acceptance:**
- `cargo test -p camel-cli --features wasm` exits 0.
- `cargo clippy -p camel-cli -- -D warnings` exits 0 (default features).
- `cargo check -p camel-cli --no-default-features` exits 0 (guard compiles out).

- [x] 1.5

## Phase 2: Kernel handshake + carrier

### camel-component-wasm

#### Task 2.1: Thread the minted principal to Exchange assembly and install the carrier

**Files:**
- `crates/components/camel-component-wasm/src/source_host.rs` (modified)

**Steps:**
1. Add private field `principal: Option<camel_auth::AuthenticatedPrincipal>` to `HttpMeta` (source_host.rs:41-45, host-defined struct with pub fields — NOT WIT-generated), defaulting `None` at every existing construction site. The guest-facing conversion in `accept_http` (source_host.rs:186-247, which maps `HttpMeta` field-by-field into the WIT `HttpRequest`) must NOT include it — the guest never sees the principal.
2. `submit_exchange` (source_host.rs:250) receives only the guest's `WasmExchange` — the `HttpMeta` never reaches it directly. Thread the principal through host state with a ONE-OUTSTANDING-REQUEST invariant (the WIT contract does NOT force accept/submit alternation — a buggy guest may accept twice; the host must not let a second accept overwrite identity): add field `pending_principal: Option<AuthenticatedPrincipal>` plus an outstanding marker to `SourceHostState`. In `accept_http`: if an outstanding request is already pending → fail closed (return the WIT error result for the second accept; do NOT overwrite the slot); else set `state.pending_principal = meta.principal.take()` (unconditional replace on the free slot) and mark outstanding. In `submit_exchange`: take the slot and CLEAR the outstanding marker. The intended guest run loop is strictly sequential (capacity-1 channels), so one slot suffices for well-behaved guests; the invariant guards misbehaving ones.
3. In `submit_exchange`, after `source_exchange_to_native` assembles the native Exchange and BEFORE the `exchange_tx.send`: `accessor.with(|state| state.pending_principal.take())` — if `Some(p)`, call `camel_auth::install_carrier(&mut native_exchange, &p)`.
4. `RequestChannelItem` (source_host.rs:53, tuple alias) needs no change — it already carries the extended `HttpMeta`.
5. Tests for this task are in-crate `#[cfg(test)]` unit tests in source_host.rs (host-import impls like `submit_exchange` are not callable from off-crate integration tests); end-to-end carrier assertions live in Task 2.3.

**Tests:**
- `pending_principal_installed_on_exchange` (in-crate, `#[cfg(test)]` mod in source_host.rs): unit-drive `accept_http`'s principal-stash + `submit_exchange`'s take-and-install with a principal minted via `camel_auth::kernel_authenticate` against an in-crate test provider → the assembled Exchange's carrier is readable (`camel_auth::read_carrier`, kernel.rs:183, returns `Some`). Command: `cargo test -p camel-component-wasm --lib pending_principal`.
- `no_principal_no_carrier` (in-crate): same flow with `pending_principal = None` → `read_carrier` returns `None`. Command: `cargo test -p camel-component-wasm --lib no_principal_no_carrier`.
- `double_accept_fails_closed` (in-crate): call `accept_http` twice without an intervening `submit_exchange` → the second accept returns an error result and the first request's stashed principal is NOT overwritten. Command: `cargo test -p camel-component-wasm --lib double_accept`.

**Acceptance:**
- `cargo test -p camel-component-wasm --lib` exits 0.
- `cargo clippy -p camel-component-wasm -- -D warnings` exits 0.

- [x] 2.1

#### Task 2.2: Host-edge handshake in `run_http_listener` — deny 401 before the guest

**Files:**
- `crates/components/camel-component-wasm/src/source_host.rs` (modified)
- `crates/components/camel-component-wasm/src/source_consumer.rs` (modified)
- `crates/components/camel-component-wasm/tests/source_auth_e2e.rs` (new)
- `scripts/xtask/allowlist-ignore.txt` (modified)

**Steps:**
1. Change `run_http_listener` signature (source_host.rs:500) to take `kernel: Option<Arc<WasmSourceKernelAuth>>` AND `plan_access: Option<AccessMode>` (derived at the call site as `self.plan.as_ref().map(|p| p.access_mode.clone())`); update the single call site (source_consumer.rs:206) to pass `self.kernel` (cloned out of the consumer before the run task spawns) and the derived classification.
2. In the axum `handler`: BEFORE `state.tx.send(...)` (source_host.rs:541 area), insert the auth step. The decision is driven by the classification captured in Task 1.3, not by kernel presence alone (`kernel = None` must NOT conflate Public with incomplete wiring):
   - classification `None` (no context ever set — raw construction) OR classification `Some(Public)` → pass-through (no extraction), current behavior.
   - classification `Some(non-Public)` with kernel present: `camel_auth::extract_token_multi` over the request headers + URI query + cookie header (credential_source.rs:84 — use the same argument shape camel-http passes; construct from `parts.headers` and `parts.uri`) with the plan's `credential_sources`. No token → respond `401` with body `"unauthenticated"`, return WITHOUT `tx.send` and WITHOUT reading the body. Token found → `camel_auth::kernel_authenticate(...)` (kernel.rs:77): on mint failure or provider mismatch → `401`, return without forwarding; on success → set `http_meta.principal = Some(minted)` and proceed to `tx.send` as today (202-immediate-ack semantics unchanged).
   - classification `Some(non-Public)` with kernel MISSING (plan-only context): respond `401` — absent wiring never yields pass-through for non-Public plans (fail-closed).
   Thread the classification to the handler: extend `run_http_listener`'s `ListenerState` with `plan_access: Option<AccessMode>` alongside the kernel.
3. No WIT or guest-visible changes: denials simply never reach `accept-http`.
4. Store `kernel` in `ListenerState` alongside `tx`/`cancel`.
5. New integration target `tests/source_auth_e2e.rs` for guest-starting handshake tests (keep `tests/source_auth.rs` from Task 1.3 guest-free — no `start()` there, so it stays outside the ignore regime). Mark every test in the new target `#[ignore]` (ADR-0054) and add `crates/components/camel-component-wasm/tests/source_auth_e2e.rs` to `scripts/xtask/allowlist-ignore.txt`. Guest fixtures: reuse the webhook-style fixture from Task 1.4 (listener behavior is guest-agnostic for denials).

**Tests:** (all `#[ignore]`d — run with `-- --ignored`)
- `authenticated_route_missing_credential_401`: consumer with kernel (Authenticated plan, fixture provider) → HTTP request without credential to the bound listener → response status is 401; assert the guest never woke by asserting no exchange reached the pipeline channel and the request channel item count is 0. Command: `cargo test -p camel-component-wasm --test source_auth_e2e -- --ignored missing_credential_401`.
- `authenticated_route_invalid_credential_401`: same with a malformed token in `Authorization: Bearer <garbage>` → 401. Command: `cargo test -p camel-component-wasm --test source_auth_e2e -- --ignored invalid_credential_401`.
- `public_plan_pass_through_unchanged`: consumer with `set_security_context(SecurityContext::from_plan(<Public-mode plan>))` (the blessed `Some(Public)` scenario — NOT an unwired consumer) → plain request → 202 immediate ack and the guest processes it (existing behavior; mirror `tests/source_integration.rs` assertions). Command: `cargo test -p camel-component-wasm --test source_auth_e2e -- --ignored public_pass_through`.
- `missing_wiring_non_public_denies`: consumer with a plan-only context (non-Public plan via `SecurityContext::from_plan`, no providers → kernel absent but classification captured) → request → 401. Command: `cargo test -p camel-component-wasm --test source_auth_e2e -- --ignored missing_wiring_non_public_denies`.

**Acceptance:**
- `cargo test -p camel-component-wasm --test source_auth_e2e -- --ignored` exits 0.
- `cargo test -p camel-component-wasm --test source_auth` exits 0 (guest-free, non-ignored).
- `cargo test -p camel-component-wasm --test source_integration -- --ignored` exits 0 (no regression; existing target already allowlisted).
- `cargo clippy -p camel-component-wasm -- -D warnings` exits 0.

- [x] 2.2

#### Task 2.3: Credential-source matrix and provider substitution (end to end)

**Files:**
- `crates/components/camel-component-wasm/tests/source_auth_e2e.rs` (modified)

**Steps:**
1. Add an end-to-end test per permitted credential source to the SAME ignored e2e target (guest-starting), each driving a real guest fixture through the listener: the route's plan declares exactly one `credential_source` variant and the request carries a valid fixture-provider token in that source — (a) AuthorizationHeader (`Authorization: Bearer <t>`), (b) Header (plan's named header), (c) Cookie (`Cookie: <name>=<t>`), (d) QueryParam (`?token=<t>` style per the source's field naming, mirroring camel-http's tests).
2. Add the substitution test: plan bound to fixture provider B; request carries a valid token minted by provider A → 401 and no guest wakeup.
3. Assert for every success case that the pipeline Exchange carries the carrier: capture the Exchange the consumer emits into its channel and assert `camel_auth::read_carrier(&exchange)` is `Some` (kernel.rs:183; same assertion the http/grpc transport tests use).

**Tests:** (all `#[ignore]`d — run with `-- --ignored`)
- `auth_via_authorization_header`: Authenticated route, source=AuthorizationHeader, valid token → request accepted (202) and downstream Exchange `read_carrier` is `Some`. Command: `cargo test -p camel-component-wasm --test source_auth_e2e -- --ignored auth_via_authorization_header`.
- `auth_via_named_header`: source=Header → same assertions. Command: `cargo test -p camel-component-wasm --test source_auth_e2e -- --ignored auth_via_named_header`.
- `auth_via_cookie`: source=Cookie → same assertions. Command: `cargo test -p camel-component-wasm --test source_auth_e2e -- --ignored auth_via_cookie`.
- `auth_via_query_param`: source=QueryParam → same assertions. Command: `cargo test -p camel-component-wasm --test source_auth_e2e -- --ignored auth_via_query_param`.
- `provider_substitution_denied`: provider-B route + provider-A token → 401, no exchange in pipeline. Command: `cargo test -p camel-component-wasm --test source_auth_e2e -- --ignored provider_substitution_denied`.

**Acceptance:**
- `cargo test -p camel-component-wasm --test source_auth_e2e -- --ignored` exits 0.
- `cargo clippy -p camel-component-wasm -- -D warnings` exits 0.

- [x] 2.3

## Phase 3: Docs + example convergence

### docs

#### Task 3.1: Amend ADR-0061 and update CONTEXT-MAP.md

**Files:**
- `docs/adr/0061-unified-transport-auth.md` (modified)
- `CONTEXT-MAP.md` (modified)

**Steps:**
1. Amend ADR-0061 (amendment section, not a rewrite): name the wasm source as the 5th kernel transport; record the boundary-auth placement (Gen B, like ws/grpc), the carrier threading via `HttpMeta` (Exchange minted after the edge), the operator-authoritative bind with conflict failing at consumer startup before bind, and the no-WIT-change property (guest never observes auth).
2. Update the ADR-0061 index line in `CONTEXT-MAP.md` to mention the 5 transports (wasm source included).
3. Update the unified-transport / CredentialSource key-term entries in `CONTEXT-MAP.md` to name `wasm:` and its permitted credential sources (AuthorizationHeader, Header, Cookie, QueryParam).

**Tests:**
- `mdbook_build_docs`: docs build after edits → `nix shell nixpkgs#mdbook -c mdbook build docs` exits 0 with no broken links to the amended ADR.

**Acceptance:**
- `nix shell nixpkgs#mdbook -c mdbook build docs` exits 0.
- Two-source rule: ADR-0061 and CONTEXT-MAP.md tell the same story (no drift); `cargo xtask lint-context-citations` exits 0.

- [x] 3.1

### camel-component-wasm / examples

#### Task 3.2: Crate CONTEXT.md inbound posture + webhook example fix

**Files:**
- `crates/components/camel-component-wasm/CONTEXT.md` (modified)
- `examples/wasm-source-webhook/routes/webhook.yaml` (modified)
- `examples/wasm-source-webhook/README.md` (modified)

**Steps:**
1. In the crate `CONTEXT.md`, add an inbound-request posture subsection to the capability-posture section: the source world is the 5th inbound transport; requests are authenticated at the host edge (`set_security_context` → `kernel_authenticate` → `install_carrier`); denied requests never reach the guest; non-loopback binds require exposure acknowledgment (ADR-0052 rule-3 posture); the sandbox guest model (operators trust the plugin) is unchanged and orthogonal.
2. Fix `examples/wasm-source-webhook/routes/webhook.yaml`: change `bind=0.0.0.0:8080` to a loopback default (`bind=127.0.0.1:8080`) so the example starts under the new gate unchanged.
3. Update the example README: document the loopback default, how to acknowledge a non-loopback bind (`[binds."0.0.0.0:8080"] allow_public_exposure = true` with the permanent warn), and how to declare a `security_policy` on the route for authenticated webhooks.
4. Verify the example route file still passes route-lint (`cargo xtask schema --check` if route yaml is in schema scope).

**Tests:**
- `webhook_example_loopback_starts`: run the example's route through the gate — guest fixture declared bind matches URI bind `127.0.0.1:8080` variant with ephemeral port → `start()` succeeds (covered mechanically by Task 1.4 tests; here verify the yaml edit did not break parsing). Command: `cargo xtask schema --check`.

**Acceptance:**
- `cargo xtask schema --check` exits 0.
- `cargo xtask lint-context-citations` exits 0.
- CONTEXT.md new subsection is English, follows STE-writing (no AI slop), cites ADR-0061/0052.

- [x] 3.2
