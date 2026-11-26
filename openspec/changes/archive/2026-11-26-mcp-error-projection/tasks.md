# Tasks: mcp-error-projection

Single-phase change (design.md has no `## Phases` section). Order: kernel
helper → http delegation → json-schema redaction → MCP fixes (each MCP task
reproduces its defect FIRST with a failing assertion, then fixes).

**Scenario coverage map** (9 scenarios → owning tasks):

| Spec scenario | Owning task(s) |
| --- | --- |
| business failure projects to the envelope | 4.1 |
| technical failure ships a generic body | 4.1 |
| resource-read failure uses the same projection | 4.2 |
| body materialization failure ships a generic body | 4.1 (tool), 4.2 (resource) |
| carried exchange error projects through the same contract | 4.1 (tool), 4.2 (resource) |
| oversized argument value is redacted | 4.3 |
| caller-invented member names are not echoed | 3.1, 4.3 |
| nested boolean subschema reports its keyword | 3.1, 4.3 |
| violation list is capped | 4.3 |

**Harness facts for Task 4.x** (verified against the worktree):

- The bridge under repair (`spawn_tool_bridge` / `spawn_resource_bridge`,
  `consumer.rs:481`/`:545`) is exercised ONLY through the
  `dsl_e2e_test.rs`-style harness: `camel_dsl::parse_yaml_to_declarative`
  over an `mcp:` YAML block → `consumer_for(&component, route.from)` →
  `consumer.start(ctx)` with `ConsumerContext::new(tx, CancellationToken,
  route_id)` → the test's route task receives `ExchangeEnvelope { exchange,
  reply_tx: Option<oneshot::Sender<Result<Exchange, CamelError>>>,
  in_flight_claim }` and answers via `reply_tx`:
  `send(Ok(mutated_exchange))` for success, `send(Err(error))` for failure,
  or `send(Ok(exchange_with_error_set))` for the carried-error path. The
  `server_tool_dispatch_test.rs` harness answers `McpToolResult` directly
  and BYPASSES the bridge — do not use it for these tests.
- A real `throw:` route reaches the bridge's `Err(e)` arm: the throw step
  fails the pipeline with `CamelError::Business` and the route controller
  forwards pipeline failures to `reply_tx` as `Err(e)` under
  `"aggregate:pipeline"` (`camel-core/src/lifecycle/adapters/
  route_controller.rs:1323-1330`). The seam reply
  `send(Err(CamelError::Business { .. }))` therefore drives the identical
  arm with the identical error value a real `throw:` route produces. The
  carried-arm tests cover the `Ok`-with-error path (ADR-0097 §3).
- `CamelError::ProcessorError(String)` Display embeds the wrapped text
  (`camel-api/src/error.rs:312`) — the production leak shape is
  `ProcessorError(format!("HTTP request failed: {e}"))`
  (`camel-http/src/ssrf.rs:373`), whose text carries the URL, the `ssrf`
  guard wording, and reqwest type names.
- WARN capture: `tests/common/mod.rs` `warn_capture()` currently records
  ONLY the `message` field (`MessageVisitor`, common/mod.rs ~387-401). The
  adapter logs nothing for a rejected call (`adapter/server.rs:208`); rmcp
  emits every handler error as `tracing::warn!(%id, ?error, "response
  error")` (rmcp-3.9.0 `src/service.rs:1625`) and Task 4.x boundary logs
  emit `error = %e` fields — so Task 4.1 first extends the capture with
  pinned field-aware semantics, and every WARN assertion (a) proves a
  matching event exists, then (b) asserts the leak string is absent from
  that event's captured fields.
- Bind discipline: `McpServerRegistry` is process-global and keyed by the
  bind string, and registration rejects duplicates (`src/registry.rs:720`
  "tool '{name}' is already registered", `:945` for resources). Every test
  in `tests/error_projection_test.rs` binds its own listener; in file order
  use `127.0.0.61:0` … `127.0.0.70:0` (ten tests; 61–70 are unused by every
  sibling test file, the highest in use is `127.0.0.52`). The same bind
  string goes in three places per test: the YAML `bind:`,
  `server_cfg(bind)`, and `McpServerRegistry::global().get_or_spawn(bind,
  &cfg)`.
- Client per test: payload assertions (Tasks 4.1, 4.2) go through
  `component_client(addr)` — copy the helper from `dsl_e2e_test.rs:138-152`
  into the new file, as `server_tool_dispatch_test.rs` does.
  `call_tool(name, args)` returns `McpToolResult { content:
  serde_json::Value, is_error: bool }` where `content` is the serialized
  content-block array — exactly one element,
  `{"type":"text","text":"<bridge payload>"}` — so the payload text is
  `result.content[0]["text"]` (`client_producer_test.rs:61`).
  `read_resource(uri)` returns `McpResource { content: Vec<u8>, mime_type:
  String }` with the wire MIME carried through. Raw `ErrorData` assertions
  (Task 4.3) use the raw peer `raw_rmcp_client(addr)`: the
  discover-lifecycle helper moved verbatim from `dsl_e2e_test.rs:119-134`
  into `tests/common/mod.rs` as `pub async fn raw_rmcp_client(addr:
  SocketAddr) -> RunningService<RoleClient, ()>`; a JSON-RPC error surfaces
  as `Err(ServiceError::McpError(ErrorData))`.
- The bridge reads the route's output from `out.input.body`
  (`consumer.rs:526`): set the reply body with
  `out.input.body = Body::Text(..)`, as `dsl_e2e_test.rs:214-215` does.
- Tools are lowered from YAML, so a schema built programmatically (Task
  4.3's 30-entry `required`) is rendered into the YAML text:
  `required: [` + the 30 names joined with `, ` + `]`.
- Test-wait discipline: `dsl_e2e_test.rs` waits are bounded and carry
  `// allow-test-wait` (`:206`, `:289-292`); new tests mirror that
  (`lint-unbounded-wait`).
- The oversize fixture: `StreamBody { stream: Arc<Mutex<Option<BoxStream>>>,
  metadata: StreamMetadata::default() }` per the `Body::Stream` doctest in
  `camel-api/src/body.rs`; `Result<Bytes, CamelError>: Clone` (manual impl,
  `camel-api/src/error.rs:474`) so `vec![Ok(..); N]` compiles; the failing
  variant is `CamelError::StreamLimitExceeded(usize)` whose Display text is
  `"Stream size exceeded limit: {0}"` (`error.rs:368`) — the variant NAME
  never appears in any payload, so Display text is the usable needle.

## camel-api (kernel)

### Task 1.1: shared ADR-0097 error-projection helper

**Files:**
- `crates/camel-api/src/error_projection.rs` (new)
- `crates/camel-api/src/lib.rs` (modified)

**Steps:**
1. Create `crates/camel-api/src/error_projection.rs` with:
   - `pub const INTERNAL_ERROR_BODY_JSON: &str = r#"{"error":"Internal Server Error"}"#;`
     (doc comment: generic non-business body, reuses camel-http's catch-all
     wording; ADR-0097 trust classes).
   - `pub fn business_envelope(code: &str, kind: &str) -> serde_json::Value`
     returning `serde_json::json!({ "error": code, "kind": kind })` — exactly
     two keys; doc comment citing ADR-0097 (`message`/`data` are log-only and
     never serialized into a client-visible body).
2. Add `pub mod error_projection;` to `crates/camel-api/src/lib.rs` (keep
   module docs complete — `cargo xtask lint-missing-docs-ratchet` must stay
   green).

**Tests:** (in-module `#[cfg(test)]` in `error_projection.rs`)
- `business_envelope_has_exactly_two_keys`: setup `code = "ORDER_NOT_FOUND"`,
  `kind = "not-found"` → call `business_envelope(code, kind)` → assert
  `serde_json::to_string(&value).unwrap()` equals
  `{"error":"ORDER_NOT_FOUND","kind":"not-found"}` and the value has no
  `message` or `data` key.
- `internal_error_body_is_exact_constant`: assert
  `INTERNAL_ERROR_BODY_JSON == r#"{"error":"Internal Server Error"}"#` and
  that it parses to a JSON object whose only key is `error`.

**Acceptance:**
- `cargo test -p camel-api --lib error_projection` passes.
- `cargo clippy -p camel-api --all-targets -- -D warnings` exits 0.
- `cargo xtask lint-missing-docs-ratchet` exits 0.

- [x] 1.1

## camel-component-http (mechanical delegation)

### Task 2.1: business_error_reply delegates to the shared helper

**Files:**
- `crates/components/camel-http/src/lib.rs` (modified)

**Steps:**
1. In `business_error_reply` (lib.rs:3860): replace the inline
   `serde_json::json!({ "error": code, "kind": kind })` construction with
   `serde_json::to_string(&camel_api::error_projection::business_envelope(code, kind))`,
   keeping the existing `unwrap_or_else(|_| "{}".to_string())` fallback and
   the `// allow-unwrap` marker. Status, headers, `failed` flag unchanged.
2. Update the function doc comment to state the body comes from the shared
   `camel_api::error_projection` helper (single projection, no duplicate).

**Tests:** (existing — must pass unchanged; the delegation is byte-identical
because serde_json's default map ordering already emits `error` before
`kind`. Package name is `camel-component-http` — the crate dir
`crates/components/camel-http` declares `name = "camel-component-http"`.)
- `code_map_wins_over_kind_map` (`src/lib_tests.rs:9172`): existing body
  assertion `{"error":"ORDER_NOT_FOUND","kind":"not-found"}` passes.
- `message_and_data_never_in_body` (`src/lib_tests.rs` ~9447): passes
  unchanged.
- `unmapped_kind_falls_back_500`: passes unchanged.

**Acceptance:**
- `cargo test -p camel-component-http` passes (all envelope-pinning tests
  green).
- `cargo clippy -p camel-component-http --all-targets -- -D warnings`
  exits 0.

- [x] 2.1

## camel-json-schema (additive redaction API)

### Task 3.1: redacted_violations accessor with instance-aware pointer redaction

**Files:**
- `crates/camel-json-schema/src/lib.rs` (modified)
- `crates/camel-json-schema/CONTEXT.md` (modified — record the shipped
  contract under `## Public surface`: `CompiledSchema::redacted_violations(
  &Value) -> Vec<RedactedViolation>` and `RedactedViolation { keyword,
  instance_path }`)

**Steps:**
1. Add to `crates/camel-json-schema/src/lib.rs`:
   - `pub struct RedactedViolation { pub keyword: String, pub instance_path:
     String }` (`#[derive(Debug, Clone, PartialEq, Eq)]`, doc comments; NO
     `Serialize` derive — assertions go through the public fields).
   - Store the parsed root document on `CompiledSchema` (new private field
     `root_schema: serde_json::Value` set at the single construction point —
     the `finish()` helper at `camel-json-schema/src/lib.rs:373` already
     holds the parsed `Value` before building the `jsonschema::Validator`).
   - `impl CompiledSchema { pub fn redacted_violations(&self, instance:
     &serde_json::Value) -> Vec<RedactedViolation> }` mapping each upstream
     `ValidationError`:
     - `keyword = error.kind().keyword().to_string()` (jsonschema 0.60.2
       `ValidationErrorKind::keyword()`, vendored `error.rs:342`; returns
       e.g. `"falseSchema"`, `"required"`, `"type"`, `"maxLength"`).
     - `instance_path = redact_pointer(&error.instance_path().to_string(),
       instance, &declared)` (step 3). `Location` is RFC 6901-escaped
       (vendored `paths.rs:867`), so segments arrive escaped.
2. Add a private `fn declared_property_names(schema: &serde_json::Value,
   out: &mut BTreeSet<String>)`: recursive walk inserting every key of every
   `"properties"` object, traversing BOTH objects and arrays (e.g.
   `items`/`prefixItems` hold subschemas), depth-capped at 32, non-container
   values skipped. Schema is trusted operator config (ADR-0032) — the walk
   is bounded and cheap.
3. Add a private `fn redact_pointer(path: &str, instance:
   &serde_json::Value, declared: &BTreeSet<String>) -> String` —
   **instance-aware walk**: split the pointer on `/` (drop the leading empty
   segment); track the current instance node starting at `instance`; for
   each segment:
   - node is `Value::Array` AND the segment parses as `usize` within bounds
     → keep the segment verbatim, descend to the indexed element;
   - node is `Value::Object` → decode the segment (`~1`→`/` then `~0`→`~`);
     if the decoded key is in `declared`, keep the ORIGINAL escaped spelling
     verbatim; otherwise emit `<prop>`; descend to
     `node.get(decoded)` if present;
   - node is a scalar, or the node is missing (walk left the known shape) →
     mask unless declared (decode-and-check as above), no descent.
   Any other case — including an `Array` node whose segment is not an
   in-bounds index — renders `<prop>` and stops descending. Fail closed:
   only the two pass-through rules above ever keep a segment. Re-join
   kept/placeholder segments with a leading `/`. Empty path renders as `""`
   (root). Add a comment: `~0`/`~1` unescaping is applied ONLY for the
   declared-set match; output preserves the escaped spelling, and
   over-redaction is the safe direction.
4. Do NOT touch the existing `Violation` struct or `violations()` — its four
   consumers (camel-processor `json_schema_validate.rs`, camel-validator
   `compiled.rs`, camel-mock `matcher.rs`, and the pre-existing MCP call
   site) keep byte-identical behavior until Task 4.3 switches the MCP call
   site.

**Tests:** (in-module `#[cfg(test)]`; helpers named exactly)
- `keyword_comes_from_kind_not_schema_path`: setup
  `compile(&SchemaSource::Inline(json!({"properties": {"value": false}})),
  SchemaSyntax::Auto, FormatMode::DraftDefault)` →
  `redacted_violations(&json!({"value": 1}))` → assert exactly one violation
  with `violation.keyword == "falseSchema"` (NOT `""`/`"properties"`/
  `"value"`) and `violation.instance_path == "/value"`.
- `undeclared_member_segment_is_masked`: schema
  `json!({"type":"object","patternProperties":{"^x-":{"type":"string"}}})`,
  instance `json!({"x-secret_marker_123": 5})` → assert the single violation
  has `violation.keyword == "type"` (the patternProperties CHILD failure
  carries the child keyword at the member pointer;
  `pattern_properties.rs:56` pushes the caller's key onto the child
  location) and `violation.instance_path == "/<prop>"` (contains no
  `secret_marker_123`).
- `declared_names_and_array_indices_pass_through`: schema
  `json!({"type":"array","items":{"type":"string"}})`, instance
  `json!(["ok", 5])` → assert `violation.keyword == "type"`,
  `violation.instance_path == "/1"`.
- `numeric_object_keys_are_redacted_but_array_indices_survive`: schema
  `json!({"type":"object","patternProperties":{"^[0-9]+$":{"type":"string"}}})`,
  instance `json!({"123456789": 5})` → assert the violation's
  `instance_path == "/<prop>"` (a numeric-looking OBJECT key must not ride
  the array-index rule) — regression companion to the `/1` array assertion
  above.
- `redaction_never_contains_instance_value`: schema
  `json!({"type":"object","properties":{"id":{"type":"string","maxLength":10}}})`,
  instance `json!({"id": "A".repeat(1_048_576)})` → assert on the returned
  violations' fields (no serialization): every `keyword` and
  `instance_path` contains no 16-char `"AAAA…"` slice of the instance
  value.

**Acceptance:**
- `cargo test -p camel-json-schema` passes (new tests + existing suite).
- `cargo build --workspace` succeeds (the four untouched consumers still
  compile).
- `cargo clippy -p camel-json-schema --all-targets -- -D warnings` exits 0.
- `cargo xtask lint-context-citations` exits 0.

- [x] 3.1

## camel-component-mcp (the fix)

### Task 4.1: field-aware WARN capture, then REPRODUCE + fix — tool bridge projection (business, technical, carried, materialization)

**Files:**
- `crates/components/camel-component-mcp/tests/common/mod.rs` (modified)
- `crates/components/camel-component-mcp/tests/error_projection_test.rs` (new)
- `crates/components/camel-component-mcp/tests/dsl_e2e_test.rs` (modified —
  local `rmcp_client` removed in favor of the common helper, now-unused
  imports pruned)
- `crates/components/camel-component-mcp/src/consumer.rs` (modified)
- `crates/components/camel-component-mcp/Cargo.toml` (modified — dev-deps
  added in workspace form: `futures = { workspace = true }`,
  `bytes = { workspace = true }`)
- `Cargo.lock` (updated by the dev-dep addition, if it changes)

**Steps:**
1. Extend `WarnCapture` (tests/common/mod.rs) additively, with PINNED
   matching semantics. Keep the three existing public methods and their
   behavior untouched, and never append field text to a message record (the
   existing `has_warn_containing` / `warn_count_containing` assertions must
   keep matching the `message` field alone). For every captured event also
   record its non-`message` fields — each rendered
   `format!("{}={:?}", field.name(), value)`, handled in BOTH `record_str`
   and `record_debug` in `MessageVisitor`, joined with `"; "`; an event
   with no other field stores `""`. The record and its field entry MUST be
   pushed under ONE lock (both vectors inside one `Mutex`), so index `i`
   always pairs an event with its own fields — tests run in parallel and
   two independent mutexes can interleave. Add
   `pub fn has_warn_field_containing(&self, event_needle: &str,
   field_needle: &str) -> bool` — true iff some index `i` has level `WARN`,
   the record's MESSAGE (the `message` field only) containing
   `event_needle`, and field entry `i` containing `field_needle`. Caller
   contract: `event_needle` is a STATIC tracing event message;
   `field_needle` is unique to the test, because the sink is shared by
   every test in the binary. An existence proof passes a `field_needle`
   that must be present; a leak-absence proof passes the leaked text.
   Existing tests stay green.
2. Move the raw peer helper into common: `pub async fn raw_rmcp_client(addr:
   SocketAddr) -> rmcp::service::RunningService<rmcp::service::RoleClient,
   ()>` with the verbatim body of `rmcp_client` (dsl_e2e_test.rs:119-134);
   re-point `dsl_e2e_test.rs` at the common helper (delete its local copy).
3. REPRODUCE FIRST: create `tests/error_projection_test.rs` with the FOUR
   Task 4.1 tests per the test specs below, against the UNMODIFIED
   consumer. Run
   `cargo test -p camel-component-mcp --test error_projection_test`; ALL
   FOUR tests must fail on their ASSERTION lines (content mismatch / leak
   found) — NOT on compilation or setup errors; a compile or setup failure
   is NOT reproduction evidence. Expected pre-fix failure shapes: business —
   content is `{"error":"business error ORDER_NOT_FOUND (not-found)"}` and
   carries the `customer 42` message; technical — content embeds the URL /
   `ssrf` / type-name text; propagated — call 1 answers
   `is_error == false` with body `"ignored"`; materialization — content is
   `{"error":"Stream size exceeded limit: 10485760"}`. Save the failing
   output to `/home/shared/tmp/mcp-error-projection/repro-tool.log` (with
   the command shown) and quote the assertion lines in the task report.
4. Add a private helper in `consumer.rs`:
   `fn project_failure(error: &CamelError) -> serde_json::Value` —
   `CamelError::Business { code, kind, .. }` =>
   `camel_api::error_projection::business_envelope(code, kind)`; any other
   variant =>
   `serde_json::from_str(camel_api::error_projection::INTERNAL_ERROR_BODY_JSON).expect("INTERNAL_ERROR_BODY_JSON must parse") // allow-unwrap`
   (single source of truth for the generic wording). The `// allow-unwrap`
   marker MUST sit on the `.expect(` line: `cargo xtask lint-unwrap` flags
   every `.expect(` outside test scope that lacks the marker on the same or
   the next line. (The warn log is emitted by the caller, not the helper —
   see step 5.)
5. In `spawn_tool_bridge`'s invocation task (consumer.rs ~495-533):
   - `Ok(out)` arm: BEFORE serializing the body, check
     `let carried = out.error.take();` — if `Some(e)`, emit
     `tracing::warn!(tool = <invocation's tool name field>, error = %e,
     "tool dispatch failed")` and produce `(project_failure(&e), true)`;
     if `None`, serialize the body as today (success stays success).
   - `Err(e)` arm: replace
     `(serde_json::json!({ "error": e.to_string() }), true)` with the same
     warn event + `(project_failure(&e), true)`.
   - `body_to_json`'s materialization `Err(e)` arm (consumer.rs:616): emit
     `tracing::warn!(tool = %tool, error = %e, "tool output materialization
     failed")` and return `(project_failure(&e), true)`; give `body_to_json`
     a `tool: &str` parameter (updated at its one call site) so the event
     names the tool. The message `"tool output materialization failed"` is
     exact and is NOT a substring of the resource arm's `"resource body
     materialization failed"` (nor vice versa): the WARN sink is shared by
     every test in the binary, so each event message must identify exactly
     one test.
   - Update the comments the fix makes stale: consumer.rs:520-524 (the
     is_error-flag comment), :596-604 (`body_to_json` doc), :621-631
     (`body_to_resource` doc — the error-shape prose).
6. Fix until all four tests pass; keep `isError`-flag semantics untouched
   (`error_shaped_success_content_stays_success_and_flag_drives_error` in
   `server_tool_dispatch_test.rs` must stay green).

**Tests:** (in `tests/error_projection_test.rs`; harness per the facts
block; `warn_capture()` installed BEFORE any client request; each test uses
its own bind from `127.0.0.61:0`…`127.0.0.70:0`; bounded waits carry
`// allow-test-wait` like `dsl_e2e_test.rs:206`)
- `business_failure_uses_adr0097_envelope` (bind `127.0.0.61:0`): setup —
  lowered tool route (`lookup`, id schema), consumer started,
  `component_client` connected; route task answers the invocation with
  `envelope.reply_tx.unwrap().send(Err(CamelError::Business { code:
  "ORDER_NOT_FOUND".to_string(), kind: "not-found".to_string(), message:
  "customer 42 is log-only".to_string(), data: None }))` (the same `Err`
  arm a real `throw:` route lands in via the `"aggregate:pipeline"`
  completion) → act — `call_tool("lookup", json!({"id": "42"}))` → assert
  `result.is_error == true`, `result.content[0]["text"]` equals exactly
  `{"error":"ORDER_NOT_FOUND","kind":"not-found"}`, and the serialized
  content contains neither the substring `customer 42` nor a `message` or
  `data` key.
- `technical_failure_body_is_generic` (bind `127.0.0.62:0`): same setup;
  route task answers with `send(Err(CamelError::ProcessorError("HTTP
  request failed: error sending request for url (http://10.0.0.138:9/setup):
  ssrf guard: ConnectError: tcp connect error".to_string())))` (the
  production leak shape from `camel-http/src/ssrf.rs:373` — the text
  carries the URL, the `ssrf` wording, and upstream type names) → act —
  call the tool → assert `result.is_error == true`,
  `result.content[0]["text"]` equals exactly
  `{"error":"Internal Server Error"}` (exact equality alone pins the
  payload), and the serialized content contains no `http://`, no `ssrf`,
  and no `ConnectError`; then
  `warn_capture().has_warn_field_containing("tool dispatch failed",
  "http://10.0.0.138:9")` is true — the event exists AND its `error` field
  carries the detail (the WARN-evidence direction; the client no-leak
  direction is the content assertions above).
- `propagated_exchange_error_uses_same_projection` (bind `127.0.0.63:0`):
  same setup; route task answers three calls — (1) `Ok(mut out)` with
  `out.error = Some(CamelError::Business { code: "SOLD_OUT".to_string(),
  kind: "failed".to_string(), message: String::new(), data: None })` and
  `out.input.body = Body::Text("ignored".to_string())`, (2) `Ok(mut out)`
  with `out.error = Some(CamelError::ProcessorError("HTTP request failed:
  error sending request for url (http://10.1.1.1:2/x): ssrf guard".
  to_string()))`, (3) `Ok(out)` with `error == None` and
  `out.input.body = Body::Text("ok".to_string())` → act — three tool calls
  → assert call 1 yields `is_error == true` with payload exactly
  `{"error":"SOLD_OUT","kind":"failed"}`; call 2 yields `is_error == true`
  with payload exactly `{"error":"Internal Server Error"}` and no
  `http://` in it; call 3 yields `is_error == false` with payload `"ok"`
  (handled-and-cleared stays success).
- `body_to_json_materialization_failure_is_generic` (bind `127.0.0.64:0`;
  wire-level in `tests/error_projection_test.rs`, like every other 4.x
  test — NOT a `--lib` unit test: `tests/common` is compiled only into
  integration-test binaries, so a unit test cannot reach `warn_capture()`):
  setup — lowered tool route (`blob`, input schema
  `{"type":"object"}`), consumer started, `component_client` connected,
  warn capture installed first; the route task answers the invocation with
  `Ok(mut out)` whose `out.input.body` is the oversize fixture, built
  inside the route task exactly like the `Body::Stream` doctest in
  `camel-api/src/body.rs`: `let chunks: Vec<Result<bytes::Bytes, CamelError>>
  = vec![Ok(bytes::Bytes::from_static(&[b'x'; 1024])); 11_000];` then
  `Body::Stream(camel_api::StreamBody { stream: Arc::new(tokio::sync::
  Mutex::new(Some(Box::pin(futures::stream::iter(chunks))))), metadata:
  Default::default() })` (11 MiB over the 10 MiB `MAX_STREAM_BODY_BYTES`)
  → act — `call_tool("blob", json!({}))` → assert
  `result.is_error == true`, `result.content[0]["text"]` equals exactly
  the string `{"error":"Internal Server Error"}`,
  `result.content.to_string()` does not contain
  `Stream size exceeded limit` (the Display text of
  `CamelError::StreamLimitExceeded` — the variant NAME never appears in any
  payload, so it is not a usable needle), and
  `warn_capture().has_warn_field_containing("tool output materialization
  failed", "Stream size exceeded limit")` is true — the detail lives in the
  log only.

**Acceptance:**
- `/home/shared/tmp/mcp-error-projection/repro-tool.log` exists and contains
  the PRE-FIX failing ASSERTION output (raw leaked strings), with commands
  shown.
- `cargo test -p camel-component-mcp --test error_projection_test` passes.
- `cargo test -p camel-component-mcp --test server_tool_dispatch_test` and
  `--test dsl_e2e_test` and `--test server_resource_dispatch_test` pass
  (flag semantics + bridge harness + resource harness untouched).
- `cargo fmt --check --all` and
  `cargo clippy -p camel-component-mcp --all-targets -- -D warnings` exit 0.
- `cargo xtask lint-unwrap` and `cargo xtask lint-log-redaction` exit 0.

- [x] 4.1

### Task 4.2: REPRODUCE + fix — resource bridge projection

**Files:**
- `crates/components/camel-component-mcp/tests/error_projection_test.rs` (modified)
- `crates/components/camel-component-mcp/src/consumer.rs` (modified)

**Steps:**
1. REPRODUCE FIRST: add `resource_failure_uses_same_projection` (spec
   below) against the UNMODIFIED resource arms; run
   `cargo test -p camel-component-mcp --test error_projection_test`; it
   must fail on its ASSERTION lines (not compile/setup). Save the failing
   output to `/home/shared/tmp/mcp-error-projection/repro-resource.log`
   with the command shown. The test drives all three resource arms: the
   `Err(e)` reply arm, the `Ok(out)`-with-`out.error`-set carried arm, and
   (via the Task 4.1 stream fixture, wire-level —
   `resource_materialization_failure_is_generic`) the materialization arm.
2. In `spawn_resource_bridge`'s read task (consumer.rs ~556-588):
   - `Err(e)` arm: replace the `e.to_string().into_bytes()` /
     `text/plain; charset=utf-8` body with
     `project_failure(&e).to_string().into_bytes()` and MIME
     `application/json`, preceded by
     `tracing::warn!(uri = %camel_api::redact::redact_url_fail_closed(&read.uri), error = %e, "resource read failed")`.
   - `Ok(out)` arm: check `out.error.take()` before materializing —
     `Some(e)` → the same warn event + `project_failure(&e)` bytes with
     `application/json`; `None` → `body_to_resource` as today.
3. In `body_to_resource`'s materialization `Err(e)` arm (consumer.rs:663):
   replace `e.to_string().into_bytes()` / text-plain with the generic
   envelope bytes + `application/json`; add
   `tracing::warn!(uri = %camel_api::redact::redact_url_fail_closed(uri), error = %e, "resource body materialization failed")`
   at that arm (the function already receives `uri`). `uri` is in
   `LOG_REDACTION_SENSITIVE` (`scripts/xtask/src/main.rs:4676`;
   CONTEXT-MAP.md "Credential redaction boundary", ADR-0051): a tracing
   argument that names it as a field or reads it as a member is a
   `lint-log-redaction` violation unless a `redact*` call redeems the same
   argument. Precedent: `src/registry.rs:950`. Do not use the
   `// allow-log-redaction` hatch.

**Tests:** (binds `127.0.0.65:0` and `127.0.0.66:0`)
- `resource_failure_uses_same_projection` (bind `127.0.0.65:0`): setup —
  lowered resource route (`crm://customers`), consumer started,
  `component_client` connected, warn capture installed first; route task
  answers three reads — (1)
  `send(Err(CamelError::Business { code: "DATA_LOCKED".to_string(), kind:
  "conflict".to_string(), message: String::new(), data: None }))`, (2)
  `send(Err(CamelError::ProcessorError("HTTP request failed: error sending
  request for url (http://10.9.9.9:1/customers): ssrf guard".to_string())))`,
  (3) `send(Ok(mut out))` carrying `out.error = Some(CamelError::Business {
  code: "STALE".to_string(), kind: "failed".to_string(), message:
  String::new(), data: None })` (carried arm) → act — three
  `read_resource` calls → assert read 1 content is exactly
  `{"error":"DATA_LOCKED","kind":"conflict"}` and read 2 content is exactly
  `{"error":"Internal Server Error"}`, both with MIME `application/json`,
  and read 2's client-visible content contains no `http://`; and
  `warn_capture().has_warn_field_containing("resource read failed",
  "http://10.9.9.9:1")` is true; and read 3 content is exactly
  `{"error":"STALE","kind":"failed"}` with MIME `application/json`.
- `resource_materialization_failure_is_generic` (bind `127.0.0.66:0`;
  wire-level in `tests/error_projection_test.rs`): setup — lowered resource
  route (`crm://blob`), consumer started, `component_client` connected,
  warn capture installed first; the route task answers the read with
  `Ok(mut out)` whose `out.input.body` is Task 4.1's oversize
  `Body::Stream` fixture → act — `read_resource("crm://blob")` → assert
  `String::from_utf8(resource.content)` equals exactly the string
  `{"error":"Internal Server Error"}`,
  `resource.mime_type == "application/json"`, the content does not contain
  `Stream size exceeded limit`, and
  `warn_capture().has_warn_field_containing("resource body materialization
  failed", "Stream size exceeded limit")` is true.

**Acceptance:**
- `/home/shared/tmp/mcp-error-projection/repro-resource.log` exists with the
  PRE-FIX failing assertion output and commands.
- `cargo test -p camel-component-mcp --test error_projection_test` and
  `--test dsl_e2e_test` pass.
- `cargo clippy -p camel-component-mcp --all-targets -- -D warnings` exits 0.

- [x] 4.2

### Task 4.3: REPRODUCE + fix — argument violation redaction + cap (wire-asserted)

**Files:**
- `crates/components/camel-component-mcp/src/adapter/server.rs` (modified)
- `crates/components/camel-component-mcp/tests/error_projection_test.rs` (modified)

**Steps:**
1. REPRODUCE FIRST: add `violations_exclude_instance_values` (spec below)
   against the UNMODIFIED `validate_arguments`; run
   `cargo test -p camel-component-mcp --test error_projection_test`; it
   must fail on its ASSERTION lines (the raw 1 MiB value / invented member
   name found in the wire payload — not a compile/setup failure). Save the
   failing output to
   `/home/shared/tmp/mcp-error-projection/repro-violations.log` with the
   command shown.
2. In `validate_arguments` (server.rs:523-534): switch to
   `schema.redacted_violations(arguments)` (Task 3.1's accessor); format
   each entry as `format!("{} at {}", violation.keyword,
   violation.instance_path)`; cap with `.truncate(25)` on the violation
   vector BEFORE the empty-check; add a comment citing the design cap
   ("25-entry cap — enough for schema authoring, bounded logs").
3. Update the EXISTING test `validate_arguments_violation_shape_unchanged`
   (server.rs:857): its expected violations vector changes from
   `vec![json!("\"name\" is a required property at ")]` to
   `vec![json!("required at ")]` — the new keyword-first format for the
   same missing-required case.
4. Leave the invalid-schema compile-error arm unchanged (it names only the
   tool and operator-authored schema text).

**Tests:** (binds `127.0.0.67:0`…`127.0.0.70:0`; wire-level: the raw peer
surfaces `ServiceError::McpError(ErrorData)` — assert `data.violations`
directly; each scenario is its own tool call against its own lowered tool)
- `violations_exclude_instance_values` (bind `127.0.0.67:0`; the
  mission-named test): setup — tool `big-value` with schema
  `json!({"type":"object","properties":{"id":{"type":"string","maxLength":10}}})`
  → act — call with `json!({"id": "A".repeat(1_048_576)})` (fits under
  rmcp's 4 MiB default request cap, `streamable_http_server/tower.rs:54`)
  → the raw peer's `ErrorData` carries code `-32602`; assert NO 16-char
  `"AAAA…"` slice of the value appears anywhere in the serialized
  `ErrorData`, and the violations entry equals exactly
  `"maxLength at /id"`; then prove the WARN log is covered AND clean, in
  this order, on rmcp's own event — the adapter logs nothing for a
  rejected call; rmcp logs every handler error as
  `tracing::warn!(%id, ?error, "response error")` (rmcp-3.9.0
  `src/service.rs:1625`), so the event message is the STATIC `"response
  error"` and the `ErrorData` rides in its `error` field: (a)
  `warn_capture().has_warn_field_containing("response error", "tool
  'big-value' arguments failed input schema validation")` is true — the
  event exists and carries THIS rejection; (b)
  `warn_capture().has_warn_field_containing("response error",
  "AAAAAAAAAAAAAAAA")` is false — the 16-char slice of the value is absent
  from the WARN log. The event is emitted before the response is sent, so
  both queries are deterministic once the client holds the error.
- `violations_mask_invented_member_names` (bind `127.0.0.68:0`): setup —
  tool `inventer` with schema
  `json!({"type":"object","properties":{"id":{"type":"string"}},"additionalProperties":false,"patternProperties":{"^x-":{"type":"string"}}})`
  → act — call with `json!({"id": "ok", "x-caller_invented_9f2e": 5})` →
  assert the violations contain `"type at /<prop>"` and the serialized
  `ErrorData` contains no `caller_invented_9f2e`; WARN, in this order:
  `has_warn_field_containing("response error", "tool 'inventer' arguments
  failed input schema validation")` is true, then
  `has_warn_field_containing("response error", "caller_invented_9f2e")`
  is false.
- `violations_keyword_from_false_subschema` (bind `127.0.0.69:0`): setup —
  tool `falsifier` with schema
  `json!({"type":"object","properties":{"value":false}})` → act — call with
  `json!({"value": 1})` → assert the violations entry equals exactly
  `"falseSchema at /value"` with no argument text.
- `violations_capped_at_25` (bind `127.0.0.70:0`): setup — tool `capped`
  whose schema's `required` array is built programmatically
  (`(1..=30).map(|i| format!("a{i}")).collect::<Vec<_>>()`, 30
  declared-but-missing properties) and rendered into the YAML
  (`required: [` + names joined `, ` + `]`) → act — call with `json!({})` →
  assert `data.violations` has length exactly 25 (30 produced, 25 kept) and
  every entry matches `"required at "` prefix.

**Acceptance:**
- `/home/shared/tmp/mcp-error-projection/repro-violations.log` exists with
  the PRE-FIX failing assertion output and commands.
- `cargo test -p camel-component-mcp` (the declared lane) passes fully.
- `cargo fmt --check --all` and
  `cargo clippy -p camel-component-mcp --all-targets -- -D warnings` exit 0.

- [x] 4.3

## Conductor-owned closeout

### Task 5.1: gate sweep + evidence record

**Files:**
- none (evidence goes to the park report at
  `/home/kenny/dev/rust-camel/.opencode/fleet/inbox/mcp-error-projection-parked.json`)

**Steps:**
1. Run the mission gates (order lines 94-106), each recorded with its exit
   code: `cargo fmt --check --all`; `cargo build --workspace`;
   `cargo clippy --workspace --all-features --all-targets -- -D warnings`
   with the AGENTS.md excludes (`camel-cli`, `camel-component-kafka`,
   `security-keycloak`, `security-wasm-policy`) plus the three follow-up
   clippy commands from the registry (`-p camel-component-kafka
   --all-targets`; `-p camel-cli`; `-p camel-cli --no-default-features
   --features flavor-regular,exec --all-targets`); `cargo test -p
   camel-component-mcp`; `bash scripts/gates-local.sh` as a transient
   `systemd-run --user --unit=gates-mcp-error-projection --collect` unit,
   polled in-turn; `openspec validate mcp-error-projection --type change
   --json`. (`gates-local.sh` parses the AGENTS.md registry and is the
   authoritative gate set.)
2. Any commit after the sweep forces a re-run of the sweep.
3. Record the full gate table, before/after payloads for the projection
   tests, redaction evidence, blessing verdicts, and unresolvable findings
   in the park report. Stop at the merge gate.

**Tests:** none (conductor-owned evidence task).

**Acceptance:**
- Every gate exit code recorded; sweep exit 0 (or an explicitly justified
  pre-existing-failure exemption with a bd follow-up).
- Park report complete per the mission order's Park section.

- [x] 5.1
