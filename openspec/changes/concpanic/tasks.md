# Tasks: concpanic

## camel-component-grpc

### Task 1: Fallible consumer_concurrency_limit + startup-entry validation + registry test probe

**Files:**
- `crates/components/camel-component-grpc/src/consumer.rs` (modified)
- `crates/components/camel-component-grpc/src/server.rs` (modified)

**Steps:**
1. In `consumer.rs`, change `fn consumer_concurrency_limit(configured: usize) -> usize` to `pub(crate) fn consumer_concurrency_limit(configured: usize) -> Result<usize, CamelError>`. Body: if `configured > tokio::sync::Semaphore::MAX_PERMITS` return `Err(CamelError::Config(format!("consumerConcurrency {configured} exceeds the supported upper bound {} (tokio::sync::Semaphore::MAX_PERMITS)", tokio::sync::Semaphore::MAX_PERMITS)))`; otherwise `Ok(configured.max(1))`. Update the doc comment: keep the channel==semaphore invariant text and the `.max(1)` rationale; add that values above `Semaphore::MAX_PERMITS` are rejected fail-closed (bd rc-9kgtm, ADR-0033) because the dispatcher semaphore panics above this bound; the channel==semaphore invariant derives both primitives from one validated value.
2. In `start_with_listener` (consumer.rs ~line 457): add `let _concurrency = consumer_concurrency_limit(self.consumer_concurrency)?;` immediately after `self.validate_route_credential_sources()?;` and before the `GrpcServerRegistry::global()` call.
3. In `impl Consumer for GrpcConsumer::start` (consumer.rs ~line 670): add the same `let _concurrency = consumer_concurrency_limit(self.consumer_concurrency)?;` immediately after `self.validate_route_credential_sources()?;` and before the `info!` log and `GrpcServerRegistry::global()` call.
4. In `start_inner` (consumer.rs ~line 488): change `let concurrency = consumer_concurrency_limit(self.consumer_concurrency);` to `let concurrency = consumer_concurrency_limit(self.consumer_concurrency)?;` (defense in depth, unchanged position).
5. In `server.rs`, inside `impl GrpcServerRegistry`, add:
   ```rust
   /// Test-only probe: does a live server entry exist for (host, port)?
   /// Lock poisoning propagates like the other lock sites.
   #[cfg(test)]
   pub(crate) fn contains_server(&self, host: &str, port: u16) -> Result<bool, CamelError> {
       let guard = self.inner.lock().map_err(|_| {
           CamelError::EndpointCreationFailed("GrpcServerRegistry lock poisoned".into())
       })?;
       Ok(guard.contains_key(&(host.to_string(), port)))
   }
   ```
6. In the `consumer.rs` tests module: replace `consumer_concurrency_limit_clamps_zero_and_follows_config` with `consumer_concurrency_limit_normalizes_zero_and_rejects_beyond_limit`, and add the other tests listed below. For tests 7/8 use `StartupSignal::pair()` + `ConsumerContext::new(mpsc::channel(1).0, CancellationToken::new(), "concpanic-test".into()).with_startup(signal)` (imports: `camel_component_api::{ConsumerContext, StartupSignal}` and `tokio_util::sync::CancellationToken`; mirror the C2/component tests for the proto path: `PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/helloworld.proto")`; test runtime — consumer.rs tests have no helper, construct it inline: `let test_rt: Arc<dyn camel_component_api::RuntimeObservability> = Arc::new(camel_component_api::NoOpComponentContext);`). For test 7, obtain an unused port by binding `std::net::TcpListener::bind(("127.0.0.1", 0))`, reading `local_addr().port()`, then dropping the listener (start() must not bind anyway).

**Tests:** (executable spec)
- `consumer_concurrency_limit_normalizes_zero_and_rejects_beyond_limit`: L = `tokio::sync::Semaphore::MAX_PERMITS` → assert `consumer_concurrency_limit(0) == Ok(1)`, `(1) == Ok(1)`, `(7) == Ok(7)`, `(L - 1) == Ok(L - 1)`, `(L) == Ok(L)`; `(L + 1)` is `Err` — destructure/match `CamelError::Config(msg)` and assert `msg` contains `"consumerConcurrency"`, `format!("{}", L + 1)`, and `format!("{L}")`. Command: `cargo test -p camel-component-grpc --lib consumer_concurrency_limit` — expected FAIL before step 1 (fn is infallible), PASS after.
- `tokio_primitives_accept_exactly_the_limit`: assert `tokio::sync::Semaphore::new(L)` constructs and `let (_tx, _rx) = tokio::sync::mpsc::channel::<()>(L);` constructs, no panic (plain `#[test]`, no runtime needed). Command: `cargo test -p camel-component-grpc --lib tokio_primitives_accept` — PASS only if our L equals the primitives' real bound.
- `start_rejects_oversized_concurrency_before_registry_bind_or_readiness` (`#[tokio::test]`): direct `GrpcConsumer::new("127.0.0.1".into(), unused_port, "/helloworld.Greeter/SayHello".into(), proto_path, "helloworld.Greeter".into(), "SayHello".into(), GrpcMode::Unary, test_rt, GrpcServerConfig::default(), L + 1)`; ctx with held receiver. ACT: `consumer.start(ctx).await`. ASSERT: result is `Err`; destructure/match `CamelError::Config(msg)` and assert `msg` contains `"consumerConcurrency"`, `format!("{}", L + 1)`, `format!("{L}")`; `GrpcServerRegistry::global().contains_server("127.0.0.1", unused_port)? == false`; `receiver.await_ready().await` is `Err` (never signalled Ready); rebind check — after the failed start, `std::net::TcpListener::bind(("127.0.0.1", unused_port))` succeeds (proves no listener was bound on the port). Command: `cargo test -p camel-component-grpc --lib start_rejects_oversized` — expected FAIL (panics) before steps 2-3, PASS after.
- `start_with_listener_rejects_oversized_concurrency_before_registry_mutation` (`#[tokio::test]`): bind `tokio::net::TcpListener::bind(("127.0.0.1", 0)).await`, take its port; consumer with same shape, concurrency `L + 1`; ACT: `consumer.start_with_listener(ctx, listener).await`. ASSERT: `Err` destructured to `CamelError::Config(msg)` with the same three substrings; `contains_server("127.0.0.1", port)? == false`; `receiver.await_ready().await` is `Err`. Command: `cargo test -p camel-component-grpc --lib start_with_listener_rejects_oversized` — expected FAIL before step 2, PASS after.

**Acceptance:**
- `cargo test -p camel-component-grpc --lib` passes (all four new/updated tests green).
- `cargo clippy -p camel-component-grpc --all-targets -- -D warnings` exits 0.
- `cargo fmt --check` clean for consumer.rs and server.rs.
- `grep -n "fn consumer_concurrency_limit" crates/components/camel-component-grpc/src/consumer.rs` shows `pub(crate)` and `Result<usize, CamelError>`.

- [x] task-1

### Task 2: URI-parse seam validation + parse boundary tests

**Files:**
- `crates/components/camel-component-grpc/src/config.rs` (modified)

**Steps:**
1. In `parse_grpc_query_params` (~line 610), after the existing `consumer_concurrency` extraction chain, add: `let consumer_concurrency = crate::consumer::consumer_concurrency_limit(consumer_concurrency)?;` (normalizes 0→1 at parse time; rejects oversized with the shared typed error). Adjust the comment above the extraction to note the upper-bound rejection (bd rc-9kgtm).
2. Update the `consumer_concurrency` field's doc comment on `GrpcConfig` (~line 322): add one sentence — values above `tokio::sync::Semaphore::MAX_PERMITS` are rejected at parse and at consumer creation/startup: the dispatcher semaphore panics above this bound; the channel==semaphore invariant derives both primitives from one validated value.
3. Add the three parse tests below to the config.rs tests module, next to the existing rc-ey6v tests (`test_parse_grpc_uri_consumer_concurrency_*`). Build URIs with `format!` and the computed `L`/`L + 1` — never hardcode 64-bit literals.

**Tests:** (executable spec)
- `test_parse_grpc_uri_consumer_concurrency_at_limit_accepted`: L = `tokio::sync::Semaphore::MAX_PERMITS`; uri = `format!("grpc://localhost:50051/pkg.Svc/Method?consumerConcurrency={L}&transport=plaintext")` → `parse_grpc_uri(uri).unwrap()` fifth tuple element `.consumer_concurrency == L`. Command: `cargo test -p camel-component-grpc --lib test_parse_grpc_uri_consumer_concurrency_at_limit` — FAIL before step 1, PASS after.
- `test_parse_grpc_uri_consumer_concurrency_above_limit_rejected`: uri with `{}` = `format!("{}", L + 1)` → `parse_grpc_uri` is `Err`; destructure/match `CamelError::Config(msg)` and assert `msg` contains `"consumerConcurrency"`, `format!("{}", L + 1)`, `format!("{L}")`. Command: `cargo test -p camel-component-grpc --lib test_parse_grpc_uri_consumer_concurrency_above_limit` — FAIL (no error) before, PASS after.
- `test_parse_grpc_uri_consumer_concurrency_zero_normalizes_to_one`: uri with `consumerConcurrency=0` → Ok and `.consumer_concurrency == 1`. Command: `cargo test -p camel-component-grpc --lib test_parse_grpc_uri_consumer_concurrency_zero` — FAIL before (stores 0), PASS after.

**Acceptance:**
- `cargo test -p camel-component-grpc --lib config` passes including the three new tests.
- Existing `test_parse_grpc_uri_consumer_concurrency_default_is_64`, `..._override_honored`, `..._invalid_rejected` still pass unmodified.
- `cargo clippy -p camel-component-grpc --all-targets -- -D warnings` exits 0.
- `cargo fmt --check` clean for config.rs.

- [x] task-2

### Task 3: create_consumer seam + C2-mirror component test + CONTEXT.md

**Files:**
- `crates/components/camel-component-grpc/src/component.rs` (modified)
- `crates/components/camel-component-grpc/CONTEXT.md` (modified)

**Steps:**
1. In `create_consumer` (~line 113), before `GrpcConsumer::new(...)`, add: `let consumer_concurrency = crate::consumer::consumer_concurrency_limit(self.config.consumer_concurrency)?;` and pass `consumer_concurrency` in place of `self.config.consumer_concurrency`.
2. Add `test_create_consumer_rejects_oversized_direct_endpoint` to the component.rs tests module, mirroring the existing C2 test fixture (`test_c2_inbound_tls_without_server_certs_errors`): same `GrpcEndpoint` construction pattern (proto_path from `env!("CARGO_MANIFEST_DIR")`), `transport=plaintext` shape, `consumer_concurrency: L + 1` (L = `tokio::sync::Semaphore::MAX_PERMITS`).
3. Update `CONTEXT.md` line 12 paragraph: extend "Consumer concurrency is configurable via `consumerConcurrency` (default 64 ...; clamped to a minimum of 1)" to also state "rejected fail-closed with a typed configuration error above `tokio::sync::Semaphore::MAX_PERMITS` (`usize::MAX >> 3`): `mpsc::channel` delegates capacity to an internal semaphore and semaphore construction panics beyond that bound (bd rc-9kgtm)". Keep the existing channel==semaphore invariant sentence.

**Tests:** (executable spec)
- `test_create_consumer_rejects_oversized_direct_endpoint`: direct `GrpcEndpoint` with `consumer_concurrency = L + 1` → ACT: `endpoint.create_consumer(test_runtime())`. ASSERT: `Err` destructured/matched as `CamelError::Config(msg)` with `msg` containing `"consumerConcurrency"`, `format!("{}", L + 1)`, `format!("{L}")`; no panic. Command: `cargo test -p camel-component-grpc --lib test_create_consumer_rejects_oversized` — FAIL (panics or Ok) before step 1, PASS after.

**Acceptance:**
- `cargo test -p camel-component-grpc --lib` fully green.
- `cargo test -p camel-component-grpc --all-targets` fully green (lib + integration tests, per proposal promise).
- `cargo clippy -p camel-component-grpc --all-targets -- -D warnings` exits 0.
- `cargo fmt --check` clean for all three touched files.
- `grep -c "usize::MAX >> 3" crates/components/camel-component-grpc/CONTEXT.md` ≥ 1.

- [x] task-3
