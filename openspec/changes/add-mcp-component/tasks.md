# Tasks: add-mcp-component

## Phase 1: Crate skeleton + rmcp client boundary + Producer (client) role

### Task 1.1: Crate skeleton, workspace wiring, component metadata

**Files:**
- `crates/components/camel-component-mcp/Cargo.toml` (new)
- `crates/components/camel-component-mcp/src/lib.rs` (new)
- `crates/components/camel-component-mcp/src/component.rs` (new)
- `crates/components/camel-component-mcp/src/error.rs` (new)
- `crates/components/camel-component-mcp/CONTEXT.md` (new)
- `Cargo.toml` (modified — workspace `[workspace.dependencies]`: add `rmcp`)

**Steps:**
1. In root `Cargo.toml` `[workspace.dependencies]` add `rmcp = { version = "3", features = ["client", "server", "transport-streamable-http-server", "transport-streamable-http-client"] }`. Do NOT enable any `transport-sse-*` feature.
2. Create crate `camel-component-mcp` with `edition.workspace = true` (workspace edition 2024), matching sibling manifests. Dependencies: `camel-api`, `camel-component-api`, `serde`, `serde_json`, `tokio`, `tracing`, `thiserror`, `async-trait`, `rmcp = { workspace = true }` (mirroring `crates/components/camel-component-llm/Cargo.toml` dep set; thiserror per its `error.rs`). The `crates/components/*` glob in workspace members already includes the new crate — no members edit.
3. `src/error.rs`: `#[non_exhaustive] #[derive(Debug, thiserror::Error)] pub enum McpError` with variants `MissingSecurityPolicy { server: String }`, `IncompatibleRemote { server: String, version: String }`, `CapExceeded { kind: String, max: usize }`, `Config(#[from] serde_json::Error)` plus a generic `Endpoint(String)` — copy `LlmError`'s `#[non_exhaustive]` + thiserror + `From`-for-`CamelError` style from `crates/components/camel-component-llm/src/error.rs`.
4. `src/component.rs`: `pub struct McpComponent;` implementing the `Component` trait from `camel-component-api` exactly as `crates/components/camel-component-llm/src/lib.rs` does — the trait surface is `scheme/metadata/create_endpoint/start/stop` (producer/consumer creation lives on the `Endpoint`, see Task 1.4). `scheme()` returns `"mcp"`. `metadata()` builds `ComponentMetadata::minimal("mcp")` then sets `version: env!("CARGO_PKG_VERSION").into()`, a one-line description, `uri_syntax: "mcp:<server>/tool/<name>?schema=<json> | mcp:<server>/resource/<name>?uri=<mcp-uri> | mcp:call?server=<name>&tool=<name> | mcp:read?server=<name>&uri=<uri>"`, capabilities with `supports_consumer: true`, `supports_producer: true`, `supports_streaming: true` (use the actual `ComponentCapabilities` field names from `crates/camel-api/src/component_metadata.rs` verbatim), and `uri_options` documenting `server`, `tool`, `uri`, `schema` (URL-encoded tool input JSON Schema carried on the tool consumer URI — the DSL lowering channel), `bind`, `security_policy`, `transport`. `create_endpoint` returns an `Err(CamelError)` stub ("not yet implemented") — Tasks 1.4 and 2.4 replace it.
5. `src/lib.rs`: module declarations (`pub mod adapter;` arrives in Task 1.3 — until then declare only existing modules) + `pub use component::McpComponent;`.
6. `CONTEXT.md`: one-section crate context (purpose, scheme `mcp:`, dual roles, rmcp confinement, ADR-0020/0060 citations) following `crates/components/CONTEXT.md` entry style.

**Tests:**
- `metadata_declares_both_roles`: (unit test in `src/component.rs` `#[cfg(test)]`) → call `McpComponent.metadata()` → assert `scheme == "mcp"`, capabilities report consumer+producer+streaming all true, and `uri_options` contains options named `server` and `schema`.
- `metadata_validates_against_scheme`: `metadata().validate_scheme("mcp")` → `Ok(())`.

**Acceptance:**
- `cargo build -p camel-component-mcp` exits 0.
- `cargo clippy -p camel-component-mcp -- -D warnings` exits 0.
- `cargo test -p camel-component-mcp metadata` passes.

- [x] 1.1

### Task 1.2: Component-local types, headers, config, McpClient trait + server map

**Files:**
- `crates/components/camel-component-mcp/src/types.rs` (new)
- `crates/components/camel-component-mcp/src/headers.rs` (new)
- `crates/components/camel-component-mcp/src/config.rs` (new)
- `crates/components/camel-component-mcp/src/client.rs` (new)
- `crates/components/camel-component-mcp/src/lib.rs` (modified)

**Steps:**
1. `src/types.rs`: `pub struct McpToolInvocation { pub name: String, pub arguments: serde_json::Value, pub reply: tokio::sync::oneshot::Sender<McpToolResult> }`, `pub struct McpToolResult { pub content: serde_json::Value }`, `pub struct McpResourceRead { pub uri: String, pub reply: tokio::sync::oneshot::Sender<McpResource> }`, `pub struct McpResource { pub uri: String, pub content: Vec<u8>, pub mime_type: String }`. All `Debug, Clone, Send, Sync` where possible (the `reply` senders make invocation types non-`Clone` — derive only what holds). The oneshot `reply` channels are how the dispatch layer (Tasks 2.6/2.7) receives the route's answer and how the consumer bridge (Task 2.4) sends it back. No rmcp import here — these are the Camel-shaped boundary types (spec: Component-local MCP types).
2. `src/headers.rs`: `pub const CAMEL_MCP_TOOL_CALL: &str = "CamelMcpToolCall";` and `pub const CAMEL_MCP_RESULT: &str = "CamelMcpResult";` header-name constants (same style as `crates/components/camel-component-llm/src/headers.rs`).
3. `src/config.rs`: `pub enum McpTransport { StreamableHttp }` with a serde representation where deserializing any other transport string (`"stdio"`, `"sse"`, `"http+sse"`, `"legacy-http-sse"`) yields a deny error naming the rejected transport; `pub struct McpServerConfig` with `#[serde(deny_unknown_fields)]` and fields `bind: String`, `tls: Option<serde_json::Value>`, `security_policy: Option<serde_json::Value>`, `max_tools: usize` (serde default 128), `max_resources: usize` (serde default 128). `pub struct McpRemoteConfig { url: String, transport: McpTransport }` for named client-role remotes. `pub struct McpGlobalConfig { servers: HashMap<String, McpServerConfig>, remotes: HashMap<String, McpRemoteConfig> }` (`#[serde(deny_unknown_fields)]`, per-item config channel, ADR-0038).
4. `src/client.rs`: `#[async_trait] pub trait McpClient: Send + Sync { async fn call_tool(&self, tool: &str, arguments: serde_json::Value) -> Result<McpToolResult, McpError>; async fn read_resource(&self, uri: &str) -> Result<McpResource, McpError>; }` and `pub type McpServerMap = HashMap<String, std::sync::Arc<dyn McpClient>>;` — the client-role twin of llm's `ProviderMap` (ADR-0020).

**Tests:**
- `transport_stdio_rejected`: deserialize `McpRemoteConfig` JSON with `"transport": "stdio"` → `Err` whose message contains `stdio`.
- `transport_legacy_sse_rejected`: same with `"transport": "http+sse"` → `Err` mentioning the transport.
- `server_config_defaults_caps_to_128`: deserialize `McpServerConfig` `{"bind": "127.0.0.1:0"}` → `max_tools == 128 && max_resources == 128`.
- `server_config_unknown_field_rejected`: add key `"session": true` → `Err` (deny_unknown_fields).

**Acceptance:**
- `cargo test -p camel-component-mcp config` passes.
- `cargo clippy -p camel-component-mcp -- -D warnings` exits 0.
- `grep -rn "rmcp" src/types.rs src/headers.rs src/config.rs src/client.rs` → zero hits.

- [x] 1.2

### Task 1.3: rmcp client adapter — discover lifecycle, fail-fast

**Files:**
- `crates/components/camel-component-mcp/src/adapter/mod.rs` (new)
- `crates/components/camel-component-mcp/src/adapter/client.rs` (new)
- `crates/components/camel-component-mcp/tests/client_producer_test.rs` (new)
- `crates/components/camel-component-mcp/tests/common/mod.rs` (new — shared test helpers: `RecordedRequest`, recorder layer, `warn_capture`, mock server builder)
- `crates/components/camel-component-mcp/src/lib.rs` (modified — `pub mod adapter;`)

**Steps:**
1. `src/adapter/client.rs`: `pub struct RmcpClient` holding the connected rmcp client. `pub async fn connect(name: &str, config: &McpRemoteConfig) -> Result<Self, McpError>`:
   - Build rmcp's Streamable-HTTP client transport for `config.url`.
   - Connect via the discover lifecycle with `2026-07-28` as the only preferred version — per `design.md` §Approach this is `serve_with_lifecycle(ClientLifecycleMode::Discover { preferred_versions: vec![ProtocolVersion::V_2026_07_28] })`. rmcp ^3 is not yet vendored in `~/.cargo`; after first build, if rmcp 3.x ships slightly different symbol names for the discover lifecycle, adjust INSIDE `src/adapter/client.rs` only — the `McpClient` trait seam and all test assertions stay fixed.
   - On discover failure mapping to "remote does not speak 2026-07-28" (unsupported version list, or `METHOD_NOT_FOUND` for `server/discover`): `tracing::warn!` naming the server and the detected/absent version, then return `Err(McpError::IncompatibleRemote { server, version })`. NEVER call the legacy `serve()`/initialize path.
   - `impl McpClient for RmcpClient`: `call_tool` issues one `tools/call` carrying `io.modelcontextprotocol/protocolVersion = "2026-07-28"` and client capabilities in request `_meta`; `read_resource` issues one `resources/read` by URI. Both emit the `Mcp-Method`/`Mcp-Name` standard headers on the JSON-RPC POST and never set `Mcp-Session-Id`.
2. `tests/client_producer_test.rs`: spin up an in-process mock MCP server using rmcp's server-side Streamable-HTTP transport on an ephemeral port: an axum service with (a) a test `ServerHandler` answering `server/discover`, `tools/call`, `resources/read`, parameterizable `supported_protocol_versions()`, and (b) an axum middleware layer (NOT the handler — `ServerHandler` never sees HTTP headers) recording every POST into a shared `Arc<Mutex<Vec<RecordedRequest>>>` with method, headers map, and JSON body. Put `RecordedRequest`, the recorder layer, and the `warn_capture()` helper (install a `tracing` subscriber with a recording `Layer` collecting `(level, message)` pairs, returning a handle to assert on) in `tests/common/mod.rs` and declare `mod common;` in every integration-test binary that needs them (2.4, 2.5) — precedent: `crates/components/camel-component-llm/tests/common/`.
3. Write the four tests below. All use ephemeral ports (`127.0.0.1:0`) — never a fixed port.

**Tests:**
- `discover_accepts_2026_07_28_remote`: mock server advertises `["2026-07-28"]` → `RmcpClient::connect` → `Ok`, then `call_tool("lookup", {"id":"42"})` returns the mock's canned `McpToolResult`.
- `legacy_remote_fails_connect`: mock server advertises only `["2025-11-25"]` → `connect` returns `Err(McpError::IncompatibleRemote)` whose Display contains the server name and `2025-11-25`; `warn_capture` saw a `warn` naming the server (fail-fast warn before the error).
- `no_discover_fails_connect`: mock returns `METHOD_NOT_FOUND` for `server/discover` → `connect` returns `Err(McpError::IncompatibleRemote)` (absent version case).
- `client_emits_standard_headers_no_session`: after a successful `call_tool`, the recorded requests show `Mcp-Method` and `Mcp-Name` headers present and NO `Mcp-Session-Id` header ever sent, and the JSON-RPC body `_meta` carries `io.modelcontextprotocol/protocolVersion = "2026-07-28"`.

**Acceptance:**
- `cargo test -p camel-component-mcp --test client_producer_test` passes.
- `cargo clippy -p camel-component-mcp -- -D warnings` exits 0.
- `grep -rn "rmcp::" src/ --include='*.rs' | grep -v 'src/adapter/'` → zero hits.

- [x] 1.3

### Task 1.4: Producer endpoints — mcp:call and mcp:read

**Files:**
- `crates/components/camel-component-mcp/src/endpoint.rs` (new)
- `crates/components/camel-component-mcp/src/producer.rs` (new)
- `crates/components/camel-component-mcp/src/component.rs` (modified — `create_endpoint` returns `McpEndpoint` for producer URIs)
- `crates/components/camel-component-mcp/src/lib.rs` (modified)
- `crates/components/camel-component-mcp/tests/client_producer_test.rs` (modified — appended tests)

**Steps:**
1. `src/endpoint.rs`: `pub struct McpEndpoint` (holds the parsed URI + `Arc<McpServerMap>`); `pub enum McpEndpointUri` parsed from the endpoint URI: `mcp:call?server=<name>&tool=<name>` → `Operation::Call { server, tool }`; `mcp:read?server=<name>&uri=<uri>` → `Operation::Read { server, uri }`. Unknown operation path (anything other than `call`/`read`) → `Err(McpError::Endpoint)` naming the URI. Read does NOT field-sniff tool-vs-resource — the two operations are distinct URI shapes (spec: Client producer resource read).
2. `src/component.rs`: `McpComponent::new(config: McpGlobalConfig) -> Result<Self, CamelError>` mirroring `LlmComponent::new` at `crates/components/camel-component-llm/src/lib.rs:83` — stores `config.remotes` and creates the (empty, live) `McpServerMap`; `RmcpClient::connect` runs at producer lifecycle `start()` (discover-at-start per design.md Phase 1; no network I/O at construction), with fail-fast `Err` on incompatible remotes. `create_endpoint` parses the URI, resolves the server name against `config.remotes` (missing name errors at construction), and returns `McpEndpoint`; consumer-shaped URIs (`mcp:<server>/tool/..|resource/..`) return `Err` until Task 2.4.
3. `src/producer.rs`: `pub struct McpProducer` as a Tower `Service<Exchange>` mirroring the shape of `crates/components/camel-component-llm/src/producer.rs` (its producer is a Tower service at producer.rs:845, not a start/stop trait). Producer startup: `McpEndpoint::create_producer()` issues `RmcpClient::connect` at endpoint/component `start` time (fail-fast — spec: Producer fail-fast on incompatible remote) and caches the client; on `Err` the producer never becomes ready. Missing remote name at construction → `Err(McpError::Endpoint)`.
4. Per-Exchange processing (Call): take the Exchange body as the arguments JSON object, set `CAMEL_MCP_TOOL_CALL` header with `{"server":..,"tool":..}`, issue one `call_tool`, set the `McpToolResult` content as the output body and `CAMEL_MCP_RESULT` header. Read: issue `read_resource(uri)` with no arguments body, output body = resource content. The producer performs NO other routing decision — it never calls any LLM component and never issues a second call (spec: Route-owned tool dispatch, no auto-loop).
5. `src/component.rs`: wire `create_endpoint` so producer URIs yield an endpoint whose `create_producer` returns the `McpProducer` service.

**Tests:**
- `producer_resolves_named_server`: `McpComponent::new` with remotes entry `crm` → construct endpoint from `mcp:call?server=crm&tool=lookup` → construction succeeds (spec scenario: client producer endpoint resolves a named server).
- `mcp_call_returns_result`: started producer against the mock → send Exchange with body `{"id":"42"}` → output body equals the mock's canned tool result.
- `mcp_read_returns_content`: `mcp:read?server=docs&uri=file:///a` against mock → output body equals the mock's canned resource content.
- `per_request_meta_three_exchanges_no_initialize`: send three Exchanges → recorded requests contain exactly three `tools/call`, EACH carrying `_meta` protocolVersion `2026-07-28`, zero `initialize` requests, zero `Mcp-Session-Id` headers (spec scenario: tool call carries per-request protocol metadata).
- `producer_start_fails_on_legacy_remote`: mock advertising only `["2025-11-25"]`, endpoint for `mcp:call?server=legacy&tool=x` → producer startup returns `Err` mapping `McpError::IncompatibleRemote` naming `legacy` and `2025-11-25` (design Phase 1 exit: producer start failure, not just connect); `warn_capture` saw the warn.
- `producer_no_auto_loop`: after one call, the recorded request log shows exactly one JSON-RPC request total and the producer crate's `Cargo.toml` has no `camel-component-llm` dependency (assert in-test by reading `env!("CARGO_MANIFEST_DIR")/Cargo.toml`).

**Acceptance:**
- `cargo test -p camel-component-mcp --test client_producer_test` passes (all 10 tests: 4 from Task 1.3 + 6 new).
- `cargo clippy -p camel-component-mcp -- -D warnings` exits 0.

- [x] 1.4

### Task 1.5: rmcp boundary test + camel-api purity test

**Files:**
- `crates/components/camel-component-mcp/tests/rmcp_boundary_test.rs` (new)

**Steps:**
1. Copy the source-scanning pattern from `crates/camel-core/tests/hexagonal_architecture_boundaries_test.rs` (walk files under `env!("CARGO_MANIFEST_DIR")/src` with `std::fs`, read contents, match patterns).
2. `no_rmcp_outside_adapter`: for every `.rs` file under `src/` NOT under `src/adapter/`, assert the file contains no `rmcp::` path. Collect violations and panic listing them (spec scenario: boundary test passes).
3. `camel_api_has_no_mcp_public_types`: walk `../../camel-api/src` (relative to `CARGO_MANIFEST_DIR`, resolved via `PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../camel-api/src")`), assert no line matches the regex `pub\s+(struct|enum|trait|type|fn|const)\s+Mcp` (spec scenario: camel-api gains no MCP type).

**Tests:**
- `no_rmcp_outside_adapter`: src tree → scan → zero violations, test passes.
- `camel_api_has_no_mcp_public_types`: camel-api tree → scan → zero regex hits.

**Acceptance:**
- `cargo test -p camel-component-mcp --test rmcp_boundary_test` passes.
- Phase 1 exit-criteria met: `cargo build -p camel-component-mcp` clean; producer + negative producer-start tests green; boundary test green (design.md Phase 1).

- [x] 1.5

## Phase 2: Server Consumer — shared listener + registries + dispatch

### Task 2.1: Fail-closed auth, bind policy warning, cap validation

**Files:**
- `crates/components/camel-component-mcp/src/config.rs` (modified)
- `crates/components/camel-component-mcp/tests/server_config_test.rs` (new)

**Steps:**
1. In `src/config.rs` add `pub fn validate_server_policy(name: &str, cfg: &McpServerConfig) -> Result<Option<BindPolicyWarning>, McpError>`:
   - `security_policy` is `None` → `Err(McpError::MissingSecurityPolicy { server })` (fail-closed, spec: MCP server fail-closed authentication).
   - Bind host is `0.0.0.0` or any non-loopback IP → `Ok(Some(BindPolicyWarning::NonLoopback))`; loopback → `Ok(None)`. `pub enum BindPolicyWarning { NonLoopback }` (Parse host from the `bind` string's `SocketAddr` part; unparseable bind → `Err(McpError::Endpoint)`).
   - `max_tools == 0 || max_resources == 0` → `Err(McpError::Endpoint)` naming the offending field (zero caps are invalid — hardened-but-raisable means >=1; `McpError::Config` is `#[from] serde_json::Error` and cannot carry a custom message).
2. The consumer (Task 2.4) calls this at start and `tracing::warn!`s once when it returns `Some(BindPolicyWarning::NonLoopback)`, naming server and bind. Returning the warning as a value keeps this task deterministic; the warn emission itself is asserted in Task 2.4 via the `warn_capture` helper from Task 1.3.

**Tests:**
- `bind_refused_without_security_policy`: `validate_server_policy("crm", cfg_without_policy)` → `Err(McpError::MissingSecurityPolicy)`.
- `loopback_bind_no_warning`: bind `127.0.0.1:9100` + policy → `Ok(None)`.
- `non_loopback_bind_warns`: bind `0.0.0.0:9100` + policy → `Ok(Some(BindPolicyWarning::NonLoopback))`.
- `zero_cap_rejected`: `max_tools: 0` → `Err`.

**Acceptance:**
- `cargo test -p camel-component-mcp --test server_config_test` passes.
- `cargo clippy -p camel-component-mcp -- -D warnings` exits 0.

- [x] 2.1

### Task 2.2: McpServerRegistry — one shared listener per bind

**Files:**
- `crates/components/camel-component-mcp/src/registry.rs` (new)
- `crates/components/camel-component-mcp/src/lib.rs` (modified)
- `crates/components/camel-component-mcp/tests/server_registry_test.rs` (new)

**Steps:**
1. `src/registry.rs`: `pub struct McpServerRegistry` with `pub fn global() -> &'static Self` (once-init, copying `ServerRegistry::global()` in `crates/components/camel-http/src/lib.rs:665-676`) and `pub async fn get_or_spawn(&self, bind: &str, cfg: &McpServerConfig) -> Result<Arc<McpListenerHandle>, McpError>`.
2. `get_or_spawn`: keyed by bind string. Existing entry → return clone of the handle. If the existing entry's stored config conflicts on `tls` or `max_tools`/`max_resources` → `Err(McpError::Endpoint)` naming the bind and the conflicting field (spec: conflicting bind rejected). No entry → spawn: bind a `TcpListener` on the bind address, mount a placeholder axum service (a bare 404 responder — replaced by `McpServerAdapter` mounting in Task 2.5; this task's tests exercise spawn/reuse/conflict only), `tokio::spawn` the serve loop, wrap in `McpListenerHandle { local_addr, tool_registry: McpToolRegistry, resource_registry: McpResourceRegistry, cfg, spawn_count: Arc<AtomicUsize> }`. Registries created from `cfg` caps here.
3. `McpToolRegistry`/`McpResourceRegistry` structs are declared in this task with their `new(max)` constructors only — their API lands in Task 2.3. Keep this task's surface: `get_or_spawn`, conflict detection, handle fields.

**Tests:**
- `first_consumer_spawns_listener`: bind `127.0.0.1:0` → `get_or_spawn` → `TcpStream::connect(handle.local_addr)` succeeds (spec scenario: first consumer spawns the listener).
- `second_consumer_reuses_listener`: same bind, same cfg → second `get_or_spawn` returns handle with identical `local_addr` and `spawn_count.load() == 1`.
- `conflicting_bind_rejected`: spawn with `tls: Some(..)`, then `get_or_spawn` same bind with `tls: None` → `Err(McpError::Endpoint)` (spec scenario: conflicting bind rejected).

**Acceptance:**
- `cargo test -p camel-component-mcp --test server_registry_test` passes.
- Tests bind only ephemeral ports (`:0`) — no fixed-port flakiness.
- `cargo clippy -p camel-component-mcp -- -D warnings` exits 0.

- [x] 2.2

### Task 2.3: Tool/resource registries — registration, readiness, caps, lookup

**Files:**
- `crates/components/camel-component-mcp/src/registry.rs` (modified)
- `crates/components/camel-component-mcp/tests/server_registry_test.rs` (modified — appended unit tests)

**Steps:**
1. `McpToolRegistry`: interior-mutability map `name -> ToolEntry { sender: mpsc::Sender<McpToolInvocation>, input_schema: serde_json::Value, ready: AtomicBool }`. API: `register(name, sender, input_schema) -> Result<(), McpError>` (reject with `McpError::CapExceeded { kind: "tools", max }` when at cap — the (N+1)th, never silent truncation), `mark_ready(name)`, `unregister(name)`, `list_ready() -> Vec<(name, input_schema)>` (filters by ready flag), `resolve(name) -> Option<ToolEntry snapshot>`.
2. `McpResourceRegistry`: `uri -> ResourceEntry { sender: mpsc::Sender<McpResourceRead>, ready: AtomicBool }`. API: `register(uri, sender)` (own cap, `kind: "resources"`), `mark_ready`, `unregister`, `list_ready() -> Vec<uri>`, `resolve(uri) -> Option<..>`.
3. Unknown/stopped lookups return `None` — the dispatch layer (Tasks 2.6/2.7) maps `None` to a clean MCP method error; no dead channel is ever awaited (spec: Tool and resource route registration; Resource URI addressing).

**Tests:**
- `register_129th_tool_rejected`: registry with default 128 → register 128 entries → 129th returns `Err(McpError::CapExceeded)` (spec scenario: 129th tool rejected).
- `raised_cap_allows_150`: `McpToolRegistry::new(200)` → 150 registers → all `Ok` (spec scenario: raised cap allows more tools).
- `not_ready_tool_hidden_from_list`: register one tool, do not `mark_ready` → `list_ready()` is empty; `mark_ready` → list contains it (spec scenario: not-ready tool is hidden from listing).
- `stopped_tool_unregistered`: register + mark_ready + `unregister` → `resolve("lookup")` → `None` (spec scenario: stopped tool is unregistered).
- `unknown_resource_uri_unresolved`: `resolve("crm://unknown")` → `None` (spec scenario: unknown resource URI rejected — dispatch-side mapping lands in 2.5).
- `resource_cap_enforced`: `McpResourceRegistry::new(2)` → third register → `Err(McpError::CapExceeded)`.

**Acceptance:**
- `cargo test -p camel-component-mcp --test server_registry_test` passes (6 new unit tests + 6 existing from 2.2, total 12).
- `cargo clippy -p camel-component-mcp -- -D warnings` exits 0.

- [x] 2.3

### Task 2.4: Server consumer endpoint — wiring registries to routes

**Files:**
- `crates/components/camel-component-mcp/src/consumer.rs` (new)
- `crates/components/camel-component-mcp/src/endpoint.rs` (modified — consumer URI shapes + `create_consumer`)
- `crates/components/camel-component-mcp/src/component.rs` (modified — `create_endpoint` accepts consumer URIs)
- `crates/components/camel-component-mcp/tests/server_consumer_test.rs` (new)

**Steps:**
1. Parse consumer URIs `mcp:<server>/tool/<name>` and `mcp:<server>/resource/<name>` (any other shape → `Err(McpError::Endpoint)` naming the URI) in `McpEndpointUri`, adding `Operation::Tool { server, name, input_schema: serde_json::Value }` (from the URL-encoded `schema` query param on the URI — the channel Task 1.1's `uri_options` documents and Task 3.2's lowering writes; missing/undecodable `schema` param → `Err(McpError::Endpoint)` naming the URI) and `Operation::Resource { server, name, resource_uri: String }` (from the `uri` query param, the declared MCP resource URI — operator config, spec: Resource URI addressing).
2. `McpConsumer::start`:    (a) resolve `McpServerConfig` for `<server>` from component config — missing → `Err(McpError::Endpoint)` naming the server (`McpError::Config` is `#[from] serde_json::Error` and cannot carry a custom message); (b) `validate_server_policy` (Task 2.1) — `Err` propagates, `Some(NonLoopback)` → one `tracing::warn!` naming server + bind; (c) `McpServerRegistry::global().get_or_spawn(bind, cfg)` (Task 2.2); (d) `register` into tool or resource registry (cap error propagates from Consumer::start, spec: Catalog cardinality cap); (e) spawn the route-facing `mpsc::Receiver` task bridging `McpToolInvocation`/`McpResourceRead` into the consumer's route processing: create Exchange from the invocation's args/uri, process route, send the result back through the invocation's `reply` oneshot (mirror how camel-http's http consumer bridges inbound requests to routes); (f) `mark_ready`.
3. `McpConsumer::stop`: `unregister` from its registry (spec: subsequent calls return a clean MCP error, not a dead channel), then release the handle.
4. `src/endpoint.rs`: `McpEndpoint::create_consumer()` returns `McpConsumer`; `src/component.rs` `create_endpoint` now accepts consumer-shaped URIs.

**Tests:**
- `consumer_start_requires_security_policy`: consumer for server with no policy → `start` fails `Err(McpError::MissingSecurityPolicy)` (E2E of Task 2.1 through the consumer).
- `non_loopback_bind_warns_at_start`: bind `0.0.0.0:0` + policy → `start` succeeds and `warn_capture` (helper from Task 1.3) saw exactly one warn naming the server (spec scenario: non-loopback bind warns).
- `tool_consumer_serves_invocation`: ephemeral-bind server `crm` with policy, started tool consumer `lookup` with schema `{"type":"object","properties":{"id":{"type":"string"}},"required":["id"]}`; directly enqueue `McpToolInvocation { name: "lookup", arguments: {"id":"42"}, reply }` into the registered sender → `reply` oneshot receives the route's processed result (route = identity/set-body processor).
- `consumer_stop_unregisters`: after `stop`, `tool_registry.resolve("lookup")` → `None`.
- `resource_consumer_registers_uri`: resource consumer for URI `crm://customers` → `resource_registry.list_ready()` contains `crm://customers`.
- `consumer_start_rejects_129th_tool_with_camel_error`: server `crm` at default cap with 129 tool consumers constructed (identical trivial schema) → the 129th `McpConsumer::start` fails with `Err(McpError::CapExceeded)` mapped into a `CamelError` (spec scenario: 129th tool rejected — exercised through real consumer start, not registry internals).
- `raised_cap_starts_150_tool_consumers`: server `crm` declaring `max_tools: 200` → 150 tool consumers all `start` successfully on the shared listener (spec scenario: raised cap allows more tools).

**Acceptance:**
- `cargo test -p camel-component-mcp --test server_consumer_test` passes (7 tests).
- `cargo clippy -p camel-component-mcp -- -D warnings` exits 0.

- [x] 2.4

### Task 2.5: rmcp server adapter — protocol baseline + discover enforcement

**Files:**
- `crates/components/camel-component-mcp/src/adapter/server.rs` (new)
- `crates/components/camel-component-mcp/src/adapter/mod.rs` (modified)
- `crates/components/camel-component-mcp/src/registry.rs` (modified — `get_or_spawn` mounts the adapter service instead of the 2.2 placeholder)
- `crates/components/camel-component-mcp/tests/server_protocol_test.rs` (new)

**Steps:**
1. `src/adapter/server.rs`: `pub struct McpServerAdapter` implementing rmcp's `ServerHandler` over a `McpListenerHandle`:
   - `supported_protocol_versions() -> Cow::Borrowed(&[ProtocolVersion::V_2026_07_28])` — sole supported version; pre-version rejection is rmcp's inline guard returning JSON-RPC `-32022` whose `data` carries the supported list in rmcp 3.1.2's object form (`{"requested": <v>, "supported": ["2026-07-28"]}`). Add exactly one `tracing::warn!` per rejected request naming peer AND rejected version, via an axum response-inspection layer — rmcp 3.1.2's guard runs in a blanket `impl Service` before any `ServerHandler` method, so no handler seam exists; the layer detects `error.code == -32022` and warns (spec: Baseline protocol version; enforcement single-channel: the component emits the JSON-RPC error only and adds no HTTP-layer signal of its own — rmcp's internal HTTP framing is transport detail).
   - `server/discover` → `DiscoverResult` with a bind-derived server identity (`camel-mcp@<bind>` — one shared bind hosts many named servers, so the config name is not visible at the listener layer), capabilities (tools + resources), `["2026-07-28"]`.
   - `on_initialized` stays a no-op (legacy lifecycle never enabled). Never read or require `Mcp-Session-Id`.
   - Stub `tools/list`/`tools/call`/`resources/list`/`resources/read`/`prompts/list`/`resources/subscribe` handlers returning a temporary "not yet dispatched" method error — Tasks 2.6/2.7 replace each with real dispatch (staged stubs have named replacement tasks).
2. Mount the adapter as the axum service in `McpServerRegistry::get_or_spawn`'s spawn path (replacing Task 2.2's placeholder 404 service; adapter gets the handle's registries).
3. `tests/server_protocol_test.rs`: in-process server on ephemeral port built from a real `McpListenerHandle`; drive with raw HTTP JSON-RPC POSTs (version-rejection assertions) and rmcp's client (happy paths). Reuse `tests/common/mod.rs` helpers (`mod common;`) for the recorder and `warn_capture`.

**Tests:**
- `discover_advertises_only_2026_07_28`: rmcp client `server/discover` → supported == `["2026-07-28"]`, identity + capabilities present (spec scenario).
- `pre_2026_07_28_meta_rejected_with_32022`: raw JSON-RPC POST `tools/call` with `_meta` protocolVersion `2025-11-25` → response is error `-32022`, `data.supported` lists `["2026-07-28"]`; `warn_capture` saw one warn naming BOTH the peer (remote addr) and the rejected version (spec scenarios: rejection + warn).
- `legacy_initialize_does_not_open_session`: raw POST `initialize` then a follow-up POST with `2025-11-25` `_meta` → response headers contain no `Mcp-Session-Id`, follow-up rejected `-32022` (spec scenario).
- `session_header_not_required`: rmcp client `tools/call` with no `Mcp-Session-Id` header → request is dispatched on `_meta` strength alone (stub method error is acceptable here — the point is it is NOT rejected for a missing session header) (spec: Standard Streamable-HTTP request headers).

**Acceptance:**
- `cargo test -p camel-component-mcp --test server_protocol_test` passes (4 tests).
- `cargo clippy -p camel-component-mcp -- -D warnings` exits 0.
- `grep -rn "rmcp::" src/ --include='*.rs' | grep -v 'src/adapter/'` → zero hits.

- [x] 2.5

### Task 2.6: Tool dispatch — listing, readiness gating, schema validation, call

**Files:**
- `crates/components/camel-component-mcp/src/adapter/server.rs` (modified — replace `tools/list` + `tools/call` stubs)
- `crates/components/camel-component-mcp/Cargo.toml` (modified — `jsonschema = { workspace = true }`)
- `crates/components/camel-component-mcp/tests/server_tool_dispatch_test.rs` (new)

**Steps:**
1. `tools/list` → `tool_registry.list_ready()` projection: registered-but-not-ready tools are absent (spec: Readiness gating).
2. `tools/call {name, args}`: resolve in tool registry → `None` → clean MCP method error, no Exchange. Found → validate `args` against the entry's `input_schema` with the `jsonschema` crate (workspace dep, same usage as `crates/camel-processor`) → invalid → clean MCP schema-validation error, NO Exchange created (spec: Tool argument JSON Schema validation); valid → send `McpToolInvocation` (with `reply` oneshot) through the entry sender, await the reply, return it.

**Tests:**
- `tools_list_hides_not_ready_tool`: tool registered in the registry but NOT `mark_ready`ed (started consumer held pre-ready) → real rmcp client `tools/list` response omits the tool; then `mark_ready` → a second `tools/list` includes it with its input schema (spec scenario: not-ready tool is hidden from listing — observable at the host boundary).
- `valid_args_reach_route`: schema requires `{id: string}`; `tools/call {"id":"42"}` → route handler received `McpToolInvocation` with those args and its reply result is returned.
- `invalid_args_rejected_no_exchange`: `tools/call {"id":7}` → schema-validation MCP error; route handler receiver saw zero invocations (spec scenario).
- `unknown_tool_call_returns_clean_error`: `tools/call` for unregistered name → clean MCP method error, route handler saw zero invocations (spec: Tool and resource route registration).
- `stopped_tool_call_returns_clean_error`: register tool via a consumer, `stop` it, then `tools/call` → clean MCP method error, no dead-channel panic (spec scenario: stopped tool is unregistered).

**Acceptance:**
- `cargo test -p camel-component-mcp --test server_tool_dispatch_test` passes (5 tests).
- `cargo clippy -p camel-component-mcp -- -D warnings` exits 0.

- [x] 2.6

### Task 2.7: Resource dispatch + declined surfaces

**Files:**
- `crates/components/camel-component-mcp/src/adapter/server.rs` (modified — replace `resources/list` + `resources/read` + `prompts/list` + `resources/subscribe` stubs)
- `crates/components/camel-component-mcp/tests/server_resource_dispatch_test.rs` (new)

**Steps:**
1. `resources/list` → `resource_registry.list_ready()`; `resources/read {uri}` → resolve → `None` → clean MCP error, no Exchange (spec: Resource URI addressing); found → send `McpResourceRead` (with `reply`), await the `McpResource` reply, return its content.
2. `prompts/list` → respond prompts capability unavailable; `resources/subscribe` → respond unsupported (spec: v1 protocol surface).

**Tests:**
- `unknown_resource_read_rejected_no_exchange`: `resources/read` for never-registered URI → clean MCP error, no Exchange (spec scenario).
- `stopped_resource_read_rejected_no_exchange`: resource consumer started then stopped → `resources/read` for its URI → clean MCP error, no Exchange (spec scenario: stopped resource route rejected).
- `resources_list_advertises_uris`: resource consumer for URI `crm://customers` ready → rmcp client `resources/list` includes `crm://customers` (spec scenario).
- `prompts_list_declines`: prompts capability unavailable response.
- `resources_subscribe_declines`: unsupported response.

**Acceptance:**
- `cargo test -p camel-component-mcp --test server_resource_dispatch_test` passes (5 tests).
- `cargo clippy -p camel-component-mcp -- -D warnings` exits 0.

- [x] 2.7

### Task 2.8: Server consumer end-to-end integration test

**Files:**
- `crates/components/camel-component-mcp/tests/server_consumer_e2e_test.rs` (new)

**Steps:**
1. Full-stack test through `McpComponent` `create_endpoint` → `create_consumer`: DSL-free programmatic route (identity processor setting a deterministic body), server `crm` bound `127.0.0.1:0` with a `security_policy` value, one tool consumer + one resource consumer on that server.
2. Drive with a real rmcp Streamable-HTTP client against `handle.local_addr`: discover → list → call → read, asserting each stage (design.md Phase 2 exit-criteria).

**Tests:**
- `host_discovers_lists_calls_and_reads`: client connects via discover lifecycle → `tools/list` contains the tool with its input schema → `tools/call` returns the route's body → `resources/read` by `crm://customers` returns the route's body.
- `two_consumers_share_one_listener`: tool + resource consumers on same bind → `spawn_count == 1`, both callable.

**Acceptance:**
- `cargo test -p camel-component-mcp --test server_consumer_e2e_test` passes.
- Phase 2 exit-criteria met per design.md (discover==[2026-07-28], -32022 + warn, auth refuse, 129th cap — 2.1/2.4/2.5 cover the unit forms; this test covers the integration form).
- `cargo test -p camel-component-mcp` (whole crate) green.

- [x] 2.8

## Phase 3: DSL `mcp:` block lowering + metadata + bundle integration

### Task 3.1: camel-dsl `mcp:` AST types + parsing

**Files:**
- `crates/camel-dsl/src/mcp.rs` (new)
- `crates/camel-dsl/src/lib.rs` (modified — `pub mod mcp;`)
- `crates/camel-dsl/src/yaml.rs` (modified — accept `mcp:` block key where `rest:` is accepted)
- `crates/camel-dsl/src/json.rs` (modified — same for the JSON input format)

**Steps:**
1. `src/mcp.rs`: `pub struct RouteDslMcp { pub server: RouteDslMcpServer, pub tools: Vec<RouteDslMcpTool>, pub resources: Vec<RouteDslMcpResource> }`, `RouteDslMcpServer { name, bind, tls, security_policy, max_tools, max_resources }`, `RouteDslMcpTool { name, input_schema: serde_json::Value }`, `RouteDslMcpResource { name, uri }`. Serde derives with `deny_unknown_fields` (a `session` or `initialize` key in the block is a parse error — the DSL is transport- and version-agnostic with no session keys to lower, design.md Phase 3). Mirror `RouteDslRest` (declared in `crates/camel-dsl/src/rest.rs`) for serde style.
2. Wire the `mcp:` block into YAML/JSON deserialization exactly where the `rest:` block is accepted: `RouteDslRoutes` (the root document type in `crates/camel-dsl/src/route_ast.rs`/`model.rs` — locate the `rest` field with `grep -n "rest" crates/camel-dsl/src/route_ast.rs crates/camel-dsl/src/model.rs`) gains a sibling `mcp` field, deserialized in `yaml.rs` and `json.rs` at the same sites the `rest` field is read.
3. Unit tests in `src/mcp.rs` `#[cfg(test)]`.

**Tests:**
- `parse_mcp_block_from_yaml`: YAML `mcp:` block with server `crm` (bind, security_policy), one tool with input schema, one resource → parsed `RouteDslMcp` fields match.
- `unknown_server_key_rejected`: `session: true` under the server block → serde error.
- `defaults_caps_128`: omitting `max_tools` → 128.

**Acceptance:**
- `cargo test -p camel-dsl mcp` passes.
- `cargo clippy -p camel-dsl -- -D warnings` exits 0.

- [x] 3.1

### Task 3.2: Lowering `mcp:` blocks to consumer routes

**Files:**
- `crates/camel-dsl/src/mcp.rs` (modified — lowering fn + tests)
- `crates/camel-dsl/src/yaml.rs` (modified — call `expand_mcp_into` adjacent to `expand_rest_into` at ~line 110)
- `crates/camel-dsl/src/json.rs` (modified — same, adjacent to its `expand_rest_into` call at ~line 43)

**Steps:**
1. `pub fn lower_all_mcp_to_routes(blocks: &[RouteDslMcp]) -> Result<Vec<RouteDslRoute>, CamelError>` mirroring `lower_all_rest_to_routes` (`crates/camel-dsl/src/rest.rs:24-26` — same return/`CamelError` shape): each tool → a consumer route with `from = "mcp:<server>/tool/<name>?schema=<URL-encoded input JSON schema>"`, each resource → `from = "mcp:<server>/resource/<name>?uri=<URL-encoded MCP resource URI>"`. The schema and resource URI travel on the URI's query params — the channel Task 2.4's `McpEndpointUri` parses and Task 1.1's `uri_options` documents (structurally the same choice `rest.rs` makes putting `httpMethod` in the `from` query) — never as Exchange headers or body content (spec scenario: schema lives in operator config not on the wire). Empty block list → empty vec, no error.
2. Add `pub fn expand_mcp_into(routes: &mut Vec<RouteDslRoute>, blocks: &[RouteDslMcp]) -> Result<(), CamelError>` (mirroring `expand_rest_into`) and call it adjacent to every `expand_rest_into` invocation (`yaml.rs` ~line 110, `json.rs` ~line 43) so both lowerings run together on every parse path. NOTE: the only other caller of `lower_all_rest_to_routes` is `camel-cli/src/commands/openapi.rs:63` (OpenAPI-specific; do NOT wire mcp there).
3. Unit tests alongside the rest.rs tests style.

**Tests:**
- `dsl_block_lowers_to_consumer_routes`: `mcp:` block server `crm` + tool `lookup` with schema → lowered routes contain a route whose from starts `mcp:crm/tool/lookup?schema=` and whose `schema` query param URL-decodes to the declared schema (spec scenario).
- `resource_lowers_with_uri`: resource `customers` URI `crm://customers` → route from `mcp:crm/resource/customers?uri=` + URL-encoded `crm://customers`.
- `schema_not_in_headers_or_body`: the lowered route's headers/body template contain no schema value (spec scenario: schema lives in operator config).
- `expand_mcp_into_appends_to_routes`: existing routes vec + one mcp block → vec grows by tool+resource routes (wiring test for yaml/json call sites).

**Acceptance:**
- `cargo test -p camel-dsl mcp` passes (7 tests for the module).
- `cargo clippy -p camel-dsl -- -D warnings` exits 0.

- [x] 3.2

### Task 3.3: McpBundle + camel-cli registration + catalog

**Files:**
- `crates/components/camel-component-mcp/src/bundle.rs` (new)
- `crates/components/camel-component-mcp/src/lib.rs` (modified)
- `crates/camel-cli/Cargo.toml` (modified — optional dep + feature `mcp = ["dep:camel-component-mcp"]`)
- `crates/camel-cli/src/commands/run.rs` (modified — `register_bundle!(ctx, camel_config, camel_component_mcp::McpBundle);`)

**Steps:**
1. `src/bundle.rs`: `pub struct McpBundle { config: McpGlobalConfig }` implementing `ComponentBundle` with `config_key() == "mcp"` and `from_toml` deserializing `McpGlobalConfig` — copy `LlmBundle` in `crates/components/camel-component-llm/src/bundle.rs` shape, including fail-fast construction (invalid server/remote entries error at startup, not first use). `register_all` registers `McpComponent::new(config)` result.
2. `crates/camel-cli/Cargo.toml`: `camel-component-mcp = { workspace = true, optional = true }` + feature `mcp = ["dep:camel-component-mcp"]` (same pattern as the `llm` feature at lines 66/96-98).
3. `crates/camel-cli/src/commands/run.rs`: add `register_bundle!(ctx, camel_config, camel_component_mcp::McpBundle);` under a `#[cfg(feature = "mcp")]` guard matching how the llm registration at line 419 is guarded.
4. Verify `camel-catalog` discovery: run the catalog command that lists component metadata (`cargo run -p camel-cli -- catalog` — find the exact subcommand with `grep -rn "catalog" crates/camel-cli/src/commands/ | head`) and confirm the `mcp:` entry appears with both roles.

**Tests:**
- `bundle_registers_component`: test-registrar pattern (copy the `#[cfg(test)]` registrar from `crates/components/camel-component-llm/src/bundle.rs:36-62`) → `McpBundle::from_toml` minimal TOML → `register_all` registers a component whose `scheme() == "mcp"`.
- `bundle_rejects_unknown_keys`: TOML with `session = true` at the mcp config root → `from_toml` errors.

**Acceptance:**
- `cargo test -p camel-component-mcp bundle` passes.
- `cargo build -p camel-cli --features mcp` exits 0.
- Catalog listing shows `mcp` with `supports_consumer` + `supports_producer`.

- [x] 3.3

### Task 3.4: ADR-0060 + context documentation

**Files:**
- `docs/adr/0060-mcp-first-class-component.md` (new)
- `crates/components/CONTEXT.md` (modified — mcp entry)
- `CONTEXT-MAP.md` (modified — domain terms)
- `crates/components/camel-component-mcp/CONTEXT.md` (modified — final form)

**Steps:**
1. ADR-0060 charter, following the repo ADR template (read a recent short ADR, e.g. `docs/adr/0052-*.md`, for structure): MCP as first-class Server+Host component; route-owned tool dispatch (no auto-loop); DSL-lowered catalog; rmcp confined to `src/adapter/` (ADR-0020 pattern); Streamable HTTP only; protocol baseline `2026-07-28` stateless; exclusions (Prompts/stdio/sessions/legacy transports/subscriptions). Status Accepted. English prose (STE-leaning, per repo language policy).
2. `crates/components/CONTEXT.md`: add the `camel-component-mcp` entry in the same format as sibling entries, citing ADR-0060.
3. `CONTEXT-MAP.md`: add domain terms (MCP Server Consumer, MCP Client Producer, Tool Registry, Resource Registry, Shared Listener) with one-line definitions + ADR citations.
4. Crate `CONTEXT.md`: finalize (purpose, endpoints, config keys, phase-2 registries, adapter confinement), citing ADR-0060/0020/0032/0033/0038/0045/0052 where each governs.

**Tests:**
- `adr_and_context_citations_resolve`: (acceptance-gate verification, not a unit test) `cargo xtask lint-context-citations` exits 0 — every ADR/context citation added in these files must resolve.

**Acceptance:**
- `cargo xtask lint-context-citations` exits 0.
- `cargo xtask lint-log-levels` exits 0 (new docs mention `warn!` policy — no log-level drift).
- All four files exist and cite ADR-0060.

- [x] 3.4

### Task 3.5: DSL end-to-end acceptance test

**Files:**
- `crates/components/camel-component-mcp/tests/dsl_e2e_test.rs` (new)
- `crates/components/camel-component-mcp/Cargo.toml` (modified — dev-dependency `camel-dsl = { workspace = true }`)

**Steps:**
1. Test pipeline: YAML route document containing an `mcp:` block (server `crm` with `security_policy`, bind `127.0.0.1:0`, tool `lookup` with `{id: string}` schema, resource `customers` URI `crm://customers` + a processing step) → parse via the camel-dsl YAML entry point (the same API that runs `expand_rest_into`) → routes now include the lowered `mcp:` consumer routes via `expand_mcp_into` → start consumers through `McpComponent` → rmcp client against the bound port: `tools/list`, `tools/call {"id":"42"}`, `resources/read crm://customers` (design.md Phase 3 exit-criteria: declare → lower → start → host calls tool → result).
2. Assert the whole chain including that the schema reached the consumer through config (an invalid-args call `{"id":7}` is rejected by schema validation — proving the DSL-injected schema is live).

**Tests:**
- `dsl_block_runs_end_to_end`: declare → lower → start → `tools/call` returns the route's processed body; `resources/read` returns the resource route's body.
- `dsl_injected_schema_is_enforced`: `tools/call {"id":7}` → schema-validation MCP error, no Exchange created (proves schema came from DSL config).

**Acceptance:**
- `cargo test -p camel-component-mcp --test dsl_e2e_test` passes.
- `cargo test -p camel-component-mcp` whole crate green.
- `cargo clippy --workspace --all-features --exclude camel-cli --exclude camel-component-kafka --exclude security-keycloak --exclude security-wasm-policy -- -D warnings` exits 0 (workspace-level, mcp crate included).
- `openspec validate add-mcp-component --type change --json` passes (design.md Phase 3 exit-criteria).
- Phase 3 exit-criteria met per design.md.

- [x] 3.5
