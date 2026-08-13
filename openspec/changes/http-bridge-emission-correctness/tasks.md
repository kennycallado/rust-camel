# Tasks: http-bridge-emission-correctness

<!--
  Multi-phase change. The WHOLE plan is written and plan-blessed ONCE.
  PHASE 3 iterates the phase-groups in order. An inter-phase r_glm
  review runs after Phase 1 and Phase 2 (both have >= 2 tasks);
  Phase 3 and Phase 4 are single-task and skip the inter-phase review.

  NOTE: before dispatching the Phase 3 worker, the conductor escalates
  the bridgeEndpoint semantics decision to e_gpt (autopilot guardrail
  #3). The spec already chose Apache-Camel semantics (option 1).

  TDD contract (applies to every task): the worker writes the named
  tests FIRST, runs them to confirm they FAIL for the right reason,
  then implements until they pass. Each Tests block lists `command`
  (the cargo test invocation) and `expected` (the pre-implementation
  failure) where the failure is meaningful; unit tests on brand-new
  pure functions fail to compile until the function exists.
-->

## Phase 1: Policy foundation (ADR-0057 + shared classification module + producer fix)

### docs/adr

#### Task 1.1: Author ADR-0057 HTTP header emission policy

**Files:**
- `docs/adr/0057-http-header-emission-policy.md` (new)

**Steps:**
1. Create `docs/adr/0057-http-header-emission-policy.md` following the repo ADR template (match the structure of `docs/adr/0056-cache-repository-port.md`: Status, Context, Decision, Consequences).
2. State the three RFC-derived buckets: hop-by-hop/framing (RFC 7230 section 6.1: `connection`, `keep-alive`, `proxy-authenticate`, `proxy-authorization`, `te`, `trailer`, `transfer-encoding`, `upgrade`, plus `proxy-connection`), request-only (`host`, `user-agent`, `accept`, `accept-encoding`, `accept-language`, `accept-charset`, `accept-datetime`, `authorization`, `cookie`, `expect`, `from`, `if-match`, `if-modified-since`, `if-none-match`, `if-range`, `if-unmodified-since`, `max-forwards`, `range`, `referer`; note `proxy-authorization` belongs to hop-by-hop per RFC 7230 section 6.1 and is listed only there), and server-owned (`date`, RFC 7231 section 7.1.1.2).
3. State the dynamic Connection-named stripping rule (RFC 7230 section 6.1): any header named by a token in a `Connection` field value is treated as hop-by-hop in both directions; malformed or empty tokens are ignored; comparison is case-insensitive; tokens are de-duplicated.
4. State the direction rules: producer outbound request excludes hop-by-hop/framing, `content-length` (re-derived), `host` (destination-derived), and dynamic Connection-named headers; request-only headers ARE forwarded. Consumer response emission excludes hop-by-hop/framing, request-only, server-owned, `content-length` and `content-type` (re-derived), and dynamic Connection-named; it does NOT exclude `cache-control`, `pragma`, `warning`, `via`.
5. Record that ADR-0024 stays scoped to `PipelineOutcome`/status/body/Stop and is not amended.
6. Cross-reference rc-eoft, rc-2jj2, rc-d3o4, rc-f0cn.

**Tests:**
- `adr_0057_exists`: setup: read `docs/adr/0057-http-header-emission-policy.md`. action: grep for the section headings `Decision`, `Consequences` and the strings `hop-by-hop`, `request-only`, `server-owned`, `Connection`. assert: all present.

**Acceptance:**
- `docs/adr/0057-http-header-emission-policy.md` exists with Status, Context, Decision, Consequences sections.
- `cargo xtask lint-context-citations` exits 0 (the ADR cites issues correctly if the lint requires it).

- [x] 1.1

### crates/components/camel-http

#### Task 1.2: Create shared header-policy classification module

**Files:**
- `crates/components/camel-http/src/header_policy.rs` (new)
- `crates/components/camel-http/src/lib.rs` (modified) - add `mod header_policy;` declaration next to the existing `mod` declarations (lines 1-10).

**Steps:**
1. Create `src/header_policy.rs` defining three private constants:
   - `HOP_BY_HOP: &[&str]` = `["connection", "keep-alive", "proxy-authenticate", "proxy-authorization", "te", "trailer", "transfer-encoding", "upgrade", "proxy-connection"]`.
   - `REQUEST_ONLY: &[&str]` = `["host", "user-agent", "accept", "accept-encoding", "accept-language", "accept-charset", "accept-datetime", "authorization", "cookie", "expect", "from", "if-match", "if-modified-since", "if-none-match", "if-range", "if-unmodified-since", "max-forwards", "range", "referer"]` (`proxy-authorization` is intentionally excluded here; it lives in `HOP_BY_HOP` per RFC 7230 section 6.1).
   - `SERVER_OWNED: &[&str]` = `["date"]`.
2. Add `pub(crate) fn connection_tokens<'a, I: IntoIterator<Item = &'a str>>(connection_header_values: I) -> Vec<String>` that: splits each value on `,`, trims whitespace, lowercases, keeps only tokens that are valid RFC 7230 `token`s (one or more `tchar`: ASCII alphanumeric or one of ``! # $ % & ' * + - . ^ _ ` | ~``), de-duplicates preserving first-seen order, and never panics on malformed input.
3. Add `pub(crate) fn excluded_outbound(name: &str, connection_tokens: &[String]) -> bool` returning true when `name` (case-insensitive) is in `HOP_BY_HOP`, equals `content-length`, equals `host`, or is in `connection_tokens`.
4. Add `pub(crate) fn excluded_response(name: &str, connection_tokens: &[String]) -> bool` returning true when `name` (case-insensitive) is in `HOP_BY_HOP`, in `REQUEST_ONLY`, in `SERVER_OWNED`, equals `content-length`, equals `content-type`, or is in `connection_tokens`. Returns false for `cache-control`, `pragma`, `warning`, `via`.
5. Declare `mod header_policy;` (private) in `src/lib.rs` after line 5 (`pub mod health;`).
6. Add a `#[cfg(test)] mod header_policy_tests` (or co-locate unit tests in the module) covering every scenario below.

**Tests:** (all unit tests in `header_policy`)
- `excluded_outbound_strips_hop_by_hop`: setup: names `Connection`, `Transfer-Encoding`, `Upgrade`. action: call `excluded_outbound(name, &[])`. assert: all return `true`.
- `excluded_outbound_excludes_host_and_content_length`: setup: names `Host`, `Content-Length`. action: `excluded_outbound(name, &[])`. assert: `true`.
- `excluded_outbound_forwards_request_only`: setup: names `Accept`, `User-Agent`. action: `excluded_outbound(name, &[])`. assert: `false`.
- `excluded_outbound_dynamic_connection`: setup: `connection_tokens(&["X-Custom, Keep-Alive"])`. action: `excluded_outbound("X-Custom", &tokens)`. assert: `true`; `excluded_outbound("X-Other", &tokens)` assert `false`.
- `connection_tokens_casefold_dedup`: setup: value `"X-Custom, x-custom,  X-Custom "`. action: `connection_tokens([value])`. assert: returns `["x-custom"]` (single, lowercased).
- `connection_tokens_malformed_no_panic`: setup: value `"X-Custom, bad token, ,"` (plus an `X-Unrelated` header on the exchange). action: `let tokens = connection_tokens([value])`. assert: `tokens == ["x-custom"]` (`bad token` is not a valid `tchar` token because it contains a space, so it is dropped; empty tokens are dropped); no panic. Additionally `excluded_outbound("X-Unrelated", &tokens) == false` and `excluded_response("X-Unrelated", &tokens) == false` (unrelated headers are never dropped).
- `excluded_response_strips_hop_by_hop`: setup: names `Connection`, `Transfer-Encoding`, `Upgrade`. action: `excluded_response(name, &[])`. assert: all `true`.
- `excluded_response_keeps_cache_control_pragma_warning_via`: setup: names `Cache-Control`, `Pragma`, `Warning`, `Via`. action: `excluded_response(name, &[])`. assert: all `false`.
- `excluded_response_excludes_request_only_and_server_owned`: setup: names `User-Agent`, `Accept`, `Date`. action: `excluded_response(name, &[])`. assert: all `true`.
- `excluded_response_excludes_content_length_and_type`: setup: names `Content-Length`, `Content-Type`. action: `excluded_response(name, &[])`. assert: both `true`.
- `excluded_response_dynamic_connection`: setup: `connection_tokens(&["X-Custom"])`. action: `excluded_response("X-Custom", &tokens)`. assert: `true`.

**Acceptance:**
- `cargo test -p camel-component-http --lib header_policy` passes.
- `cargo clippy -p camel-component-http -- -D warnings` exits 0.
- `cargo fmt --check --all` exits 0.

- [x] 1.2

#### Task 1.3: Wire producer header loop to the classification module (rc-eoft)

**Files:**
- `crates/components/camel-http/src/lib.rs` (modified) - producer header-forwarding loop at ~L2107-2121.

**Steps:**
1. Before the `for (key, value) in &exchange.input.headers` loop (~L2107), compute `let conn_tokens = header_policy::connection_tokens(exchange.input.headers.iter().filter(|(k, _)| k.eq_ignore_ascii_case("connection")).filter_map(|(_, v)| v.as_str()));`.
2. In the loop condition, add `&& !header_policy::excluded_outbound(key, &conn_tokens)` alongside the existing `!key.starts_with("Camel")` and `!config.skip_request_headers.iter().any(|h| h.eq_ignore_ascii_case(key))` checks.
3. Confirm the outbound `Host` is now derived by reqwest from the destination URL (it is no longer pushed into `collected_headers`).
4. Leave the existing `connection_close` push (~L2150) and auth-header injection (~L2124) untouched; they are gated on `bridge_endpoint` and stay correct.
5. Add/extend producer unit tests in the `tests` module covering the three scenarios below.

**Tests:**
- `producer_excludes_host_and_framing`: setup: an exchange whose `input.headers` carry `Host: localhost`, `Content-Length: 42`, `Connection: keep-alive`, `Upgrade: h2c`; a producer endpoint pointing at the test capture server. action: run the producer once. assert: the captured outbound request has no `Host: localhost`, no `Content-Length` copied from the exchange, no `Connection`, no `Upgrade`; the outbound `Host` matches the capture-server address.
- `producer_forwards_request_only_headers`: setup: exchange headers `Accept: application/json`, `User-Agent: myclient/1.0`. action: run the producer. assert: the captured outbound request carries both `Accept` and `User-Agent`.
- `producer_honours_skip_request_headers`: setup: producer configured `skipRequestHeaders=Authorization`; exchange header `Authorization: Bearer x`. action: run the producer. assert: the outbound request carries no `Authorization`.

**Acceptance:**
- `cargo test -p camel-component-http --lib producer_` passes (covers `producer_excludes_host_and_framing`, `producer_forwards_request_only_headers`, `producer_honours_skip_request_headers`).
- `cargo clippy -p camel-component-http -- -D warnings` exits 0.

- [x] 1.3

## Phase 2: Response emission (consumer stops stripping valid response headers + CONTEXT.md)

### crates/components/camel-http

#### Task 2.1: Wire consumer reply-finalizer to the classification module (rc-2jj2)

**Files:**
- `crates/components/camel-http/src/lib.rs` (modified) - reply-finalizer header filter at ~L1589-1638.

**Steps:**
1. Extract the response-header selection (~L1589-1638) into a private, unit-testable helper `fn select_response_headers(headers: &HashMap<String, serde_json::Value>, user_content_type: Option<String>, inferred_content_type: Option<String>) -> Vec<(String, String)>` (the element type is `serde_json::Value`, defined at `crates/camel-api/src/message.rs:11`; `Value::as_str()` returns `Option<&str>`).
2. Inside `select_response_headers`: compute `let conn_tokens = header_policy::connection_tokens(headers.iter().filter(|(k, _)| k.eq_ignore_ascii_case("connection")).filter_map(|(_, v)| v.as_str()));`; collect headers where `!k.starts_with("Camel") && !header_policy::excluded_response(k, &conn_tokens)` via `v.as_str()`; then push `Content-Type` from `user_content_type.or(inferred_content_type)` if present.
3. Replace the inline finalizer block with a call to `select_response_headers(&out.input.headers, user_content_type, inferred_content_type)`.
4. Confirm the four valid response headers `cache-control`, `pragma`, `warning`, `via` now pass through (they are not in any bucket).
5. Add unit tests on `select_response_headers` and keep the predicate unit tests from Task 1.2 as the classification authority.

**Tests:**
- `response_emits_cache_control_via_pragma_warning`: setup: a header list containing `Cache-Control: public, max-age=3600`, `Via: 1.1 myproxy`, `Pragma: no-cache`, `Warning: 199 misc`. action: `select_response_headers(headers, None, None)`. assert: the returned vec contains all four names.
- `response_excludes_request_only_and_server_owned`: setup: header list containing `User-Agent: x`, `Accept: */*`, `Date: Thu, 01 Jan 2026 00:00:00 GMT`. action: `select_response_headers(headers, None, None)`. assert: none of `User-Agent`, `Accept`, `Date` appear.
- `response_re_derives_content_type`: setup: header list with `Content-Type: text/plain`, plus `user_content_type = Some("application/json")`. action: `select_response_headers(headers, Some("application/json".into()), None)`. assert: exactly one `Content-Type` entry with value `application/json`.
- `response_excludes_camel_headers`: setup: header list with `CamelHttpPath: /foo`, `Cache-Control: public`. action: `select_response_headers(headers, None, None)`. assert: `CamelHttpPath` absent, `Cache-Control` present.

**Acceptance:**
- `cargo test -p camel-component-http --lib response_` passes (covers `response_emits_cache_control_via_pragma_warning`, `response_excludes_request_only_and_server_owned`, `response_re_derives_content_type`, `response_excludes_camel_headers`).
- `cargo clippy -p camel-component-http -- -D warnings` exits 0.

- [x] 2.1

#### Task 2.2: Re-cite CONTEXT.md header policy to ADR-0057

**Files:**
- `crates/components/camel-http/CONTEXT.md` (modified).

**Steps:**
1. Locate the reply-header contract section (~L58) that cites `Per ADR-0024 and spec section 3.4`.
2. Replace the citation with `Per ADR-0057 (headers) and ADR-0024 (status/body/Stop)`.
3. Replace the mislabel `Hop-by-hop / request-side headers` with the three-bucket categorisation (hop-by-hop/framing, request-only, server-owned) and note that `cache-control`, `pragma`, `warning`, `via` are valid response headers and are emitted.
4. Add a one-line pointer to `docs/adr/0057-http-header-emission-policy.md`.

**Tests:**
- `context_md_cites_adr_0057`: setup: read `crates/components/camel-http/CONTEXT.md`. action: grep for `ADR-0057`. assert: present; grep for the old `Per ADR-0024 and spec` header-only citation assert absent in the reply-header section.

**Acceptance:**
- `cargo xtask lint-context-citations` exits 0.
- CONTEXT.md reply-header section names the three buckets and cites ADR-0057.

- [x] 2.2

## Phase 3: Bridge URL semantics (bridgeEndpoint gates resolve_url) — ESCALATE bridgeEndpoint decision to e_gpt before dispatching this task

### crates/components/camel-http

#### Task 3.1: Gate resolve_url on bridge_endpoint (rc-d3o4)

**Files:**
- `crates/components/camel-http/src/lib.rs` (modified) - `resolve_url` at ~L1970-2027, and the existing `test_resolve_url_combines_path_and_query_sources` test at ~L5228.

**Steps:**
1. At the top of `resolve_url`, after the existing `CamelHttpUri` override branch, add: when `config.bridge_endpoint == true`, return the endpoint base URL with only configured `query_params` applied (use the existing `url::Url` + `query_pairs_mut` pattern already present for the `config.query_params` branch), and do NOT append exchange `CamelHttpPath` or `CamelHttpQuery`. The `CamelHttpUri` branch remains an explicit override that precedes the bridging check (so an explicit `CamelHttpUri` header still wins even when `bridge_endpoint=true`); this precedence is intentional and matches Apache Camel.
2. When `config.bridge_endpoint == false` (default), keep the existing path/query merge behaviour unchanged.
3. Extend the unit tests in the `tests` module to cover the three scenarios below; leave `test_resolve_url_combines_path_and_query_sources` passing unchanged (it exercises the default branch).

**Tests:**
- `resolve_url_bridge_endpoint_true_ignores_exchange_path`: setup: an exchange with `CamelHttpPath=/foo`, `CamelHttpQuery=dropme=1`; config `base_url="http://x"`, `bridge_endpoint=true`, `query_params=[("token","secret")]`. action: `resolve_url(&exchange, &config)`. assert: the result is `http://x/?token=secret` (no `/foo`, no `dropme`).
- `resolve_url_bridge_endpoint_false_merges_path`: setup: same exchange; config `bridge_endpoint=false`. action: `resolve_url(&exchange, &config)` with `base_url="http://x"`. assert: the result contains `/foo` and `?dropme=1`.
- `resolve_url_bridge_endpoint_true_keeps_base_when_no_query_params`: setup: exchange with `CamelHttpPath=/foo`; config `base_url="http://x"`, `bridge_endpoint=true`, `query_params=[]`. action: `resolve_url(&exchange, &config)`. assert: the result is `http://x` with no `/foo`.

**Acceptance:**
- `cargo test -p camel-component-http --lib resolve_url` passes.
- `cargo clippy -p camel-component-http -- -D warnings` exits 0.

- [x] 3.1

## Phase 4: Composed acceptance (end-to-end bridge integration test)

### crates/components/camel-http

#### Task 4.1: Local deterministic http:->http: bridge integration test (rc-f0cn)

**Files:**
- `crates/components/camel-http/src/lib.rs` (modified) - add async integration tests in the `tests` module, alongside the existing `start_test_server_with_header_capture()` harness (~L4892) and the `create_producer` test helper (~L1922).

**Steps:**
1. Add a helper `async fn start_host_capturing_destination() -> (String, Arc<std::sync::Mutex<Option<(String, String)>>>, tokio::task::JoinHandle<()>)` modeled on `start_test_server_with_header_capture()`: bind `127.0.0.1:0`, parse the received raw request for the request line and the `Host:` header, store the tuple `(host_value, request_line)` into the returned `Arc<Mutex<Option<(String, String)>>>`, and reply `HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: N\r\n\r\n{"echo":"ok"}`.
2. Add `bridge_proxy_outbound_host_matches_destination`: build a producer via the `create_producer` test helper pointed at the destination URL with `bridge_endpoint=true`; construct an exchange whose `input.headers` carry `Host: localhost` and `CamelHttpPath=/foo`; run the producer once (await its result); read the captured `(host_value, request_line)` from the mutex.
3. Assert the captured `host_value` equals `127.0.0.1:{port}` (the Host header carries no scheme) and is NOT `localhost`; assert the captured request line path is the base path with no `/foo` appended (bridging honoured).
4. Add `bridge_proxy_route_set_response_header_survives`: start an `http:` consumer server on a free port via the component's `create_endpoint` + `create_consumer` path; set up the exchange/pipeline so the outbound `HttpReply` carries `Cache-Control: public, max-age=3600` (a `set_header` step or a header placed directly on the reply, matching how existing consumer tests build a reply); then send a real client request with `reqwest` to the consumer URL and read the HTTP response. Assert the client-received HTTP response carries `Cache-Control` on the wire (this is the wire-level assertion; do NOT short-circuit to an in-process `HttpReply` struct check).
5. Both tests are fully local and deterministic: raw TCP + in-process consumer + `reqwest` to `127.0.0.1`. No public CDN, no httpbin, no network egress. Abort the test (not panic-skip) if the destination capture mutex is empty after the producer call.

**Tests:**
- `bridge_proxy_outbound_host_matches_destination`: setup: `start_host_capturing_destination()` + producer `bridge_endpoint=true` + exchange `Host: localhost`, `CamelHttpPath=/foo`. action: run producer, read mutex. assert: captured `host_value` == `127.0.0.1:{port}` (no scheme), captured request line path has no `/foo`. command: `cargo test -p camel-component-http --lib bridge_proxy_outbound_host_matches_destination`. expected before implementation: the producer currently forwards `Host: localhost`, so this FAILS until Task 1.3 lands.
- `bridge_proxy_route_set_response_header_survives`: setup: consumer server + route setting `Cache-Control`. action: `reqwest` client GET to consumer URL. assert: response header `Cache-Control` present. command: `cargo test -p camel-component-http --lib bridge_proxy_route_set_response_header_survives`. expected before implementation: the consumer currently strips `Cache-Control`, so this FAILS until Task 2.1 lands.

**Acceptance:**
- `cargo test -p camel-component-http --lib bridge_proxy` passes (both tests, no network).
- `cargo clippy -p camel-component-http -- -D warnings` exits 0.

- [x] 4.1
