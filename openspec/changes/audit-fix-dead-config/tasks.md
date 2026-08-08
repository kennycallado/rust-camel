# Tasks: audit-fix-dead-config

<!-- Single-phase change: 5 tasks across 4 disjoint component crates. No phase grouping needed. -->

## camel-xj

### Task 1.1: Remove transformDirection and resourceUri dead config fields

**Spec coverage:** `No silently ignored config fields` — scenarios `Removed xj transformDirection rejected`, `Removed xj resourceUri rejected`

**Files:**
- `crates/components/camel-xj/src/config.rs` (modified)
- `crates/components/camel-xj/src/lib.rs` (modified — remove transformDirection/resourceUri from crate docs lines 4, 10)
- `crates/components/camel-xj/src/endpoint.rs` (modified — remove transform_direction reference line 14)
- `crates/components/camel-xj/CONTEXT.md` (modified — remove transformDirection/resourceUri mention line 26)
- `crates/components/camel-xj/README.md` (modified)

**Steps:**
1. Delete the `transform_direction: Option<String>` field (line 31) and `resource_uri: Option<String>` field (line 34) from the `XjEndpointConfig` struct.
2. Remove `let mut transform_direction = None;` (line 59) and `let mut resource_uri = None;` (line 60) from `from_uri`.
3. Change the `if key == "transformDirection"` branch (lines 71-74) to return `Err(CamelError::EndpointCreationFailed("transformDirection is not supported; use 'direction' instead".into()))`.
4. Change the `if key == "resourceUri"` branch (lines 76-79) to return `Err(CamelError::EndpointCreationFailed("resourceUri is not supported".into()))`.
5. Remove `transform_direction` and `resource_uri` from the `Ok(Self) struct construction` struct construction (lines 127-128).
6. Delete the `parses_transform_direction_and_resource_uri` test (lines 167-176) and the `parses_direction_without_transform` test (lines 180-186) that assert `transform_direction` is `None`.
7. Remove the `transformDirection` row AND the `resourceUri` row from the README parameter table (lines 46-47).
8. Remove `transformDirection` and `resourceUri` from the crate-level docs in `lib.rs` (lines 4, 10).
9. Remove the `transform_direction` reference from `endpoint.rs` (line 14).
10. Remove the `transformDirection`/`resourceUri` mention from `CONTEXT.md` (line 26).

**Tests:**
- name: `rejects_transform_direction`
  - setup: a URI string `"xj:file:///tmp/a.xslt?direction=xml2json&transformDirection=XML2JSON"`
  - action: call `XjEndpointConfig::from_uri(uri)`
  - assert: result is `Err`, error message contains "transformDirection is not supported"
  - command: `cargo test -p camel-xj rejects_transform_direction`
  - expected: FAIL before implementation (currently parses silently), PASS after

- name: `rejects_resource_uri`
  - setup: a URI string `"xj:file:///tmp/a.xslt?direction=xml2json&resourceUri=classpath:extra.xslt"`
  - action: call `XjEndpointConfig::from_uri(uri)`
  - assert: result is `Err`, error message contains "resourceUri is not supported"
  - command: `cargo test -p camel-xj rejects_resource_uri`
  - expected: FAIL before implementation, PASS after

**Acceptance:**
- `cargo clippy -p camel-xj -- -D warnings` exits 0
- `cargo test -p camel-xj` passes
- No reference to `transform_direction` or `resource_uri` remains in any file under `crates/components/camel-xj/`

- [ ] 1.1

## camel-ws

### Task 2.1: Wire send_timeout into client-mode WebSocket send path

**Spec coverage:** `WebSocket send timeout enforcement (client mode)` — scenarios `Client send completes within timeout`, `Client send exceeds timeout`, `Default send timeout`

**Files:**
- `crates/components/camel-ws/src/lib.rs` (modified)

**Steps:**
1. Add a private async helper function near the send path (after `try_send_with_backpressure`):
   ```rust
   async fn send_with_timeout(
       send_future: impl std::future::Future<Output = Result<(), tungstenite::Error>>,
       timeout: Duration,
   ) -> Result<(), CamelError> {
       match tokio::time::timeout(timeout, send_future).await {
           Ok(result) => result.map_err(|e| CamelError::ProcessorError(format!("WebSocket send failed: {e}"))),
           Err(_) => Err(CamelError::ProcessorError(format!("WebSocket send timeout after {timeout:?}"))),
       }
   }
   ```
   The error type is `tungstenite::Error` — the same type used by the existing `ws_stream.send(out_msg)` call at line 1349 (already imported via `use tokio_tungstenite::tungstenite;` at line 40).
2. Replace the client-mode send call `ws_stream.send(out_msg).await.map_err(|e| CamelError::ProcessorError(format!("WebSocket send failed: {e}")))?` (line 1349) with `send_with_timeout(ws_stream.send(out_msg), cfg.inner.send_timeout).await?`.
3. Do NOT modify the server-send path (`try_send_with_backpressure`) — it is an internal mpsc channel send, not a network send.

**Tests:**
- name: `send_with_timeout_fires_on_elapsed`
  - setup: a pending future typed as `pending::<Result<(), tungstenite::Error>>()`, a 100ms timeout, `#[tokio::test(start_paused = true)]`
  - action: poll `send_with_timeout(pending::<Result<(), tungstenite::Error>>(), Duration::from_millis(100))` once (it is pending), then call `tokio::time::advance(Duration::from_millis(200)).await`
  - assert: result is `Err`, error message contains "timeout"
  - command: `cargo test -p camel-component-ws send_with_timeout_fires_on_elapsed`
  - expected: FAIL before implementation (helper does not exist), PASS after

- name: `send_with_timeout_succeeds_when_fast`
  - setup: an immediately-resolving future `async { Ok::<(), tungstenite::Error>(()) }`, a 30s timeout
  - action: call `send_with_timeout(async { Ok::<(), tungstenite::Error>(()) }, Duration::from_secs(30)).await`
  - assert: result is `Ok(())`
  - command: `cargo test -p camel-component-ws send_with_timeout_succeeds_when_fast`
  - expected: FAIL before implementation, PASS after

- name: `endpoint_config_default_send_timeout` (existing test at line 2815, unchanged)
  - setup: `WsEndpointConfig::default()`
  - action: read `send_timeout` field
  - assert: equals `Duration::from_secs(30)`
  - command: `cargo test -p camel-component-ws endpoint_config_default_send_timeout`
  - expected: PASS before and after (no change to this test)

**Acceptance:**
- `cargo clippy -p camel-component-ws -- -D warnings` exits 0
- `cargo test -p camel-component-ws` passes
- `send_timeout` is referenced in the client send path via `send_with_timeout` (grep confirms)

- [ ] 2.1

## camel-http

### Task 3.1: Remove CookieHandling enum and reject cookieHandling param

**Spec coverage:** `No silently ignored config fields` — scenario `Removed http cookieHandling rejected`

**Files:**
- `crates/components/camel-http/src/lib.rs` (modified)
- `crates/components/camel-http/src/ssrf.rs` (modified)

**Steps:**
1. Delete the `CookieHandling` enum (line 154, variants `Disabled` and `InMemory`).
2. Delete the `cookie_handling: CookieHandling` field from the `HttpEndpointConfig` struct (line 123).
3. Replace the `cookie_handling` URI parsing block (lines 271-279) with explicit rejection: if `parts.params.get("cookieHandling").is_some()`, return `Err(CamelError::InvalidUri("cookieHandling is not supported".into()))`.
4. Keep `"cookieHandling"` in the `HTTP_CAMEL_OPTIONS` set (line 178) so it is not forwarded as a query param to the remote server.
5. Remove the `cookie_handling` parameter from `build_client` signature (line 1585). Remove the `if matches!(cookie_handling, CookieHandling::InMemory)` block (lines 1603-1604).
6. Update all `build_client` call sites (lines 1739, 1857, 2052) to remove the `config.cookie_handling` argument.
7. Remove `endpoint_config.cookie_handling` from `ssrf.rs` (line 377) and any related type signatures.
8. In `test_http_endpoint_config_auth_and_headers_options` (line 2464): remove `&cookieHandling=InMemory` from the test URI. Remove the `the cookie_handling assertion` line (line 2473). Keep all other assertions (auth, userAgent, bridgeEndpoint, connectionClose, skipRequestHeaders, skipResponseHeaders).
9. In `test_http_endpoint_config_basic` (line 2423): remove the `assert!(matches!(config.cookie_handling, CookieHandling::Disabled))` line if present. Keep all other assertions.
10. Add new rejection tests.

**Tests:**
- name: `rejects_cookie_handling_inmemory`
  - setup: URI `"http://localhost/api?cookieHandling=InMemory"`
  - action: call `HttpEndpointConfig::from_uri(uri)`
  - assert: result is `Err`, error message contains "cookieHandling is not supported"
  - command: `cargo test -p camel-component-http rejects_cookie_handling_inmemory`
  - expected: FAIL before (currently parses to InMemory), PASS after

- name: `rejects_cookie_handling_disabled`
  - setup: URI `"http://localhost/api?cookieHandling=Disabled"`
  - action: call `HttpEndpointConfig::from_uri(uri)`
  - assert: result is `Err`, error message contains "cookieHandling is not supported"
  - command: `cargo test -p camel-component-http rejects_cookie_handling_disabled`
  - expected: FAIL before, PASS after

**Acceptance:**
- `cargo clippy -p camel-component-http -- -D warnings` exits 0
- `cargo test -p camel-component-http` passes
- No reference to `CookieHandling` enum remains in the crate

- [ ] 3.1

### Task 3.2: Keep-and-reject proxy_url, remove deprecated setter

**Spec coverage:** `proxy_url validation rejection` — scenarios `proxy_url set to valid URL rejected at validation`, `proxy_url None passes validation`, `proxy_url from TOML rejected at validation`

**Files:**
- `crates/components/camel-http/src/config.rs` (modified)

**Steps:**
1. Keep the `proxy_url: Option<String>` field (line 36) in `HttpConfig` — do NOT remove it.
2. Delete the `#[deprecated]` `with_proxy_url` method (lines 234-240).
3. In `validate()`: replace the existing proxy_url URL-format check (lines 167-171) with a rejection: if `self.proxy_url.is_some()`, return `Err` with message "proxy_url is incompatible with SSRF DNS pinning and cannot be used".
4. Rename `test_rejects_invalid_proxy_url` (line 349) to `test_rejects_proxy_url_with_invalid_url` — now asserts SSRF rejection message.
5. Rename `test_accepts_valid_proxy_url` (line 358) to `test_rejects_proxy_url_with_valid_url` — now asserts SSRF rejection.
6. Keep `test_http_config_defaults` (line 280) unchanged — it already asserts `cfg.proxy_url.is_none()` at line 293 and continues to pass.

**Tests:**
- name: `test_rejects_proxy_url_with_valid_url`
  - setup: `HttpConfig` with `proxy_url: Some("http://proxy:8080".into())`
  - action: call `validate()`
  - assert: `Err`, message contains "incompatible with SSRF DNS pinning"
  - command: `cargo test -p camel-component-http test_rejects_proxy_url_with_valid_url`
  - expected: FAIL before (currently passes URL validation), PASS after

- name: `test_http_config_defaults` (existing test, line 280, unchanged)
  - setup: `HttpConfig::default()`
  - action: read `proxy_url` field
  - assert: `proxy_url` is `None` (already asserted at line 293)
  - command: `cargo test -p camel-component-http test_http_config_defaults`
  - expected: PASS before and after

- name: `test_proxy_url_toml_deserialize_then_reject`
  - setup: a TOML string containing `proxy_url = "http://proxy:8080"` that deserializes into `HttpConfig` via serde
  - action: deserialize with `toml::from_str`, then call `validate()` on the result
  - assert: deserialization succeeds (field is retained for compatibility), `validate()` returns `Err` with "incompatible with SSRF DNS pinning"
  - command: `cargo test -p camel-component-http test_proxy_url_toml_deserialize_then_reject`
  - expected: FAIL before (validate passes for valid URL), PASS after

**Acceptance:**
- `cargo clippy -p camel-component-http -- -D warnings` exits 0
- `cargo test -p camel-component-http` passes
- `with_proxy_url` does not exist (grep confirms)
- `proxy_url` field still exists on `HttpConfig` (grep confirms)

- [ ] 3.2

## camel-direct

### Task 4.1: Remove block and exchange_pattern dead config fields

**Spec coverage:** `No silently ignored config fields` — scenarios `Removed direct block rejected`, `Removed direct exchange_pattern rejected`, `Removed direct exchangePattern (camelCase) rejected`

**Files:**
- `crates/components/camel-direct/src/lib.rs` (modified)

**Steps:**
1. Delete the `pub block: Option<bool>` field (line 74) and `pub exchange_pattern: Option<String>` field (line 78) from the `DirectConfig` struct. Also remove the doc comment above `block` that references DIR-001.
2. Remove the `the block parsing block (parts.params.get("block") map+parse)` parsing block (lines 112-116).
3. Add rejection after `timeout_ms` parsing: `if parts.params.contains_key("block") { return Err(CamelError::InvalidUri("block is not supported".into())); }`.
4. Remove the `the exchange_pattern parsing block (parts.params.get chain)` parsing block (lines 125-128).
5. Add rejection: `if parts.params.contains_key("exchange_pattern") || parts.params.contains_key("exchangePattern") { return Err(CamelError::InvalidUri("exchange_pattern is not supported".into())); }`.
6. Remove `block` and `exchange_pattern` from the `Ok(Self) struct construction` construction (lines 134, 136).
7. Update ALL DirectConfig struct literals to remove `block: None` and `exchange_pattern: None`: lines 941/943, 972/974, 997/999, 1027/1029, 1136/1138.

**Tests:**
- name: `rejects_block_param`
  - setup: URI `"direct:foo?block=true"`
  - action: call `DirectConfig::from_uri(uri)`
  - assert: `Err`, message contains "block is not supported"
  - command: `cargo test -p camel-component-direct rejects_block_param`
  - expected: FAIL before (currently parses to Some(true)), PASS after

- name: `rejects_exchange_pattern_snake_case`
  - setup: URI `"direct:foo?exchange_pattern=InOnly"`
  - action: call `DirectConfig::from_uri(uri)`
  - assert: `Err`, message contains "exchange_pattern is not supported"
  - command: `cargo test -p camel-component-direct rejects_exchange_pattern_snake_case`
  - expected: FAIL before, PASS after

- name: `rejects_exchange_pattern_camel_case`
  - setup: URI `"direct:foo?exchangePattern=InOnly"`
  - action: call `DirectConfig::from_uri(uri)`
  - assert: `Err`, message contains "exchange_pattern is not supported"
  - command: `cargo test -p camel-component-direct rejects_exchange_pattern_camel_case`
  - expected: FAIL before, PASS after

**Acceptance:**
- `cargo clippy -p camel-component-direct -- -D warnings` exits 0
- `cargo test -p camel-component-direct` passes
- No reference to `block` or `exchange_pattern` as config fields remains (grep on struct definition confirms)

- [ ] 4.1
