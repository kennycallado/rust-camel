# Design: mcp-request-deadline

## Approach

**The rmcp answer first** (the mission question: per-request or
transport-level?). rmcp 3.9.0 offers a **per-request** option only:
`PeerRequestOptions { timeout, .. }` applied through
`Peer::send_cancellable_request` + `RequestHandle::await_response`
(`service.rs:780-800`, `service.rs:555-584` — the `(Some, None, false)` arm
fires `ServiceError::Timeout` and sends `notifications/cancelled`). There is
no client-level default: `serve_with_lifecycle` takes no options struct, the
generated `call_tool`/`read_resource_once` hardcode
`PeerRequestOptions::no_options()` (`service/client.rs:1455-1465`), and the
transport config (`StreamableHttpClientTransportConfig`) bounds only
cancellation/reply POSTs (`control_request_timeout`, 5 s default) and session
recovery (`session_recovery_timeout`, 5 s) — its comment says so explicitly
(`streamable_http_client.rs:2114-2121`).

Using `send_cancellable_request` directly would bypass the component's
existing call paths: `RunningService::call_tool` drives SEP-2322
`input_required` (MRTR) rounds, and `read_resource_once` consults the
response cache. Re-implementing either is a protocol-behaviour change —
out of bounds. So the deadline is applied **at the component boundary**,
wrapping each existing call: two sites, listed.

**Option** — `McpRemoteConfig` (client/Producer role, the only role that
dials out) gains:

```rust
#[serde(default = "default_request_timeout_secs",
        deserialize_with = "deserialize_request_timeout_secs")]
pub request_timeout_secs: u64,
```

- Default `120`. Workspace convention is `*_secs: u64` (`exec`
  `timeout_secs`, `llm` `timeout_secs`); the component has no `Millis`
  precedent, so seconds it is. The behavioural test uses the 1 s floor (the
  option's unit is seconds; 200 ms is not expressible).
- Zero and non-numeric values (`""`) rejected at parse time by a custom
  deserializer (precedent: `non_empty_path` in the same file — direct `u64`
  deserialization cannot name the field), each message naming the option:
  `request_timeout_secs must be at least 1 (got 0)` and
  `request_timeout_secs must be an integer number of seconds of at least 1`
  → `McpError::Config` → `CamelError::Config`.
- **Default rationale**: remote tool calls are agent-shaped — the remote may
  itself run a multi-step LLM pipeline, so a legitimate call can take
  minutes. The default is an explicit operational cutoff, not a measured
  statistic: a stalled server costs 2 minutes instead of forever, while
  operators whose tools legitimately run longer raise the option (the
  option exists precisely because the cutoff is policy, not physics). The
  llm component defaults to no timeout (`timeout_secs: None`,
  `camel-component-llm/src/config.rs:33-36`), which is exactly the failure
  mode this change removes: an unbounded remote parks the Exchange for
  good.

**Application** — `RmcpClient` gains a `request_deadline: Duration` field,
set in `connect()` from `config.request_timeout_secs` (connect already
receives `&McpRemoteConfig`; the producer fetches the client from the live
map, so the deadline must travel with the client). A public accessor
`request_deadline()` exposes the effective value so an integration test can
assert the parsed default (absent option → 120 s) actually reaches the
connected client — parsing alone does not prove the adapter is bounded. Both
call sites wrap:

- `call_tool` (`adapter/client.rs:149-153`):
  `tokio::time::timeout(self.request_deadline, self.service.call_tool(params))`
- `read_resource` (`adapter/client.rs:168-172`):
  `tokio::time::timeout(self.request_deadline, self.service.read_resource_once(params))`

On `Elapsed`, the error is built at the call site, which has the name in
scope:

- `tool call '{tool}' exceeded the {n}s request deadline`
- `resource read '{uri}' exceeded the {n}s request deadline`

Dropped-mid-await hygiene is best-effort from rmcp:
`send_request_cancel_on_drop` installs an `AbandonedRequestGuard` whose
`Drop` calls `try_cancel_request(id, "request abandoned by the caller")`
(`service/client.rs:489-514`) — the component's deadline expiry therefore
sends a best-effort `notifications/cancelled`; delivery is not guaranteed
(the `try_send` result is ignored inside rmcp), and the local caller always
gets its error. No wire change.

**Error mapping** — both messages surface as `McpError::Endpoint(String)`,
the taxonomy's designated transport-error catch-all, mapping through the
existing `From<McpError> for CamelError` to
`CamelError::EndpointCreationFailed`; at runtime the producer's dispatch
failure classifies as `ProcessorError` (existing generic path, unchanged).
No new variant — the taxonomy doc forbids rmcp-specific variants.

**Connect/DNS honesty** — both startup phase limits get named, as separate
5 s budgets (never framed as one overall startup deadline):

- DNS resolution (`dns_pin.rs:176-180`), our text:
  `timed out resolving remote host '{host}' after 5s`.
- TCP connect: `map_connect_error` gains an arm for
  `ClientInitializeError::TransportError` — its
  `DynamicTransportError.error` field is public
  (`rmcp transport.rs:242-247`). The boxed error is NOT a bare
  `reqwest::Error`: the streamable-http client transport is
  `WorkerTransport<StreamableHttpClientWorker<C>>`
  (`streamable_http_client.rs:2016`), whose `Transport::Error` is the
  worker's `StreamableHttpError<C::Error>`
  (`streamable_http_client.rs:1126`), and reqwest errors enter it as
  `StreamableHttpError::Client(e)`
  (`transport/common/reqwest/streamable_http_client.rs:28-31`). The
  downcast therefore targets
  `StreamableHttpError<reqwest::Error>` (public at
  `rmcp::transport::streamable_http_client::StreamableHttpError`),
  matching the `Client(cause)` arm; when
  `cause.is_connect() && cause.is_timeout()` the error becomes
  `connect to remote '<server>' exceeded the 5s TCP connect limit`
  (cause preserved in the message). Other transport errors keep the
  existing catch-all passthrough — not every startup error is a timeout.

Residual filed to bd as **rc-9kpye**: the connect-time `server/discover`
POST itself is unbounded (rmcp's 10 s `auto_discover_timeout` applies only
to Auto-mode probing; the component uses Discover mode directly). Bounding
it is a connect-time change, outside this mission's post-connect scope.

**Tests** — extend the existing fake (`tests/common/mod.rs::spawn_mock`)
with a `stall: bool` knob: `MockHandler::call_tool`/`read_resource` await
`std::future::pending()` when set (discover still answers, so connect
succeeds). New `tests/request_deadline_test.rs` (per-file 127.0.0.x bind
convention) covers: stalled call trips the deadline naming tool + bound
(assert the error message AND that elapsed wall time is under a generous
scheduler-tolerance watchdog — e.g. under 10 s for a 1 s deadline — not an
exact-duration assertion, which flakes); stalled read trips it naming the
URI; a fast call with an explicit deadline succeeds; and with the option
absent, `connect` yields a client whose `request_deadline()` is 120 s
(default reaches the adapter). Connect-honesty: a unit test on
`map_connect_error` feeding a `reqwest::Error` connect-timeout downcast
asserts the message names the 5 s TCP limit (the fixture constructs the
REAL wrapper — a `StreamableHttpError::Client(reqwest_error)`, boxed as
rmcp boxes it — not a bare boxed `reqwest::Error`, which would falsely
validate a wrong-type downcast); a DNS-timeout unit test
asserts `after 5s`. Config tests in `config.rs` cover default / explicit /
zero-rejected / empty-rejected. Literal `McpRemoteConfig` constructors in
tests gain the field (mechanical).

## Affected crates

- `camel-component-mcp`: `config.rs` (option + deserializer + tests),
  `adapter/client.rs` (deadline field + accessor + two wrapped sites +
  connect-error arm), `adapter/dns_pin.rs` (one error message),
  `tests/common/mod.rs` (stall knob), `tests/request_deadline_test.rs`
  (new), `docs/src/components/mcp.md`, component `CONTEXT.md` (option
  documented alongside the existing config surface).

## Architecture boundaries

Component-internal. The `McpClient` trait seam (ADR-0020 pattern) carries no
new types — the deadline lives in the adapter, behind it. No Runtime, DSL,
Services, Languages, or Functions change; `camel-api` untouched. The
`deny_unknown_fields` config surface grows by one documented key.

## Phases

Single-phase: one option, two wrapped call sites, docs, delta — one
coherent slice.
