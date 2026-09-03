# Tasks: ws-client-consumer

## camel-ws config

### Task 1.1: Add `consumeAsClient` URI option

**Files:**
- `crates/components/camel-ws/src/config.rs` (modified)

**Steps:**
1. Add field `consume_as_client: bool` to `WsUriConfig` with `#[uri_param(name = "consumeAsClient", default = "false", desc = "Consumer dials out as WebSocket client and creates one exchange per pushed frame")]`.
2. Add field `pub consume_as_client: bool` to `WsEndpointConfig` (default `false` in `Default` impl).
3. In `WsEndpointConfig::from_uri`, after parsing `send_to_all`, bridge the parsed `consume_as_client` into the endpoint config.
4. Confirm `WsUriConfig::metadata()`/`uri_options()` pick the option up automatically (macro-derived; no manual registry edit).

**Tests:** (in `config_validation_tests` / nearby test module)
- `consume_as_client_parses_true`: `WsEndpointConfig::from_uri("ws://localhost:8080/echo?consumeAsClient=true")` → `cfg.consume_as_client == true`.
- `consume_as_client_parses_false`: same URI with `consumeAsClient=false` → `cfg.consume_as_client == false`.
- `consume_as_client_defaults_false`: `WsEndpointConfig::from_uri("ws://localhost:8080/echo")` → `cfg.consume_as_client == false`.
- `consume_as_client_invalid_rejected`: `from_uri("ws://localhost:8080/echo?consumeAsClient=yes")` returns `Err` (bool parse error surfaced, not coerced).
- `consume_as_client_in_metadata`: `WsEndpointConfig::uri_options()` contains an option with name `consumeAsClient`; `metadata()` lists it.
- `command`: `cargo test -p camel-component-ws --lib config` — all pass after implementation; the true/false/default/invalid tests fail before step 3.
- `expected`: new tests fail pre-implementation, pass post.

**Acceptance:**
- `cargo test -p camel-component-ws --lib config` exits 0.
- `cargo clippy -p camel-component-ws -- -D warnings` exits 0.
- `consume_as_client_in_metadata` asserts both `uri_options()` and `WsEndpointConfig::metadata()` list `consumeAsClient` (metadata IS the lint catalog source).

- [x] 1.1

## camel-ws client consumer

### Task 1.2: Cancellation-aware connect helper

**Files:**
- `crates/components/camel-ws/src/client_consumer.rs` (new)
- `crates/components/camel-ws/src/lib.rs` (modified: add `mod client_consumer;` ONLY — no `pub use` re-export; all later use sites are crate-internal via `crate::client_consumer::…`)

**Steps:**
1. Create `client_consumer.rs` with `pub(crate) async fn connect_ws_client_cancelable<R>(request: R, url: &str, connect_timeout: std::time::Duration, policy: &NetworkRetryPolicy, cancel: &CancellationToken, metrics: Option<&dyn MetricsCollector>) -> Result<tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>, CamelError> where R: IntoClientRequest + Unpin + Clone`.
2. Body: biased `tokio::select!` — `biased;` arm 1: `_ = cancel.cancelled() => Err(CamelError::ProcessorError("WebSocket client connect cancelled".into()))`, arm 2: the existing `retry_async_cancelable(policy, "ws", "connect", op, is_retryable_ws_error, cancel, metrics)` where `op` clones the request and runs `tokio::time::timeout(connect_timeout, tokio_tungstenite::connect_async(r))` exactly as `connect_ws_with_retry` does today (lib.rs ~1700-1740 — mirror its error mapping).
3. The helper owns connect metrics exclusively via `metrics` (attempts + exhaustion error); no `record_component_operation` here.
4. Do NOT touch the existing `connect_ws_with_retry` (producer keeps it).

**Tests:** (in `client_consumer.rs` `#[cfg(test)] mod tests`)
- `connect_cancelable_returns_clean_on_cancel`: policy with `initial_delay = 5s`, unreachable URL, cancel token cancelled immediately → helper returns `Err` containing "cancelled" within ~100ms (not after the delay). `command`: `cargo test -p camel-component-ws --lib connect_cancelable`.
- `connect_cancelable_exhausts_to_err`: policy `max_attempts = 2`, `initial_delay = 10ms`, unreachable port (bind nothing) → helper returns `Err` after 2 attempts.
- `connect_cancelable_connects_when_reachable`: spawn a `tokio_tungstenite::accept_async` listener on an ephemeral port, build request from `ws://127.0.0.1:{port}/` → helper returns `Ok(stream)`.
- `expected`: fail before implementation (module absent), pass after.

**Acceptance:**
- `cargo test -p camel-component-ws --lib connect_cancelable` exits 0.
- `cargo clippy -p camel-component-ws -- -D warnings` exits 0.

- [x] 1.2

### Task 1.3: `WsClientConsumer` lifecycle

**Files:**
- `crates/components/camel-ws/src/client_consumer.rs` (modified)

**Steps:**
1. Define `#[derive(Clone, Debug, PartialEq, Eq)] pub enum ClientConnState { Connecting, Connected, Reconnecting, Exhausted }`.
2. Define `pub struct WsClientConsumer` with fields: `cfg: WsClientConfig`, `runtime: Arc<dyn camel_component_api::RuntimeObservability>`, `conn_state_tx: watch::Sender<ClientConnState>`, `cancel: Option<CancellationToken>`, `task: Option<JoinHandle<Result<(), CamelError>>>`, `state` enum `Created | Running | Stopped` (private), `security_ctx: Option<SecurityContext>`.
3. `pub fn new(cfg: WsClientConfig, runtime: Arc<dyn RuntimeObservability>, conn_state_tx: watch::Sender<ClientConnState>) -> Self` — state `Created`, `conn_state_tx` seeded `Connecting`.
4. `impl Consumer for WsClientConsumer`:
   - `async fn start(&mut self, ctx: ConsumerContext)`: on `Running` return `Err(CamelError::EndpointCreationFailed("WebSocket client consumer already started".into()))`. Set `conn_state_tx` to `Connecting`. Build the client request from `format!("{}://{}:{}{}", scheme, host, port, path)` via `IntoClientRequest`, adding `Sec-WebSocket-Protocol` header when `cfg.inner.subprotocols` is non-empty (mirror producer code lib.rs ~1494-1503). Store `ctx.cancel_token()`. Call `connect_ws_client_cancelable(request, &url, cfg.inner.connect_timeout, &cfg.inner.reconnect_policy, &cancel, Some(self.runtime.metrics().as_ref()))`; on `Err`: FIRST branch on `cancel.is_cancelled()` — if cancelled, return that `Err` WITHOUT publishing `Exhausted` (shutdown is not exhaustion); otherwise set state back to `Created`, publish `Exhausted`, return the `Err` (fail-loud start). On `Ok(stream)`: `ctx.mark_ready()`, publish `Connected`, spawn the receive task (Task 1.4 fills the loop body; for now the task closes the stream and returns `Ok(())`), store the `JoinHandle`, state `Running`.
   - `async fn stop(&mut self)`: idempotent — if `Stopped`, return `Ok(())`. Cancel stored token if any, take-and-await `self.task` only when still locally owned, set `Stopped`, return `Ok(())`.
   - `fn concurrency_model(&self) -> ConcurrencyModel` → `ConcurrencyModel::Sequential`.
   - `fn startup_mode(&self) -> ConsumerStartupMode` → `ConsumerStartupMode::Explicit`.
   - `fn background_task_handle(&mut self)` → `self.task.take()` (at-most-once transfer to Runtime).
   - `fn set_security_context(&mut self, ctx: SecurityContext)` → store.
5. Temporary loop placeholder is FORBIDDEN — the spawned task in this task performs: best-effort `stream.close(None)`, then `Ok(())`. Task 1.4 replaces it with the real loop in the same edit series; both land before this task's checkbox is checked.

**Tests:** (test module; helper `async fn spawn_push_ws_server(bind: &str) -> (SocketAddr, JoinHandle<()>)` wrapping `TcpListener` + `accept_async`, replying nothing, accepting N connections. Lifecycle tests use the NON-panicking `NoopRuntimeObservability` from camel-component-api `test_support` (camel-ws test-support dev-dep) — NOT `test_rt()`/`PanicRuntimeObservability`, whose `metrics()` panics by design and would fail every test below because `start()` calls `runtime.metrics()`)
- `client_consumer_start_marks_ready_and_connects`: reachable server; `WsClientConsumer::new(cfg with small timeouts, Arc::new(NoopRuntimeObservability), watch)`; build the context API-exactly: `let (signal, receiver) = StartupSignal::pair(); let ctx = ConsumerContext::new(channel, token, "r1".into()).with_startup(signal);` then `start(ctx)` → `Ok`, watch observed `Connecting` then `Connected`, and `tokio::time::timeout(1s, receiver.await_ready())` resolves `Ok`.
- `client_consumer_unreachable_fails_start`: policy `max_attempts=2, initial_delay=10ms`, free port → `start` returns `Err`, watch ends `Exhausted`, second `start` after failure still allowed (state `Created`).
- `client_consumer_double_start_rejected`: after successful `start`, a second `start` with a fresh ctx returns `Err` whose message contains "already started" and the first task is unaffected.
- `client_consumer_stop_idempotent`: `stop()` twice → both `Ok`, task handle consumed once.
- `client_consumer_shutdown_while_receiving`: server holds connection open; start; cancel the ctx token → consumer task exits `Ok` within 1s (assert via `background_task_handle` join with `tokio::time::timeout(1s, handle)`).
- `subprotocol_header_sent_on_connect`: server variant asserting the upgrade request headers before completing the handshake; cfg with `subprotocols=["vt1"]` → server observes `Sec-WebSocket-Protocol: vt1` and the handshake completes (start returns `Ok`).
- `expected`: fail before implementation, pass after.

**Acceptance:**
- `cargo test -p camel-component-ws --lib client_consumer` exits 0.
- `cargo clippy -p camel-component-ws -- -D warnings` exits 0.

- [x] 1.3

### Task 1.4: Frame receive loop, mapping, backpressure, limits

**Files:**
- `crates/components/camel-ws/src/client_consumer.rs` (modified)

**Steps:**
1. Replace the temporary spawned task body with the real loop: a `loop` containing a biased `tokio::select!` with two arms — arm 1 awaits `cancel.cancelled()` and on fire performs the best-effort `stream.close(None).await` then `return Ok(())`; arm 2 awaits `stream.next()` and dispatches on the yielded `Option<Result<Message, Error>>` per the frame-handling rules of step 2.
2. Frame handling: `Text(t)` → `Exchange` with String body, header `CamelWsMessageType="text"`; `Binary(b)` → bytes body, header `"binary"`; `Ping`/`Pong` → `continue` (transparent); `None` (EOF) or `Err` or `Some(Ok(Close(_)))` → break to reconnect path (Task 1.5; here `return Ok(())` until 1.5).
3. Oversized guard BEFORE exchange construction: if frame byte length > `cfg.inner.max_message_size as usize` → `tracing::warn!`, `runtime.metrics().increment_errors(route_id, "ws_client_consumer")`, `continue`.
4. Delivery: `sender.send(ExchangeEnvelope { exchange, reply_tx: None })` where `sender = ctx.sender()` captured at spawn; wrap the send in the same `select!` (cancel wins — spec scenario "shutdown while backpressured"): on cancel during send → best-effort close, `return Ok(())`; on `Err` (channel closed) → `return Err(CamelError::ChannelClosed)`.
5. Frame outcome metrics: `runtime.component_metrics().observe("ws", "frame", failed)` — `failed=false` after successful send; on channel-closed send failure emit BOTH `observe("ws", "frame", true)` AND `runtime.metrics().increment_errors(route_id, "ws_client_consumer")` before returning the `Err` (matches design.md dispatch-failure contract).
6. `Exchange` construction follows the server consumer's forward path (reuse its body/header assembly pattern from lib.rs forward task).

**Tests:**
- `frames_become_exchanges`: server pushes 3 text frames → route channel (mpsc) yields 3 envelopes; bodies equal payloads; header `CamelWsMessageType == "text"`; `reply_tx` is `None`.
- `binary_frame_maps_to_bytes`: one binary frame → one envelope with bytes body and header `"binary"`.
- `ping_pong_transparent`: server sends Ping then Pong then Text → exactly 1 envelope (the text one).
- `oversized_frame_dropped_flow_continues`: cfg `max_message_size=1024`; server sends a 2048-byte text frame then a 10-byte frame → 1 envelope (small), zero for oversized, one `increment_errors("r", "ws_client_consumer")` recorded on a counting `MetricsCollector` stub.
- `backpressure_pauses_reads`: channel capacity 1 with no receiver draining; server pushes 2 small frames and both socket writes complete into kernel buffers; after 200ms assert the channel holds EXACTLY 1 envelope and no second envelope arrives (loop is blocked in `send`); then drain the channel and assert the 2nd envelope arrives (reads resumed).
- `shutdown_while_backpressured`: full channel + pending send; cancel token → task exits Ok within 1s.
- `frame_metrics_via_facade`: counting `RuntimeObservability` test double whose `metrics()` returns a collector counting `record_component_operation(component, operation, outcome)` and `increment_errors` calls, whose `component_metrics()` override returns `ComponentMetrics::new(self.metrics(), true)` (the trait default passes lever=false, which SUPPRESSES the component-operation success series — without this override the facade emits nothing observable), and whose `health()` returns a no-op `HealthCheckRegistry` → after 2 delivered frames, 2 calls `("ws","frame","success")`; no raw `record_component_operation` on connect.
- `dispatch_failure_emits_both`: drop the receiving channel (drop all receivers) so `send` fails → task returns `Err(CamelError::ChannelClosed)` and the counting stub recorded exactly one `("ws","frame","failure")` component-operation AND one `increment_errors(route_id, "ws_client_consumer")`.
- `command`: `cargo test -p camel-component-ws --lib client_consumer::`.
- `expected`: fail before loop implementation, pass after.

**Acceptance:**
- `cargo test -p camel-component-ws --lib client_consumer::` exits 0.
- `cargo clippy -p camel-component-ws -- -D warnings` exits 0.
- `cargo check -p camel-component-ws --all-features` exits 0 (otel/all-features compile gate).

- [x] 1.4

### Task 1.5: Reconnect loop and exhaustion

**Files:**
- `crates/components/camel-ws/src/client_consumer.rs` (modified)

**Steps:**
1. On EOF / stream `Err` / `Close` frame (the break path from 1.4): publish `Reconnecting`, attempt `connect_ws_client_cancelable(request.clone(), &url, connect_timeout, &policy, &cancel, Some(metrics))` — one fresh bounded sequence per disconnect. On `Ok` → publish `Connected`, `record nothing` (helper owns metrics), continue the outer loop with the new stream. On `Err` (exhaustion) → publish `Exhausted`, `return Err(CamelError::ProcessorError("WebSocket client consumer reconnect policy exhausted".into()))` so route supervision sees the failure.
2. Cancellation during the reconnect sequence returns the helper's clean cancel error → map to `Ok(())` exit (shutdown, not failure).
3. Keep the `Sec-WebSocket-Protocol` header and URL identical across reconnects (clone the original request).

**Tests:**
- `disconnect_reconnect_resumes_delivery`: helper server accepts 1 connection, sends 1 frame, drops it, then accepts a 2nd connection and sends another frame; policy enabled `max_attempts=5, initial_delay=20ms` → 2 envelopes total, watch transitions include `Reconnecting` then `Connected`.
- `reconnect_exhaustion_returns_err`: server accepts 1 connection then never again (listener dropped); policy `max_attempts=2, initial_delay=10ms` → task join yields `Err` containing "exhausted", watch ends `Exhausted`.
- `shutdown_during_reconnect_backoff`: after a drop with `initial_delay=5s`, cancel token → task exits Ok within 500ms (no 5s wait).
- `exhaustion_error_recorded_once`: collector stub counting `increment_retry_attempt` and `increment_errors` → with `max_attempts=3` unreachable: exactly 3 retry attempts and exactly 1 connect-exhaustion error; zero call-site `record_component_operation` for connect.
- `command`: `cargo test -p camel-component-ws --lib client_consumer::`.
- `expected`: fail before, pass after.

**Acceptance:**
- `cargo test -p camel-component-ws --lib client_consumer::` exits 0.
- `cargo clippy -p camel-component-ws -- -D warnings` exits 0.

- [x] 1.5

### Task 1.6: Endpoint wiring + passive health check

**Files:**
- `crates/components/camel-ws/src/health.rs` (modified)
- `crates/components/camel-ws/src/lib.rs` (modified)
- `crates/components/camel-ws/src/client_consumer.rs` (modified: make `ClientConnState`/watch types available to health.rs)

**Steps:**
1. In `health.rs` add `pub(crate) struct ConnectionStateCheck { name: &'static str, state_rx: watch::Receiver<ClientConnState> }` with `pub(crate) fn new(state_rx: watch::Receiver<ClientConnState>) -> Self` and `#[async_trait] impl AsyncHealthCheck for ConnectionStateCheck`: `name()` returns `"ws-client"`, `check()` reads `state_rx.borrow()` — `Connected` → `CheckResult::healthy`, other states → `CheckResult::unhealthy` with message containing the state name (`"ws-client: {state}"`). No network I/O.
2. In `lib.rs` — `WssComponent::create_endpoint` (lib.rs ~991-1030) is a textual DUPLICATE of `WsComponent::create_endpoint` (~917-956), not a delegation: BOTH bodies must be edited IDENTICALLY. In each: when parsed `cfg.consume_as_client` → create `watch::channel(ClientConnState::Connecting)`, register `Arc::new(ConnectionStateCheck::new(rx))` via the existing `ctx.register_current_route_health_check` call site, store the `watch::Sender` on `WsEndpoint` as new field `conn_state_tx: Option<watch::Sender<ClientConnState>>`; else keep today's `WsHealthCheck::new(host, port)` registration and `None`.
3. `WsEndpoint::create_consumer` branches: `cfg.consume_as_client` → `Box::new(WsClientConsumer::new(self.cfg.client_config(), rt, self.conn_state_tx.clone().expect("client mode always carries a state sender")))` — replace `expect` with a `map_err` to `CamelError::EndpointCreationFailed` when the sender is missing; else existing `WsConsumer::new(self.cfg.server_config(), rt)` unchanged.
4. `create_producer` untouched.

**Tests:**
- `connection_state_check_passive_states`: `ConnectionStateCheck::new(rx)` with sender publishing `Connecting` then `Connected` → `check()` unhealthy (message contains "Connecting") then healthy; assert zero TCP connections by construction (no I/O in the type).
- `wiring_registers_correct_check`: implement one exact `RecordingComponentContext` in the test module: a struct implementing `ComponentContext` with EXPLICIT implementations for every required method of the trait (the trait provides NO default methods) — `register_current_route_health_check(&self, check: Arc<dyn AsyncHealthCheck>)` pushes `check.name().to_string()` into a `Mutex<Vec<String>>`; every other required method gets a minimal no-op/None impl matching its signature (read the trait in `crates/components/camel-component-api/src/component_context.rs` for the exact method set). Server mode: `create_endpoint("ws://localhost:0/echo", &recording_ctx)` using the REAL argument order of the `Component` trait's `create_endpoint` → recorded names contain exactly `["ws"]`. Client mode: same with `?consumeAsClient=true` → recorded names contain exactly `["ws-client"]`. Do NOT assert `conn_state_tx` directly — `create_endpoint` returns `Box<dyn Endpoint>` with no downcast/accessor; the sender wiring is proven behaviorally by `client_mode_wires_client_consumer` below.
- `client_mode_wires_client_consumer`: `create_endpoint` with `consumeAsClient=true` against a reachable `spawn_push_ws_server`, then `create_consumer(rt)` + `start(ctx)` → frames flow to the channel (behavioral proof the branch built a `WsClientConsumer`; the server consumer would have bound a listener instead).
- `absent_option_keeps_server_behavior`: existing server-mode test suite (`cargo test -p camel-component-ws --lib`) passes unchanged — no regression.
- `command`: `cargo test -p camel-component-ws --lib` and `cargo test -p camel-component-ws --test accept_loop_metric_regression` (untouched, must stay green).
- `expected`: new tests fail before wiring, pass after.

**Acceptance:**
- `cargo test -p camel-component-ws --lib` exits 0 (full lib suite).
- `cargo clippy -p camel-component-ws -- -D warnings` exits 0.

- [x] 1.6

## camel-ws TLS (wss)

### Task 1.7: TLS feature, wss client + producer tests

**Files:**
- `Cargo.toml` (workspace root, modified: `tokio-tungstenite = { version = "0.30", features = ["rustls-tls-native-roots"] }`)
- `crates/components/camel-ws/src/client_consumer.rs` (modified)
- `crates/components/camel-ws/tests/fixtures/ws-test-ca.crt` (new)
- `crates/components/camel-ws/tests/fixtures/ws-test-server.crt` (new)
- `crates/components/camel-ws/tests/fixtures/ws-test-server.key` (new)
- `crates/components/camel-ws/tests/wss_tls_test.rs` (new)

**Steps:**
1. Change workspace `tokio-tungstenite` entry to add `features = ["rustls-tls-native-roots"]` (no version bump, no other changes).
2. Generate fixture material once with openssl (committed files, not runtime), with certificate properties rustls requires — CA cert with `basicConstraints=critical,CA:TRUE` and `keyUsage=keyCertSign,cRLSign`; server cert with `subjectAltName=DNS:localhost` and `extendedKeyUsage=serverAuth` (CN alone fails rustls name validation): `openssl req -x509 -newkey rsa:2048 -nodes -keyout ws-test-ca.key -out ws-test-ca.crt -subj "/CN=WS Test CA" -addext "basicConstraints=critical,CA:TRUE" -addext "keyUsage=keyCertSign,cRLSign"`; then a server CSR `/CN=localhost` signed by that CA with `-copy_extensions copy` or `-extfile` carrying `subjectAltName=DNS:localhost` and `extendedKeyUsage=serverAuth`. Commit ONLY `ws-test-ca.crt`, `ws-test-server.crt`, `ws-test-server.key`; DELETE `ws-test-ca.key` after generation (CA private key never committed).
3. Add private field `connector: Option<tokio_tungstenite::Connector>` to `WsClientConsumer`; the existing 3-argument `new` (Task 1.3 signature) sets it to `None`; add `#[cfg(test)] pub(crate) fn with_connector(mut self, connector: tokio_tungstenite::Connector) -> Self` for test injection. Both the initial connect (1.3 step 4) and the reconnect call (1.5 step 1) pass the stored connector through.
4. Extend `connect_ws_client_cancelable` signature with a final param `connector: Option<tokio_tungstenite::Connector>` and use `tokio_tungstenite::connect_async_tls_with_config(r, None, false, connector.clone())` instead of `connect_async`. Production callers pass `None` (native roots via the enabled feature); tests pass `Some(Connector::Rustls(test_client_config))`. Update the 1.2/1.3 internal call sites for the new signature.
5. In `wss_tls_test.rs`: build a TLS test server (`tokio_rustls::TlsAcceptor` from `rustls::ServerConfig` loaded with the fixture cert/key via `rustls-pemfile`, wrapping a `tokio_tungstenite::accept_async` loop) and a client `rustls::ClientConfig` whose root store contains ONLY the fixture CA (load via `rustls-pemfile`, `roots.add(cert)`).

**Tests:**
- `wss_client_consumer_frames_flow` (UNIT test, lives in `client_consumer.rs` test module — it needs `WsClientConsumer::with_connector`, which is `#[cfg(test)]` crate-private): consumer built from `wss://localhost:{port}/feed?consumeAsClient=true` config with `with_connector(Connector::Rustls(test_client_config))`, TLS server pushes 1 text frame → 1 envelope received. `command`: `cargo test -p camel-component-ws --lib wss_client_consumer_frames_flow`.
- `producer_wss_connect` (in `wss_tls_test.rs`): the producer path is untouched and trusts NATIVE roots only, so the test must make the fixture CA visible to it: inside the test, take a process-wide `static TLS_ENV_LOCK: Mutex<()>`, set env `SSL_CERT_FILE=<abs path to ws-test-ca.crt>` (honored by rustls-native-certs on Unix), run the `WsProducer` send against the same TLS echo server, assert the echoed body, then RESTORE the previous env value before releasing the lock. The env mutation is serialized behind the lock so parallel tests in this binary are unaffected. `command`: `cargo test -p camel-component-ws --test wss_tls_test`.
- `expected`: fail before feature+fixtures, pass after.

**Acceptance:**
- `cargo test -p camel-component-ws --lib wss_client_consumer_frames_flow` exits 0.
- `cargo test -p camel-component-ws --test wss_tls_test` exits 0.
- `cargo test -p camel-ws` exits 0 (whole crate incl. prior tasks' suites).
- `cargo build --workspace` exits 0 (feature change compiles tree-wide).
- `cargo check -p camel-component-ws --all-features` exits 0.
- `ws-test-ca.key` absent from the worktree (CA private key not committed).

- [x] 1.7

## docs

### Task 1.8: Documentation alignment

**Files:**
- `crates/components/camel-ws/CONTEXT.md` (modified)
- `crates/components/camel-ws/README.md` (modified)
- `CONTEXT-MAP.md` (modified)

**Steps:**
1. `CONTEXT.md`: add a section for the client-consumer role — `WsClientConsumer`, `consumeAsClient` option semantics, `ClientConnState` lifecycle, applicability matrix summary, TLS posture (native roots; producer side effect), link the spec capability `ws-component`.
2. `README.md`: document `consumeAsClient=true` usage with a `from("ws://host:port/path?consumeAsClient=true")` example, reconnect/backoff behavior, fail-loud startup, wss note.
3. `CONTEXT-MAP.md`: add key term `WsClientConsumer` (one paragraph, crate-local landing rule: term lands in camel-ws `CONTEXT.md`, map gets the cross-cutting entry citing rc-39d6/spec) under Key Terms, and add camel-ws client-consumer mention to the Components context entry if the list enumerates it.
4. English prose only (language policy).

**Tests:**
- `docs_citations_valid`: `cargo xtask lint-context-citations` exits 0 (no stale citations).
- `command`: `cargo xtask lint-context-citations`.

**Acceptance:**
- `cargo xtask lint-context-citations` exits 0.
- `cargo xtask lint-log-levels`, `cargo xtask lint-metric-labels` exit 0 (docs examples with metric names must not trip label lint).

- [x] 1.8
