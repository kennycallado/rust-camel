# Tasks: mcp-list-result-cache-metadata

## camel-component-mcp (server adapter)

### Task 1.1: SEP-2549 cache metadata on tools/list and resources/list

**Files:**
- `crates/components/camel-component-mcp/src/adapter/server.rs` (modified)
- `crates/components/camel-component-mcp/tests/server_protocol_test.rs` (modified)

**Steps:**
1. In `src/adapter/server.rs`, add a private constant near `SUPPORTED_VERSIONS`
   (line ~73): `/// SEP-2549 non-cacheable list result: catalog is dynamic under
   readiness gating, so clients must not reuse list responses.` with value
   `const LIST_RESULT_TTL_MS: u64 = 0;`
2. In `list_tools` (~line 141-161), change the return to
   `Ok(ListToolsResult::with_all_items(tools).with_ttl_ms(LIST_RESULT_TTL_MS).with_cache_scope(CacheScope::Private))`
3. In `list_resources` (~line 228-243), apply the same builder chain to
   `ListResourcesResult::with_all_items(resources)`
4. Add `CacheScope` to the existing `rmcp::model::{...}` import list
5. In `tests/server_protocol_test.rs`, add three wire-level tests using the
   existing `raw_json_rpc_post` helper. New tests use fresh loopback IPs
   (127.0.0.6, 127.0.0.7, 127.0.0.8 — file convention 127.0.0.1-5 is taken).
   A list request carries BOTH the `_meta` version in params AND the
   `MCP-Protocol-Version` HTTP header — rmcp's tower layer
   (`validate_request_protocol_version_meta`) rejects a `_meta` version
   without the matching header before handler dispatch, so pass
   `&[("MCP-Protocol-Version", "2026-07-28")]` as extra headers (mirror of
   the existing pattern at lines 212-214). tools/list body:
   `{"jsonrpc":"2.0","id":9,"method":"tools/list","params":{"_meta":
   {"io.modelcontextprotocol/protocolVersion":"2026-07-28"}}}`;
   resources/list body: `{"jsonrpc":"2.0","id":9,"method":"resources/list",
   "params":{"_meta":{"io.modelcontextprotocol/protocolVersion":
   "2026-07-28"}}}`. Tool/resource registration uses the
   registry handle returned by `McpServerRegistry::global().get_or_spawn(...)`
   (pattern: `server_tool_dispatch_test.rs` lines 129-138 and
   `server_resource_dispatch_test.rs` line 137): create
   `let (tx, _rx) = tokio::sync::mpsc::channel::<McpToolInvocation>(8);` then
   `handle.tool_registry.register("lookup".to_string(),
   "cache-metadata-route".to_string(), tx, id_schema()).expect("tool must
   register")` (schema: `serde_json::json!({"type":"object","required":["id"],
   "properties":{"id":{"type":"string"}}})`) then `handle.tool_registry.
   mark_ready("lookup")`; resources: create
   `let (rtx, _rrx) = tokio::sync::mpsc::channel::<McpResourceRead>(8);` then
   `handle.resource_registry.register("crm://customers".to_string(),
   "cache-metadata-route".to_string(), rtx).expect("resource must register")`
   then `handle.resource_registry.mark_ready("crm://customers")`.

**Tests:** (executable spec — name, arrange, act, assert)
- `tools_list_carries_cache_metadata`: server on 127.0.0.6:0 with tool "lookup"
  registered + mark_ready → raw POST `tools/list` (meta 2026-07-28) → reply
  JSON has `result.ttlMs == 0`, `result.cacheScope == "private"`, and
  `result.tools` array containing name "lookup". Command:
  `cargo test -p camel-component-mcp --test server_protocol_test tools_list_carries_cache_metadata`
  Expected: FAIL before implementation (fields absent), PASS after.
- `resources_list_carries_cache_metadata`: server on 127.0.0.7:0 with resource
  "crm://customers" registered + mark_ready → raw POST `resources/list` (meta
  2026-07-28) → `result.ttlMs == 0`, `result.cacheScope == "private"`,
  `result.resources` contains the URI. Command:
  `cargo test -p camel-component-mcp --test server_protocol_test resources_list_carries_cache_metadata`
  Expected: FAIL before, PASS after.
- `tools_list_empty_catalog_still_carries_cache_metadata`: fresh server on
  127.0.0.8:0, nothing registered → raw POST `tools/list` (meta 2026-07-28) →
  `result.tools == []` AND `result.ttlMs == 0` AND
  `result.cacheScope == "private"` (presence independent of cardinality).
  Command:
  `cargo test -p camel-component-mcp --test server_protocol_test tools_list_empty_catalog_still_carries_cache_metadata`
  Expected: FAIL before, PASS after.

**Acceptance:**
- `cargo check -p camel-component-mcp --all-targets` exits 0
- `cargo test -p camel-component-mcp` exits 0 (all three new tests pass)
- `cargo fmt --check --all` exits 0
- `cargo clippy -p camel-component-mcp --all-targets -- -D warnings` exits 0

- [x] 1.1

### Task 1.2: legacy `initialize` rejected fail-closed (-32022)

**Files:**
- `crates/components/camel-component-mcp/src/adapter/server.rs` (modified)
- `crates/components/camel-component-mcp/tests/server_protocol_test.rs` (modified)

**Steps:**
1. In `src/adapter/server.rs`, add `InitializeRequestParams` and
   `InitializeResult` to the `rmcp::model` import list (as needed by the new
   method signature)
2. Add an `initialize` override to `impl rmcp::ServerHandler for
   McpServerAdapter` (after `discover`, ~line 137). Signature per rmcp 3.1.4
   trait (`handler/server.rs` line 318; `McpError` is an alias of
   `ErrorData`, so the error type is `rmcp::ErrorData`):
   `async fn initialize(&self, request: InitializeRequestParams, context:
   RequestContext<RoleServer>) -> Result<InitializeResult, ErrorData>`
3. Body: if `!SUPPORTED_VERSIONS.contains(&request.protocol_version)`, return
   `Err(ErrorData::unsupported_protocol_version(request.protocol_version,
   SUPPORTED_VERSIONS))` — the rmcp constructor that emits code `-32022` with
   `data.requested` + `data.supported`. Otherwise preserve the default
   handler's behavior: `context.peer.set_peer_info(request.clone()); Ok(
   self.get_info())` (get_info already sets protocol_version
   V_2026_07_28). Emit NO `warn!` — `warn_protocol_rejections` middleware
   already logs every -32022 exactly once
4. In `tests/server_protocol_test.rs`, add two tests on fresh loopback IPs
   (127.0.0.9, 127.0.0.10). Rejection-test body:
   `{"jsonrpc":"2.0","id":1,"method":"initialize","params":
   {"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":
   "test-client","version":"0.0.1"}}}`. Success-test body:
   `{"jsonrpc":"2.0","id":1,"method":"initialize","params":
   {"protocolVersion":"2026-07-28","capabilities":{},"clientInfo":{"name":
   "test-client","version":"0.0.1"}}}`. Initialize
   requests carry no `_meta` and need no `MCP-Protocol-Version` header
   (rmcp's tower layer exempts initialize from header/meta validation)

**Tests:** (executable spec)
- `legacy_initialize_rejected_fail_closed`: server on 127.0.0.9:0 → raw POST
  `initialize` with `protocolVersion: "2025-11-25"` → reply JSON has
  `error.code == -32022`, `error.data.supported == ["2026-07-28"]`,
  `error.data.requested == "2025-11-25"`, and NO top-level `"result"` key
  (no fallback success). Command:
  `cargo test -p camel-component-mcp --test server_protocol_test legacy_initialize_rejected_fail_closed`
  Expected: FAIL before (rmcp default answers success with server-default
  version), PASS after.
- `initialize_with_baseline_version_succeeds`: server on 127.0.0.10:0 → raw
  POST `initialize` with `protocolVersion: "2026-07-28"` → reply JSON HAS a
  top-level `"result"` whose `protocolVersion == "2026-07-28"` (guards
  against over-rejection of the baseline version). Command:
  `cargo test -p camel-component-mcp --test server_protocol_test initialize_with_baseline_version_succeeds`
  Expected: PASS before and after (regression guard).
- Existing `legacy_initialize_does_not_open_session` (preserved, unchanged):
  legacy initialize + follow-up `_meta` 2025-11-25 request → no
  `mcp-session-id` header, follow-up rejected -32022. Command:
  `cargo test -p camel-component-mcp --test server_protocol_test legacy_initialize_does_not_open_session`
  Expected: PASS before and after (regression guard for the no-session
  scenario; the override adds rejection, it does not open sessions).

**Preserved-test ownership (spec coverage):** the other two MODIFIED-spec
scenarios are owned by existing tests that this change must keep green,
untouched:
- `discover_advertises_only_2026_07_28` — scenario "server advertises only
  2026-07-28 via discover". setup: server on its existing 127.0.0.x bind →
  action: rmcp discover-lifecycle client connects and calls discover →
  assert: supported_versions == ["2026-07-28"].
- `pre_2026_07_28_meta_rejected_with_32022` — scenario "pre-2026-07-28
  request is rejected" incl. the exactly-one peer-scoped `warn!`. setup:
  server + warn capture → action: raw tools/call with `_meta` 2025-11-25 +
  matching header → assert: error -32022, `data.supported == ["2026-07-28"]`,
  exactly one warn naming peer + rejected version.
Command (whole file, covers both):
`cargo test -p camel-component-mcp --test server_protocol_test` exits 0.
Expected: PASS before and after.

**Acceptance:**
- `cargo check -p camel-component-mcp --all-targets` exits 0
- `cargo test -p camel-component-mcp` exits 0 (new + preserved tests)
- `cargo fmt --check --all` exits 0
- `cargo clippy -p camel-component-mcp --all-targets -- -D warnings` exits 0

- [x] 1.2

### Task 1.3: CONTEXT.md protocol-baseline note

**Files:**
- `crates/components/camel-component-mcp/CONTEXT.md` (modified)

**Steps:**
1. In the `## Protocol baseline` section, extend the sentence about rmcp's
   inline guard (currently "rmcp's inline guard rejects other peers with
   JSON-RPC `-32022`") to also state: a legacy `initialize` offer is answered
   `-32022` fail-closed by the adapter (no fallback to server default), and
   `tools/list` / `resources/list` results carry SEP-2549 cache metadata
   (`ttlMs: 0`, `cacheScope: "private"` — non-cacheable). Keep STE prose,
   ~2 added sentences, no new sections

**Tests:**
- `cargo xtask lint-context-citations` exits 0: setup = crate CONTEXT.md
  edited as in step 1 → action = run the lint → assert = exit code 0
  (CONTEXT.md structure/citations still compliant)

**Acceptance:**
- CONTEXT.md edited as described; no other sections touched; diff limited to
  `crates/components/camel-component-mcp/CONTEXT.md`
- `cargo xtask lint-context-citations` exits 0

- [x] 1.3
