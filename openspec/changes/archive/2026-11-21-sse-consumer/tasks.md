# Tasks: sse-consumer

Single-phase change. All paths relative to repo root. New symbols are
introduced in Task 1.1 and reused by later tasks unchanged.

## Task 1.1 — SSE framing parser

- [x] 1.1

**Files**
- `crates/components/camel-http/src/sse.rs` (new — parser section)
- `crates/components/camel-http/src/lib.rs` (modified — `mod sse;` wiring only; the test module is wired from sse.rs per step 6)

**Steps**
1. In `crates/components/camel-http/src/sse.rs`, add `pub(crate) struct SseEventFrame { pub event: Option<String>, pub id: Option<String>, pub retry_ms: Option<u64>, pub data: String }` and `pub(crate) enum SseFrame { Event(SseEventFrame), Comment }`.
2. Add `pub(crate) struct SseFrameParser` with `pub(crate) fn new(max_event_bytes: usize) -> Self` (holds an internal `Vec<u8>` line buffer and per-event accumulator), `pub(crate) fn push(&mut self, chunk: &[u8]) -> Result<Vec<SseFrame>, CamelError>` and `pub(crate) fn finish(&mut self) -> Result<Vec<SseFrame>, CamelError>` (EOF: DISCARDS any unterminated block — events dispatch only on the blank-line delimiter per the SSE grammar; a stream that ends mid-block emits nothing for it).
3. Implement the state machine: split incoming bytes on `\n`, `\r\n`, or lone `\r`; strip field names/values per the SSE grammar (`event:`, `id:`, `retry:`, `data:`, optional single leading space after the colon); a line starting with `:` is a comment (feeds `SseFrame::Comment`); a blank line dispatches the accumulated block — events with empty data are suppressed (no frame emitted); multi-line `data:` values join with `\n`, final `\n` dropped; `event` defaults to `message` at EXCHANGE mapping time (the frame keeps `Option<String>`); `retry:` parses as u64 milliseconds (invalid or absent → `None`); an `id:` longer than 1024 bytes is kept on the frame but marked so it is never stored for resend (`SseEventFrame` gains `pub id_over_cap: bool`, set when `id.as_bytes().len() > 1024`).
4. Strip exactly one UTF-8 BOM (EF BB BF) if it appears in the very first bytes fed to the parser; BOM byte sequences seen later pass through verbatim.
5. Enforce `max_event_bytes` while accumulating: when the event's accumulated data bytes exceed the cap, return `Err(CamelError::Io("sse event exceeds maxEventBytes bound"))` immediately without buffering beyond the cap (non-retryable task error class).
6. Write the unit tests listed below in the `sse_tests.rs` module (new file `crates/components/camel-http/src/sse_tests.rs`, `#[cfg(test)]`, wired from sse.rs with the `#[path]` include pattern the crate uses (`#[cfg(test)] #[path = "lib_tests.rs"] mod tests;` at lib.rs:4213-4216, `#[path = "tls_tests.rs"]` at tls.rs:911) — i.e. `#[cfg(test)] #[path = "sse_tests.rs"] mod sse_tests;` inside sse.rs).

**Tests**
- `sse_parser_one_exchange_worth_of_frames_per_event` — setup: `SseFrameParser::new(1024)`; action: `push(b"data: hello\n\n")`; assert: one `SseFrame::Event` with `data == "hello"`. command: `cargo test -p camel-component-http sse_parser -- --nocapture`; expected: FAIL before impl (module missing), PASS after.
- `sse_parser_multiline_data_joins_with_newline_no_trailing` — action: `push(b"data: a\ndata: b\ndata: c\n\n")`; assert: `data == "a\nb\nc"` (no trailing `\n`). Same command; expected: FAIL→PASS.
- `sse_parser_comment_only_block_yields_comment_frame` — action: `push(b": keep-alive\n\n")`; assert: exactly one `SseFrame::Comment`, no Event. Same command; expected: FAIL→PASS.
- `sse_parser_empty_data_block_suppressed` — action: `push(b"event: tick\n\n")` (block with no data line) AND `push(b"data:\n\n")` (block with an empty data value); assert: no frame of either kind for both blocks. Same command; expected: FAIL→PASS.
- `sse_parser_headers_event_id_retry_captured` — action: `push(b"event: tick\nid: 42\nretry: 5000\ndata: x\n\n")`; assert: `event == Some("tick")`, `id == Some("42")`, `retry_ms == Some(5000)`. Same command; expected: FAIL→PASS.
- `sse_parser_invalid_retry_field_is_none` — action: feed `retry: abc`; assert: `retry_ms == None`. Same command; expected: FAIL→PASS.
- `sse_parser_crlf_cr_lf_equivalent` — action: feed the same three-event stream once with `\r\n`, once with lone `\r`, once with `\n` line endings; assert: identical frame lists. Same command; expected: FAIL→PASS.
- `sse_parser_bom_stripped_only_at_stream_start` — action: feed `\xEF\xBB\xBFdata: a\n\n` then `data: \xEF\xBB\xBFb\n\n`; assert: first event data == `"a"`, second event data contains the BOM bytes verbatim. Same command; expected: FAIL→PASS.
- `sse_parser_split_across_pushes` — action: push `b"data: he"`, then `b"llo\n\nevent: t\ndata: x\n\n"` in two calls; assert: second call yields both events intact. Same command; expected: FAIL→PASS.
- `sse_parser_oversize_event_errors_within_bound` — setup: `SseFrameParser::new(8)`; action: `push(b"data: 1234567890123\n\n")`; assert: returns `Err` (CamelError::Io mentioning maxEventBytes) and buffered bytes never exceed the cap. Same command; expected: FAIL→PASS.
- `sse_parser_eof_discards_unterminated_block` — setup: `SseFrameParser::new(1024)`; action: `push(b"data: partial")` (no blank line), then `finish()`; assert: no frames emitted (the unterminated block is discarded, not dispatched at EOF). command: `cargo test -p camel-component-http sse_parser -- --nocapture`; expected: FAIL→PASS.
- `sse_parser_oversize_id_marked_not_stored` — setup: parser cap 1 MiB; action: feed `id: <1100 bytes>\ndata: x\n\n`; assert: frame delivered with `id_over_cap == true`; a normal `id: 42` frame has `id_over_cap == false`. Same command; expected: FAIL→PASS.
- `sse_parser_lf_only_field_without_space` — action: feed `data:x\n\n` (no space after colon); assert: `data == "x"`. Same command; expected: FAIL→PASS.

**Acceptance**
- `cargo test -p camel-component-http sse_parser` passes.
- `cargo fmt --check --all` and `cargo clippy -p camel-component-http --all-targets -- -D warnings` exit 0.
- No new dependency added to `crates/components/camel-http/Cargo.toml` (git diff on the manifest is empty).
- No `unwrap()` outside tests (`cargo xtask lint-unwrap` exit 0).

## Task 1.2 — `sse:` URI config, component and endpoint registration

- [x] 1.2

**Files**
- `crates/components/camel-http/src/sse.rs` (modified — config + component + endpoint section)
- `crates/components/camel-http/src/bundle.rs` (modified — register `SseComponent`)
- `crates/components/camel-http/src/sse_tests.rs` (modified — config tests)

**Steps**
1. In `sse.rs`, add `#[derive(..., UriConfig)] #[uri_scheme = "sse"] pub(crate) struct SseUriConfig` (follow the `HttpEndpointUriConfig` derive pattern at lib.rs:461-499, INCLUDING the `#[uri_config(metadata(...), crate = "camel_component_api")]` metadata block the derive requires, lib.rs:469-478) with `#[uri_param]` fields: `maxEventBytes` (default `1048576`), `idleTimeoutMs` (default `90000`), `connectTimeoutMs` (default `10000`), `retryInitialMs` (default `3000`), `retryMaxMs` (default `60000`), `allowInternal` (default `false`), `allowCleartext` (default `false`), `authHeaders` (default empty; format `Name1=Value1,Name2=Value2`, values percent-decoded by the query parser).
2. Add `pub(crate) struct SseComponent` holding `Arc<HttpConfig>` and its OWN `Arc<PinnedClientCache>` constructed via `PinnedClientCache::new(PINNED_CLIENT_TTL, PINNED_CLIENT_MAX_ENTRIES)` (client_cache.rs:37/:47) — exactly the pattern every existing component uses (`HttpComponent::new`/`with_config` each build their own cache instance, lib.rs:2577/:2591/:2672/:2686). The client_cache SEAM (type, TTL, pinning semantics, `get_or_build`) is reused unchanged; no shared-instance rewiring of HttpComponent constructors and no `lib.rs` constructor changes. `HttpBundle::register_all` (bundle.rs:25-28) additionally registers `Arc::new(SseComponent::new(self.config.clone()))` alongside the existing http/https registrations. Implement `Component for SseComponent` with `scheme() -> "sse"`, metadata capabilities `consumer` only, and `create_endpoint` returning `SseEndpoint`.
3. In `SseEndpoint::create_endpoint` construction, reject URLs whose authority carries userinfo (`url::Url::parse` + non-empty `url.username()` or `url.password()`) with `Err(CamelError::EndpointCreationFailed("sse: URL must not carry userinfo".to_string()))`. Parse `authHeaders` into `Vec<(HeaderName, HeaderValue)>` at this point; invalid header names/values fail endpoint creation with `EndpointCreationFailed`.
4. `SseEndpoint` implements `Endpoint`: `create_consumer` returns `Ok(Box::new(SseConsumer::new(...)))`. In THIS task, `SseConsumer` is a compiling skeleton: `new(config, shared deps)` stores fields, and the `Consumer` impl has `start` returning `Err(CamelError::Config("sse consumer loop not yet implemented"))` and `stop` as a no-op, so the crate compiles green at the task boundary; the real loop replaces this in Task 1.3.
5. `create_producer` returns `Err(CamelError::Config("sse: is consumer-only"))`.
6. Config clamping/validation at endpoint creation: clamp `retryInitialMs` and `retryMaxMs` into [1000, 60000] with `retryMaxMs >= retryInitialMs`; `maxEventBytes` must be > 0; invalid values fail endpoint creation with `Config`.

**Tests**
- `sse_config_defaults_pinned` — setup: parse `sse://127.0.0.1:1/stream` with default `HttpConfig`; action: inspect resolved config; assert: maxEventBytes 1_048_576, idleTimeoutMs 90_000, connectTimeoutMs 10_000, retryInitialMs 3_000, retryMaxMs 60_000. command: `cargo test -p camel-component-http sse_config`; expected: FAIL before impl, PASS after.
- `sse_endpoint_rejects_userinfo_url` — action: `create_endpoint` with `sse://user:pass@127.0.0.1:1/stream?allowInternal=true`; assert: `Err(EndpointCreationFailed)` mentioning userinfo. Same command; expected: FAIL→PASS.
- `sse_endpoint_rejects_invalid_auth_headers` — action: authHeaders value with an invalid header name (`Bad Header=x`); assert: `Err(EndpointCreationFailed)`. Same command; expected: FAIL→PASS.
- `sse_retry_bounds_clamped_at_endpoint_creation` — action: parse with `retryInitialMs=100&retryMaxMs=999999`; assert: resolved values are 1000 and 60000. Same command; expected: FAIL→PASS.
- `sse_endpoint_producer_rejected` — action: `create_producer`; assert: `Err(Config)` mentioning consumer-only. Same command; expected: FAIL→PASS.
- `sse_component_registered_by_bundle` — setup: a `ComponentRegistrar` test double (same pattern the bundle tests use for HttpBundle registration) + `HttpBundle::from_toml(toml::Value::Table(toml::map::Map::new()))`; action: `register_all`; assert: a component with scheme `"sse"` is registered (and `http`/`https` remain). command: `cargo test -p camel-component-http sse_component`; expected: FAIL→PASS.

**Acceptance**
- `cargo test -p camel-component-http sse_` passes (one shared filter running every `sse_*` unit test added in this task).
- `cargo clippy -p camel-component-http --all-targets -- -D warnings` exits 0; `cargo xtask lint-component-deps` exits 0 (no camel-core dependency added to src).
- `SseConsumer` skeleton compiles; the crate has no broken task boundary (`cargo build -p camel-component-http` exits 0).

## Task 1.3 — SSE consumer: connect, stream loop, back-pressure, cancellation

- [x] 1.3

**Files**
- `crates/components/camel-http/src/sse.rs` (modified — consumer section)
- `crates/components/camel-http/src/sse_tests.rs` (modified)

**Steps**
1. Implement `Consumer for SseConsumer` following the camel-mqtt pattern (crates/components/camel-mqtt/src/consumer.rs:118-200): `start` spawns the loop task via `tokio::spawn`, stores its handle for `background_task_handle()`, uses `ConsumerStartupMode::Explicit`, calls `ctx.mark_ready()`, and clones the cancel token from `ctx.cancel_token()`.
2. Connect phase (one function `connect_once(&self) -> Result<reqwest::Response, CamelError>`), executed identically on every attempt. SCHEME TRANSLATION (pinned): the request URL is the configured `sse://host/path` translated to `https://host/path` for the wire (SSE rides HTTP; secure by default); plain `http://` translation is permitted when the existing SSRF policy allows cleartext for that target — loopback/`allowInternal=true` targets per the existing producer-test precedent (producer tests hit `http://127.0.0.1:{port}` with only `allowInternal=true`), and public hosts only with `allowCleartext=true` (ADR-0081). An `HttpEndpointConfig` is constructed from the translated URL (defaults, with `allowInternal`/`allowCleartext` mapped from `SseUriConfig`) and feeds the existing seams unchanged. The translated URL is used consistently for `ssrf::validate_url_for_ssrf` (ssrf.rs:32 — which allowlists http/https only, so validation runs on the TRANSLATED URL unchanged; ssrf.rs is NOT modified), `ssrf::resolve_initial_url_for_ssrf` (ssrf.rs:273) for DNS pinning, the `PinnedClientCache` host key, and `ssrf::send_with_ssrf_safe_redirects` (ssrf.rs:331) with `reqwest::Method::GET`, header set = configured `authHeaders` + `Accept: text/event-stream` + `Last-Event-ID` when a stored id exists; connect timeout = `connectTimeoutMs`.
3. Response gates after connect: status 204 → terminal `Err(CamelError::HttpOperationFailed { method: "GET", url: <redacted via camel_api::redact::redact_url>, status_code: 204, .. })`, never reconnected; any other non-2xx → same terminal variant; `Content-Type` not starting with `text/event-stream` → terminal `Err(CamelError::Io("sse: unexpected Content-Type, expected text/event-stream"))`, never reconnected.
4. Stream loop: read the response body stream chunk-wise; feed chunks into the `SseFrameParser`; a tokio::time::timeout of `idleTimeoutMs` wraps EACH read — timeout → close the connection and treat as retriable transport failure (Task 1.4 loop reconnects); ANY bytes received (comments included) reset the idle timer (this falls out of per-read timeout).
5. Frame dispatch: `SseFrame::Comment` → reset retry budget (Task 1.4 counter) and continue; `SseFrame::Event` → reset retry budget; build `Exchange::new(Message::new(Body::Text(frame.data)))` (InOnly by default per camel-api/src/exchange.rs:109-119), set headers `CamelSseEvent` (frame.event unwrap_or `"message"`), `CamelSseId` (only when present), `CamelSseRetry` (only when present, as string); store `id` for resend unless `id_over_cap`; then `ctx.send(exchange).await?` BEFORE reading further bytes — no internal channel (ADR-0044).
6. Malformed input: parser `Err` (oversize) → increment metric error label `b-prime:sse:oversize` via the runtime observability handle (`increment_errors(&route_id, "b-prime:sse:oversize")`), return the error as a task error (non-retryable, propagates out of the loop); invalid UTF-8 — convert each event's data with `String::from_utf8` semantics on the accumulated bytes and on `Err` increment `b-prime:sse:utf8` and return a non-retryable task error naming the deviation from U+FFFD replacement.
7. Cancellation: every await in the loop (stream read and send) selects on the cancel token with `biased` (mqtt consumer.rs:250-313 pattern); on cancellation stop immediately and return `Ok(())` with no further reads or exchanges.

**Tests** (drive `SseConsumer` directly against `HttpTestServer` from `camel-component-api` test_support — `Responder::staged`/`chunked` for streams; construct the consumer via the component's `create_consumer` on an `sse://127.0.0.1:{port}/events?allowInternal=true` endpoint; collect exchanges through the `ConsumerContext` double the same way camel-mqtt/camel-redis consumer tests do)
- `sse_consumer_emits_one_exchange_per_event` — setup: server staged response `Content-Type: text/event-stream`, chunks `b"data: a\n\n"`, `b"data: b\n\n"`, then channel drop; action: start consumer, await 2 exchanges; assert: 2 exchanges, bodies `a`,`b`, both `Body::Text`, MEP InOnly. command: `cargo test -p camel-component-http sse_consumer`; expected: FAIL before impl, PASS after.
- `sse_consumer_header_mapping` — setup: one event `event: tick\nid: 42\nretry: 5000\ndata: x\n\n`, one event without fields; action: await 2 exchanges; assert: first has CamelSseEvent=tick, CamelSseId=42, CamelSseRetry=5000; second has CamelSseEvent=message and no CamelSseId/CamelSseRetry. Same command; expected: FAIL→PASS.
- `sse_consumer_terminal_wrong_content_type` — setup: response 200 `Content-Type: text/html`; action: start consumer; assert: start's background task finishes with terminal Err mentioning Content-Type, no reconnect request recorded beyond the first (`server.recorded()` count == 1). Same command; expected: FAIL→PASS.
- `sse_consumer_backpressure_next_read_waits_for_send` — setup: staged stream sends event 1, then a consumer whose `ConsumerContext::send` future is gated on an unresolved oneshot; action: assert via a probe that no second chunk is requested (receiver closed / read pending) until the gate opens; assert: after opening the gate event 2 flows. Same command; expected: FAIL→PASS (the worker may implement the gate with a `tokio::sync::Barrier`/oneshot on the send double — the observable contract is: no read past event 1 dispatch until send completes).
- `sse_consumer_oversize_event_task_error_with_metric` — setup: staged stream sends a data line larger than `maxEventBytes=64` param; action: await task result; assert: Err mentioning maxEventBytes, error metric label `b-prime:sse:oversize` incremented once (assert via the observability double). Same command; expected: FAIL→PASS.
- `sse_consumer_invalid_utf8_task_error_with_metric` — setup: staged stream sends `data: \xFF\xFE\n\n`; assert: Err naming UTF-8 (no U+FFFD replacement anywhere in the message), label `b-prime:sse:utf8` incremented once. Same command; expected: FAIL→PASS.
- `sse_consumer_cancel_mid_read_stops_promptly` — setup: staged response sends headers then one event then stays open with no bytes; action: start consumer, await first exchange, fire cancel token, await task; assert: task returns within 1 s (`tokio::time::timeout`), no further exchange. Same command; expected: FAIL→PASS.
- `sse_consumer_sends_configured_headers_and_accept` — setup: endpoint with `authHeaders=X-Api-Key=secret`; staged `text/event-stream` with one event; action: start consumer, await the exchange; assert via `server.recorded()` that the request carried `Accept: text/event-stream` and `X-Api-Key: secret`, and no `Last-Event-ID` header (no id stored yet). Same command; expected: FAIL→PASS.
- `sse_consumer_json_data_stays_text` — setup: staged stream sends `data: {"a":1}\n\n`; action: await the exchange; assert: body is `Body::Text` equal to the raw `{"a":1}` string (no JSON conversion, no structured body). Same command; expected: FAIL→PASS.
- `sse_consumer_idle_timeout_includes_comments` — setup: endpoint with `idleTimeoutMs=500`; staged stream sends a comment `:k\n\n` every 300 ms for 2 s (longer than the idle bound); action: start consumer, wait 2 s; assert: no reconnect occurred (`recorded()` count == 1) and the consumer is still running (comment bytes reset the idle timer). Same command; expected: FAIL→PASS.
- `sse_consumer_tls_allowlist_pinned_host` — setup: the existing TLS harness (`src/tls_harness.rs`, `#[cfg(test)]` module in-crate; pattern from `src/tls_tests.rs` / `src/tls_allowlist_tests.rs`, rcgen dev-dep) serving an event stream over TLS with an expired certificate for the host, the expired-cert allowlist entry configured exactly as the `https:` producer tests do; action: start the consumer on an `sse://` TLS URL for that host; assert: connect succeeds and events flow (allowlist honored through the shared `build_client` path). command: `cargo test -p camel-component-http sse_consumer_tls`; expected: FAIL→PASS.
- `sse_connect_redirect_strips_authorization_cross_origin` — setup: two `HttpTestServer` instances (different ports on 127.0.0.1 = different origins); server A responds 302 with `Location` pointing at server B; server B serves a staged `text/event-stream` with one event; endpoint configured with `authHeaders=Authorization=Bearer sekrit`; action: start consumer, await the event; assert: server B's recorded request has NO `Authorization` header (ADR-0081 cross-origin strip, implemented by the reused `send_with_ssrf_safe_redirects` seam). Same command; expected: FAIL→PASS.

**Acceptance**
- All `sse_` tests pass: `cargo test -p camel-component-http sse_` (covers every pinned name in this block, including `sse_connect_redirect_strips_authorization_cross_origin` and `sse_consumer_tls_allowlist_pinned_host`).
- `cargo fmt --check --all`; `cargo clippy -p camel-component-http --all-targets -- -D warnings`; `cargo xtask lint-metric-labels`; `cargo xtask lint-unbounded-wait`; `cargo xtask lint-test-sleep`; `cargo xtask lint-no-free-port`; `cargo xtask lint-no-raw-http-test-server` all exit 0.
- The `http:` producer path is untouched: `git diff main...HEAD -- crates/components/camel-http/src/lib.rs` shows no change inside `impl Service<Exchange> for HttpProducer` (`read_timeout`/`max_response_bytes` block, lib.rs:3604-3645).

## Task 1.4 — SSE consumer: resubscription loop

- [x] 1.4

**Files**
- `crates/components/camel-http/src/sse.rs` (modified — reconnect loop)
- `crates/components/camel-http/src/sse_tests.rs` (modified)

**Steps**
1. Wrap the Task 1.3 stream loop in the reconnect loop: on retriable failure (transport error, 5xx status, idle-timeout fire, connect-timeout fire) gate with `NetworkRetryPolicy::should_retry(attempt)` (camel-component-api/src/network_retry.rs:193); when the budget refuses, return `Err(CamelError::Io("sse: retry budget exhausted after N attempts"))` as a task error (ADR-0007 — route supervision owns recovery, the consumer does not self-supervise).
2. Sleep `policy.delay_for(attempt)` where the policy uses `retryInitialMs`/`retryMaxMs` clamped to [1000, 60000] (Task 1.2), defaulting to 3 s when no server `retry:` override is in effect; a parsed `retry:` field from the stream overrides the delay for subsequent reconnects until a later valid `retry:` field changes it (clamp the override into [1 s, 60 s] too); make the sleep cancelable via `tokio::select!` on the cancel token (mqtt consumer.rs:300-308 pattern). Expose the scheduling as a pure helper `pub(crate) fn next_delay(attempt: u32, retry_override_ms: Option<u64>, cfg: &SseUriConfig) -> Duration` (override wins over the policy delay, clamped to [1000, 60000] ms) — the loop calls it, and tests assert it directly without clock injection.
3. Budget reset: `attempt = 0` exactly when the FIRST complete event or comment frame is parsed on a stream (not on HTTP 200, not on partial bytes).
4. On every reconnect: `connect_once` re-runs in full — SSRF validation, fresh DNS resolution with pinning, pinned client reuse, `Last-Event-ID` header when a stored id exists (memory only, never persisted) — and always targets the CONFIGURED URL (never a previous redirect target; `send_with_ssrf_safe_redirects` re-follows redirects only within that connect).
5. 5xx status handling: `connect_once` returns the response; status 5xx → drop the response body, count as retriable, continue the loop (do NOT consume the body).

**Tests**
- `sse_consumer_5xx_retried_then_succeeds` — setup: `Responder::route` returning 503 on the first request, then a staged `text/event-stream` with one event; action: start consumer with `retryInitialMs=1000` (clamped floor); assert: first `recorded()` request got 503, second got the stream, one exchange arrives, no task error, and the second request carries NO `Last-Event-ID` header (no id received before the reconnect — spec scenario "No id, no header"). command: `cargo test -p camel-component-http sse_reconnect`; expected: FAIL before impl, PASS after.
- `sse_consumer_204_terminal_no_reconnect` — setup: route responder always 204; action: start; assert: terminal task error, `recorded()` count == 1. Same command; expected: FAIL→PASS.
- `sse_consumer_403_terminal_no_reconnect` — same shape with 403; assert: terminal error, count == 1. Same command; expected: FAIL→PASS.
- `sse_reconnect_last_event_id_resent_from_memory` — setup: staged stream sends `id: 42\ndata: a\n\n` then channel drop (transport break); route responder then serves a second staged stream sending `id: 43\ndata: b\n\n` followed by an event with an id longer than 1 KiB (`data: c\n\n`), then drop; third staged stream records headers; action: run consumer, await 3 exchanges; assert: 2nd request carries `Last-Event-ID: 42`, 3rd carries `Last-Event-ID: 43` — the oversized id is NOT stored or resent (it was delivered as an exchange). Same command; expected: FAIL→PASS.
- `sse_reconnect_idle_timeout_fires_on_silence` — setup: endpoint with `idleTimeoutMs=500`; first staged stream sends headers + one comment then goes silent; route responder then serves a staged stream with one event; action: start consumer; assert: a second request appears (`recorded()` count == 2) after the idle bound and the event arrives (silence is retriable, not terminal). Same command; expected: FAIL→PASS.
- `sse_reconnect_connect_timeout_bounded` — setup: a `tokio::net::TcpListener` bound to `127.0.0.1:0` that accepts connections and never writes bytes (not an HTTP responder — no raw HTTP bytes are written, so lint-no-raw-http-test-server does not apply); endpoint with `connectTimeoutMs=500`; action: start consumer with a 2-attempt budget; assert: task error mentioning budget exhaustion, and the listener observed 2 accepted connections (the connect-timeout failure was retried, not terminal). Same command; expected: FAIL→PASS.
- `sse_reconnect_budget_reset_on_first_complete_event` — OBSERVABLE CONTRACT (no spy/double), covering BOTH reset branches (comment AND event): setup: `retryInitialMs=1000`, `retryMaxMs=2000` so backoff escalation is visible in timing; route responder: first request → 503 (attempt 1, escalated delay scheduled); second request → staged stream that emits ONE comment `:k\n\n` (variant A) or ONE complete event `data: x\n\n` (variant B) then breaks; third request → 503 forever with a 2-attempt budget; action: run the consumer per variant; assert: the reconnect AFTER the parsed comment (variant A) or parsed event (variant B) happens ~1 s after the break (initial delay again, bounded window 0.9-1.5 s) — an unreset counter would schedule the escalated 2 s delay. Run both variants (two `#[tokio::test]`s or a parameterized pair). Same command; expected: FAIL→PASS.
- `sse_reconnect_budget_exhaustion_is_task_error` — setup: route responder always 503, policy `max_attempts=2`; action: start; assert: task error mentioning budget exhausted after the 2nd attempt, `recorded()` count == 3 (initial + 2 retries). Same command; expected: FAIL→PASS.
- `sse_reconnect_server_retry_field_overrides_delay` — unit test on the pinned pure helper (no clock injection): setup: `SseUriConfig` with defaults; action + assert: `next_delay(0, None, cfg)` == 3 s (default), `next_delay(0, Some(5000), cfg)` == 5 s (override), `next_delay(0, Some(50), cfg)` == 1 s (clamp floor — spec "Retry delay clamped" below-1 s branch), `next_delay(0, Some(70000), cfg)` == 60 s (clamp ceiling), `next_delay(2, None, cfg)` returns the escalated policy delay; every value within [1 s, 60 s]. Same command; expected: FAIL→PASS.
- `sse_reconnect_cancel_mid_sleep_stops_promptly` — setup: route responder always 503 with `retryInitialMs=60000`; action: start, wait for the first retry to be scheduled, fire cancel token; assert: task returns within 1 s without a second connect (recorded() count == 1). Same command; expected: FAIL→PASS.
- `sse_reconnect_target_is_configured_url_not_redirect` — setup: route responder first answers 302 with `Location` pointing at the same server's alternate path, second answers the event stream then breaks, then always 404 (which on reconnect would differ from the redirect path); action: run consumer until the first stream breaks; assert: the reconnect request path is the CONFIGURED path (the one from the URI), not the redirect target path. Same command; expected: FAIL→PASS.
- `sse_reconnect_ssrf_revalidation_runs_per_connect` — OBSERVABLE CONTRACT (ssrf.rs resolves via `lookup_host` inline with no test double; no resolver trait is invented): setup: loopback endpoint whose first staged stream breaks after one event; route responder then serves a second stream; action: run consumer across the reconnect; assert: exactly 2 requests reach the server and BOTH target the same configured host/path (`recorded()` requests 1 and 2) — proving the reconnect re-executed `connect_once`, whose body re-runs the full SSRF + fresh-DNS-pinning pass by construction; the no-skip control flow (every loop iteration goes through `connect_once`) is additionally verified at review time. Same command; expected: FAIL→PASS.

**Acceptance**
- All `sse_reconnect_*` and `sse_consumer_*` tests pass (`cargo test -p camel-component-http` green on the crate).
- `cargo fmt --check --all`; `cargo clippy -p camel-component-http --all-targets -- -D warnings`; `cargo xtask lint-cancel-tokens`; `cargo xtask lint-unbounded-wait`; `cargo xtask lint-test-sleep` all exit 0.
- No real-time waits longer than ~2 s in tests (lint-test-sleep clean; delays asserted through injected clock or clamped floors).

## Task 1.5 — Route-level tests (`sse:` through a real CamelContext)

- [x] 1.5

**Files**
- `crates/components/camel-http/tests/sse_consumer_route.rs` (new)
- `crates/components/camel-http/Cargo.toml` (modified — only if a dev-dep is genuinely missing; camel-core, camel-builder, camel-processor and test-support are already dev-deps)

**Steps**
1. Create the test file following the camel-rabbitmq route e2e pattern (crates/components/camel-rabbitmq/tests/route_e2e.rs): build a `CamelContext`, register `HttpBundle` (from_toml of an empty table), add a route `from("sse://127.0.0.1:{port}/events?allowInternal=true&retryInitialMs=1000")` whose sink accumulates exchanges into an `Arc<Mutex<Vec<Exchange>>>` via a processor step, start the context, and drive it against an `HttpTestServer`.
2. Route-level scenario coverage (each its own `#[tokio::test]`): one-exchange-per-event end-to-end; 204 terminal fails the route (consumer task error surfaces, context stop works); 5xx retried then events flow; context `stop()` cancels mid-read and mid-sleep promptly (bounded await ≤ 2 s).

**Tests**
- `sse_route_one_exchange_per_event_end_to_end` — setup: server staged stream with 3 events; action: run route, await sink len == 3 (bounded 15 s); assert: 3 exchanges in order, bodies correct, all InOnly. command: `cargo test -p camel-component-http --test sse_consumer_route`; expected: FAIL before impl (route scheme unknown), PASS after Tasks 1.1-1.4.
- `sse_route_204_terminal_fails_route_and_stops_cleanly` — setup: responder always 204; action: start context, await route failure surfacing, then `ctx.stop()`; assert: stop completes bounded, no hang. Same command; expected: FAIL→PASS.
- `sse_route_5xx_retried_events_flow` — setup: 503 then staged stream with 1 event; assert: event reaches the sink within the bound. Same command; expected: FAIL→PASS.
- `sse_route_stop_cancels_mid_read_and_mid_sleep` — setup: (a) staged stream open with no further bytes (mid-read), (b) responder always 503 with 60 s retry delay (mid-sleep); action: start context, then stop after consumer readiness; assert: stop returns bounded ≤ 2 s in both variants. Same command; expected: FAIL→PASS.

**Acceptance**
- `cargo test -p camel-component-http --test sse_consumer_route` passes.
- `bash scripts/gates-lanes-test.sh` exits 0 (camel-http lane declaration intact).
- No infra dependency introduced (tests run with plain `cargo test`, no docker).

## Task 1.6 — ADR-0102 and component docs

- [x] 1.6

**Files**
- `docs/adr/0102-sse-consumer-scheme.md` (new)
- `docs/src/components/http.md` (modified)

**Steps**
1. Write `docs/adr/0102-sse-consumer-scheme.md` following the house ADR format (mirror docs/adr/0098-redis-value-contract.md structure: Status/Context/Decision/Consequences). Record: (a) `sse:` is a consumer-only scheme INSIDE camel-http (rejecting the separate-crate option; reuses `pub(crate)` ssrf/tls_allowlist/client_cache seams), amending the ADR-0060 exclusion note (which concerned the `stream:` stdio data plane, not http-crate schemes), including the scheme-to-transport translation rule (`sse://` rides `https://`; cleartext per ADR-0081 consent); (b) resubscription-not-retry with the budget-reset point (first complete event/comment after reconnect, never HTTP 200) and ADR-0007 supervision on exhaustion; (c) the pinned bounds (1 MiB / 90 s / 10 s / [1 s, 60 s] / 1 KiB id cap, no total-bytes or max-events cap); (d) the recorded UTF-8 deviation: invalid UTF-8 fails the task with the `b-prime:sse:utf8` label instead of the SSE-spec U+FFFD replacement; (e) security re-validation on every connect. Reference ADR-0007, 0013, 0044, 0081.
2. In `docs/src/components/http.md`, add a `## Server-Sent Events consumer (sse:)` section after `## Client` with: scheme syntax `sse://host/path?maxEventBytes=...&idleTimeoutMs=...&connectTimeoutMs=...&retryInitialMs=...&retryMaxMs=...&allowInternal=...&allowCleartext=...&authHeaders=Name=Value,...`; a runnable example route (`from("sse:...")` with a `log`/`to` sink); the header mapping table (`CamelSseEvent`/`CamelSseId`/`CamelSseRetry`); terminal-vs-retried semantics; the MVP exclusions (no POST bodies, no persisted ids, no max-events cap, no total-bytes cap); and the authHeaders percent-encoding note (a value containing a comma must be percent-encoded as %2C).

**Tests**
- `mdbook_sse_section_renders_and_tests` — setup: docs built; action: `cd docs && mdbook test`; assert: exits 0 (no broken links/code blocks in the new section). command: `cd docs && mdbook test`; expected: PASS before (section absent) and after (must remain green with the section).

**Acceptance**
- `cd docs && mdbook test` exits 0.
- `cargo xtask lint-context-citations` exits 0 (ADR citations lint).
- ADR file numbered 0102 (next free number verified against `ls docs/adr/`).
- English prose only (LANGUAGE POLICY).

## Task 1.7 — Schema regeneration, exclusions surface check, crate-level sweep

- [x] 1.7

**Files**
- `schemas/component-metadata.json` (modified — regenerated)
- `schemas/dsl/route-schema.json`, `schemas/canonical-route-spec.json` (modified only if `cargo xtask schema` reports drift)

**Steps**
1. Run `cargo xtask schema` in the worktree; inspect `git diff schemas/` — the `sse` scheme must appear in `schemas/component-metadata.json` with exactly the uri_options from Task 1.2 and consumer-only capabilities; commit the regenerated artifacts.
2. Surface check for the MVP exclusions (spec requirement "MVP exclusions"): assert in a test appended to `sse_tests.rs` that `SseEndpoint` metadata contains NO option named `maxEvents`/`totalBytes` and no body/request option, and that `create_producer` still errs — this makes the exclusions scenario machine-checkable.
3. Run the crate-level quality battery and fix anything it flags (fmt, clippy, unwrap, metric labels).

**Tests**
- `sse_exclusions_surface_check` — setup: endpoint metadata; action: enumerate metadata options; assert: no `maxEvents`, no `totalBytes`, no request-body option; `create_producer` errs. command: `cargo test -p camel-component-http sse_exclusions`; expected: FAIL before the test exists, PASS after.
**Tests**
- `schema_contains_sse_scheme` — UNACHIEVABLE AS PINNED (recorded deviation): `schemas/component-metadata.json` is the JSON-Schema of the `ComponentMetadata` struct (`schemars::schema_for!`), not a per-scheme registry — it cannot list schemes without hand-editing an artifact that `--check` would then reject. SUBSTITUTED: `sse_metadata_surface_pinned` (runtime `Component::metadata()` — scheme, consumer-only capabilities, exactly the 8 options in order with kind/default/secret) + `sse_exclusions_surface_check` + `cargo xtask schema --check` exit 0 (no drift). setup: regenerated schema (no-op) + runtime metadata; action: run the two tests + check; assert: both pass, check exits 0. command: `cargo test -p camel-component-http sse_metadata sse_exclusions && cargo xtask schema --check`; expected: PASS.

**Acceptance**
- `cargo xtask schema --check` exits 0 (regenerated and committed).
- `cargo fmt --check --all`; `cargo clippy -p camel-component-http --all-targets -- -D warnings`; `cargo test -p camel-component-http`; `cargo xtask lint-unwrap` all exit 0.
- `git status` clean after commit.
