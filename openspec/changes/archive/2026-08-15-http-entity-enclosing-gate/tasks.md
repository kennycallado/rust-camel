# Tasks: http-entity-enclosing-gate

## Task 1.1 — entity-enclosing gate in producer + direct-path tests

Files:
- `crates/components/camel-http/src/lib.rs` (modified)
- `crates/components/camel-http/Cargo.toml` (modified)

Steps:
1. In `crates/components/camel-http/Cargo.toml`, add `tracing-test = { workspace = true, features = ["no-env-filter"] }` to `[dev-dependencies]` (workspace root already declares `tracing-test = "0.2"`; the `no-env-filter` feature matches the convention used by the other consumer crates).
2. In `crates/components/camel-http/src/lib.rs`, inside the `HttpProducer` implementation, add:
   `fn is_entity_enclosing(method: &str) -> bool { matches!(method, "POST" | "PUT" | "PATCH") }`
3. In `HttpProducer::process`, immediately after `let method_str = HttpProducer::resolve_method(&exchange, &config);` and before the body materialization block, compute `let suppress_body = !HttpProducer::is_entity_enclosing(&method_str);`.
4. Rework the materialization block so that when `suppress_body` is true:
   - If the body is `Body::Stream`, still `std::mem::take` it (drop the stream) and emit the warn below. Set `is_stream_body` handling so the `wrap_stream` arm never runs and `materialized_body` stays `None`.
   - If the body is bytes, run the existing `std::mem::take` + `into_bytes` conversion. If the converted bytes are non-empty, emit the warn and set `materialized_body = None`; if empty, set `None` silently (no warn).
   - The warn is exactly one `warn!` per suppressed request: fields `correlation_id` (existing `exchange.correlation_id()` pattern) and `method = %method_str`, message naming the dropped body, e.g. `"dropping request body for non-entity-enclosing HTTP method"`. Follow ADR-0012 log-level policy for producer-owned warn sites.
   - The redirect branch and both direct-send arms receive `materialized_body = None` unchanged; no edits inside `ssrf::send_with_ssrf_safe_redirects`.
5. Add a test helper `start_capture_server() -> (String, tokio::task::JoinHandle<()>, Arc<Mutex<Vec<CapturedRequest>>>)` in the producer test module, modeled on the existing `start_redirect_server` (~L3152): a raw TCP listener where each connection parses the request head first, then performs a Content-Length-driven exact read of the body (do NOT read until EOF/shutdown — reqwest pools keep-alive connections and never sends FIN; do NOT rely on a single fixed-size read — a segmented small body would flake the regression test). Each request is parsed into:
   `struct CapturedRequest { method: String, path: String, body: Vec<u8>, content_length: Option<String>, transfer_encoding: Option<String> }`
   stored in the shared `Arc<Mutex<Vec<CapturedRequest>>>`. Respond `200 OK` with `Content-Length: 0` and an empty body, then drop the connection stream so each hop opens a fresh connection (same pattern as `start_redirect_server`).
6. Write the tests below in the same producer test module (they use `test_producer_ctx`, `HttpComponent::with_config`, `create_endpoint`, `producer.oneshot` exactly as `test_follow_redirects_false_does_not_follow` does; force the method via the `httpMethod` URI param on the endpoint, e.g. `format!("{url}?httpMethod=GET&allowInternal=true")`, and set the exchange body to `Body::Bytes(b"payload".to_vec().into())` or the crate's existing byte-body construction). The 200 response carries an empty body, so the returned exchange body is empty and the consumption assertion below is meaningful.

Tests (all in `crates/components/camel-http/src/lib.rs` test module):
- name: `test_get_with_body_sends_no_body_and_no_framing_headers`
  - setup: capture server running; endpoint `{url}?httpMethod=GET&allowInternal=true`; exchange with non-empty byte body.
  - action: `producer.oneshot(exchange)`.
  - assert: response completes with status 200; the single captured request has `method == "GET"`, `body.is_empty()`, `content_length.is_none()`, `transfer_encoding.is_none()`; after the send, the returned exchange body is empty (consumed).
  - command: `cargo test -p camel-component-http --lib test_get_with_body_sends_no_body_and_no_framing_headers`
  - expected: fails before the gate exists (body is sent today); passes after.
- name: `test_head_with_body_suppressed_via_header`
  - setup: capture server; endpoint `{url}?allowInternal=true`; exchange with non-empty byte body and header `CamelHttpMethod: HEAD`.
  - action: `producer.oneshot(exchange)`.
  - assert: captured request has `method == "HEAD"` and `body.is_empty()`.
  - command: `cargo test -p camel-component-http --lib test_head_with_body_suppressed_via_header`
  - expected: fails before; passes after.
- name: `test_delete_options_trace_with_body_suppressed`
  - setup: capture server; three endpoints built with `httpMethod=DELETE`, `httpMethod=OPTIONS`, `httpMethod=TRACE` respectively; three exchanges each with a non-empty byte body.
  - action: one `oneshot` per endpoint.
  - assert: each captured request has `body.is_empty()` and the matching method.
  - command: `cargo test -p camel-component-http --lib test_delete_options_trace_with_body_suppressed`
  - expected: fails before; passes after.
- name: `test_post_put_patch_with_body_still_sent`
  - setup: capture server; three endpoints with `httpMethod=POST`, `httpMethod=PUT`, `httpMethod=PATCH`; exchanges with non-empty byte bodies.
  - action: one `oneshot` per endpoint.
  - assert: each captured request has a non-empty body equal to the sent payload (regression guard).
  - command: `cargo test -p camel-component-http --lib test_post_put_patch_with_body_still_sent`
  - expected: passes before AND after (regression guard).
- name: `test_suppressed_body_logs_exactly_one_warn` (annotated `#[tracing_test::traced_test]` ABOVE `#[tokio::test]`, matching the in-repo precedent in camel-kafka/camel-sql tests)
  - setup: endpoint `{url}?httpMethod=GET&allowInternal=true`; exchange with non-empty byte body.
  - action: `producer.oneshot(exchange)`.
  - assert: use the `logs_assert!` lines-slice form from tracing-test 0.2.x — `logs_assert!(|lines: &[&str]| { let hits = lines.iter().filter(|l| l.contains("dropping request body") && l.contains("GET") && l.contains(&format!("cid={}", exchange.correlation_id()))).count(); ... })` adapted to whatever field format the warn! macro emits for the correlation id (inspect the actual line format in the assertion; a comment must state the mapping between warn fields and the matched substrings). Require exactly one hit. If captured lines do not include the "WARN" level token in this crate's subscriber format, filter on message + method + correlation-id substrings alone and note the deviation in the test comment.
  - command: `cargo test -p camel-component-http --lib test_suppressed_body_logs_exactly_one_warn`
  - expected: fails before (no warn exists); passes after.
- name: `test_empty_body_get_emits_no_warn` (annotated `#[tracing_test::traced_test]`)
  - setup: endpoint `{url}?httpMethod=GET&allowInternal=true`; exchange with empty body.
  - action: `producer.oneshot(exchange)`.
  - assert: zero lines containing "dropping request body" in the captured logs.
  - command: `cargo test -p camel-component-http --lib test_empty_body_get_emits_no_warn`
  - expected: passes before and after.

Acceptance:
- `cargo test -p camel-component-http --lib` exits 0.
- `cargo fmt --check` exits 0 for the touched files.
- `cargo clippy -p camel-component-http --all-targets -- -D warnings` exits 0.
- `cargo xtask lint-unwrap` reports no new unwrap in the added code.
- `rg -n 'is_entity_enclosing' crates/components/camel-http/src/lib.rs` shows the definition and exactly one call site in `process`.

- [x] 1.1

## Task 1.2 — stream and redirect suppression paths

Files:
- `crates/components/camel-http/src/lib.rs` (modified)

Steps:
1. Confirm Task 1.1's gate covers `Body::Stream`: when `suppress_body` is true and the body is a `Body::Stream`, the stream is taken and dropped, `materialized_body` is `None`, and the `wrap_stream` arm cannot execute. Adjust the gate if the stream arm can still run.
2. Extend `start_capture_server` (or add `start_redirect_capture_server`) so the server responds `307 Temporary Redirect` with `Location` pointing at a second path on the same listener, and the second path answers `200 OK`; both hops record `CapturedRequest`. Use a distinct first-hop path so hop 1 (307) and hop 2 (200) are distinguishable in the capture vector. Add a `308 Permanent Redirect` variant selectable by first-hop path (e.g. `/hop307` and `/hop308`).
3. Write the tests below using the existing redirect-test configuration pattern (`HttpConfig::default().with_follow_redirects(true)`, endpoint URI with `allowInternal=true`).

Tests:
- name: `test_stream_body_under_get_not_attached`
  - setup: capture server; endpoint `{url}?httpMethod=GET&allowInternal=true`; exchange whose body is a `Body::Stream` built the way `lib.rs` ~L4567-4569 builds it: a `StreamBody { stream, metadata }` whose items are `Result<Bytes, CamelError>` over a small in-memory byte source (follow that existing construction; do not invent an `std::io::Error`-typed stream — it will not compile).
  - action: `producer.oneshot(exchange)`.
  - assert: result is `Ok` (no `AlreadyConsumed` error surfaces); the captured request has `body.is_empty()`; the returned exchange body is empty (stream consumed).
  - command: `cargo test -p camel-component-http --lib test_stream_body_under_get_not_attached`
  - expected: fails before; passes after.
- name: `test_redirect_hops_never_replay_suppressed_body`
  - setup: redirect capture server with `/hop307` and `/hop308` first-hop paths; endpoint `{url}/hop307?httpMethod=GET&allowInternal=true` and a second endpoint for `/hop308`; exchanges with non-empty byte bodies.
  - action: `producer.oneshot` for both the 307 chain and the 308 chain.
  - assert: every `CapturedRequest` in both chains (first hop and final hop) has `body.is_empty()`; final status is 200.
  - command: `cargo test -p camel-component-http --lib test_redirect_hops_never_replay_suppressed_body`
  - expected: fails before; passes after.

Acceptance:
- `cargo test -p camel-component-http --lib` exits 0.
- `cargo clippy -p camel-component-http --all-targets -- -D warnings` exits 0.
- `cargo fmt --check` exits 0.

- [x] 1.2

## Task 1.3 — CONTEXT.md contract surface update

Files:
- `crates/components/camel-http/CONTEXT.md` (modified)

Steps:
1. In the `## Trust boundary and credential redaction` section (or the producer subsection that documents outbound behavior), add a short paragraph: the producer attaches the exchange body only for entity-enclosing methods (POST, PUT, PATCH); GET, HEAD, DELETE, OPTIONS, TRACE send no body and log one `warn!` when a non-empty body (or any stream body) is dropped; the body stays consumed; no configuration override exists. Cite ADR-0012 for the warn site in the existing `Log-level policy > warn! sites` list style.
2. Add the warn site to the `Log-level policy > warn! sites` list if that list enumerates producer warn sites.

Tests:
- name: `lint-context-citations`
  - setup: CONTEXT.md edited.
  - action: `cargo xtask lint-context-citations`
  - assert: exits 0 (no missing citation).
  - command: `cargo xtask lint-context-citations`
  - expected: pass.
- name: `context-doc-mentions-gate`
  - setup: CONTEXT.md edited.
  - action: `rg -n 'entity-enclosing' crates/components/camel-http/CONTEXT.md`
  - assert: at least one hit.
  - command: `rg -c 'entity-enclosing' crates/components/camel-http/CONTEXT.md`
  - expected: output >= 1.

Acceptance:
- `cargo xtask lint-context-citations` exits 0.
- `cargo xtask lint-log-levels` exits 0 (the new warn site follows the policy).

- [x] 1.3
