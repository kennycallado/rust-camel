# Tasks: redis-live-tls-and-ready-race

## Phase 1: PubSub ready race — red to green

### Task 1.1: De-mask the pubsub consumer test and capture red race evidence

**Files:**
- `crates/camel-test/tests/redis_test.rs` (modified)

**Steps:**
1. In `redis_consumer_pubsub_mode` (redis_test.rs:559): delete the `PUBSUB CHANNELS` barrier block — the `barrier_client`/`barrier_conn` construction (~lines 591-600) and the subscribe-readiness poll loop (~lines 602-620) — so the test PUBLISHes immediately after `h.start().await` returns.
2. Replace the fixed channel name with a per-invocation unique channel: `let channel = format!("pubsub-race-{}", uuid::Uuid::new_v4());` and use it in both the route URI (`command=SUBSCRIBE` channels param) and the test-side PUBLISH. If `uuid` is not a dependency of `camel-test`, use `std::process::id()` + `SystemTime::now()` elapsed millis to build the unique name instead of adding a dependency.
3. Keep the existing assertion (message received within the 5s `wait_until`) unchanged.
4. On the UNFIXED tree, run the race battery with failure counting: `fails=0; for i in $(seq 1 20); do cargo test -p camel-test --test redis_test --features integration-tests redis_consumer_pubsub_mode -- --exact --nocapture || fails=$((fails+1)); done; echo "RED BATTERY FAILURES: $fails/20"`. Record the per-run pass/fail detail and the total. At least one failure (timeout waiting for message) is the expected red evidence of the message-loss window. If all 20 pass, add a concurrent CPU load (e.g. run the battery under a parallel `cargo build -p camel-test`) and re-run with the same counting. If BOTH batteries come back 20/20, do NOT proceed silently — STOP and report `red-evidence-failure: race not reproduced` with the full battery logs; the anti-masking contract requires captured pre-fix red evidence before the fix lands.
5. EVIDENCE-ONLY TASK: after recording the battery results, revert the working-tree edit (`git checkout -- crates/camel-test/tests/redis_test.rs`) so the branch stays green; Task 1.2 re-applies the de-mask atomically with the fix (landing the racy test before the fix would make the branch intermittently red).

**Tests:** (executable spec — name, arrange, act, assert)
- `redis_consumer_pubsub_mode` (existing test, de-masked): unique fresh channel + SUBSCRIBE consumer route started via `h.start().await` → test PUBLISHes immediately after start with no barrier, no sleep, no retry → message is received by the mock endpoint within the existing 5s `wait_until`.
- Red evidence (pre-fix battery, not a committed test): 20 sequential invocations of `cargo test -p camel-test --test redis_test --features integration-tests redis_consumer_pubsub_mode -- --exact` on the unfixed tree → record exact pass/fail counts per run in the worker report (expected: ≥1 timeout failure demonstrating the race).

**Acceptance:**
- `git -C <worktree> diff --stat -- crates/camel-test/tests/redis_test.rs` is empty after the revert (the file has no working-tree diff; other worktree state is out of this task's scope).
- Red-evidence battery totals (failures/20) recorded in the worker's report — ≥1 failure required, or an explicit `red-evidence-failure` stop report.
- `rg -n "PUBSUB CHANNELS" crates/camel-test/tests/redis_test.rs` still shows the barrier (revert confirmed) — its removal lands in 1.2.

- [x] 1.1

### Task 1.2: Move pubsub readiness after first subscribe acknowledgement

**Files:**
- `crates/components/camel-redis/src/consumer.rs` (modified)
- `crates/components/camel-redis/src/pubsub.rs` (modified)
- `crates/camel-test/tests/redis_test.rs` (modified — step 5 re-applies the 1.1 de-mask)

**Steps:**
1. In `pubsub.rs`, change `pubsub_session` to take an additional parameter `on_ready: Option<Box<dyn FnOnce() + Send>>`. Immediately after the FIRST successful `connect_and_subscribe(...)` returns `Ok(())` in the session loop (before entering the message-delivery inner loop), fire it via `if !cancel.is_cancelled() { if let Some(f) = on_ready.take() { f(); } } }` — the cancellation guard avoids signalling ready on a session that returned `Ok` early because it was cancelled mid-connect without ever subscribing (pubsub.rs:137-139). `Option::take` makes it fire-once: reconnect re-subscriptions (subsequent loop iterations) must not re-signal.
2. Update every existing `pubsub_session(...)` call site in the `pubsub.rs` tests module to pass `None` (mechanical; behavior of existing tests unchanged).
3. In `consumer.rs` `run_pubsub_consumer` (~line 283): delete the eager `ctx.mark_ready();` call before the `tokio::select!`. Clone the `ConsumerContext` once (`let ready_ctx = ctx.clone();` — the same clone pattern the `deliver` closure already uses) and pass `Some(Box::new(move || ready_ctx.mark_ready()))` as `on_ready` to `pubsub_session`. The `info!("PubSub consumer started")` log may move next to the session entry but must not be deleted.
4. Do NOT touch `run_queue_consumer`'s `ctx.mark_ready()` (~line 389) — BLPOP has no loss window.
5. Re-apply Task 1.1's de-mask atomically with this fix (same file edit, restated): in `redis_consumer_pubsub_mode` (redis_test.rs:559) delete the `PUBSUB CHANNELS` barrier block (`barrier_client`/`barrier_conn` construction ~lines 591-600 plus the subscribe-readiness poll loop ~lines 602-620); replace the fixed channel with a per-invocation unique name (`format!("pubsub-race-{}", uuid::Uuid::new_v4())`, or `std::process::id()` + `SystemTime` elapsed millis if `uuid` is not a `camel-test` dependency) used by both the SUBSCRIBE route and the test-side PUBLISH; keep the existing 5s `wait_until` assertion.

**Tests:** (executable spec — name, arrange, act, assert. All four tests drive termination via a `CancellationToken` cancelled from the `deliver` hook or the ready hook after the expected events, and wrap the session join in `tokio::time::timeout(Duration::from_secs(5), ...)` — never relying on stream exhaustion alone, since connect outcomes reuse-last and would loop forever.)
- `ready_fires_after_first_subscribe_ack` (new, pubsub.rs tests): `FakePubSubIo::new(vec![Ok(())]).with_messages_per_connect(vec![vec![fake_msg("ch", "m1")]])` (mirroring the existing pattern at pubsub.rs:527), one channel `["ch"]` + one pattern `["p*"]`, `on_ready = Some(...)` setting an `Arc<AtomicBool>` and then cancelling the token → run session under timeout → assert the flag is true and the session returned `Ok(())`.
- `ready_stays_pending_until_subscribe_acks_release` (new, pubsub.rs tests): extend `FakePubSubIo` with a `with_blocked_subscribes(events: Arc<std::sync::Mutex<Vec<String>>>, notify: Arc<tokio::sync::Notify>)` builder — in blocked mode the fake is OWNED by the spawned session, so call observation must go through the shared `events` sink: `subscribe(ch)` pushes `format!("ch:{ch}")` to `events` and then awaits `notify`; `psubscribe(pat)` pushes `format!("pat:{pat}")` and awaits `notify`; `connect`/`next_msg` behave normally (connect `Ok`, stream pends). Test: build the fake with the shared `events` + `notify`, start `pubsub_session` via `tokio::spawn` with one channel `["ch"]` + one pattern `["p*"]`; poll `events` until it contains `"ch:ch"`; assert the ready flag (Arc<AtomicBool>) is false; `notify_one()` (releases channel ack); poll `events` until it also contains `"pat:p*"` (the pattern call only happens after the channel ack returns); assert the ready flag is STILL false (pattern ack outstanding); `notify_one()` again (releases pattern ack); await the spawned session under `tokio::time::timeout(5s)` and cancel via token; assert the ready flag is now true. This proves readiness waits for every channel AND pattern acknowledgement, with no deadlock and fully external observability.
- `ready_fires_exactly_once_across_reconnect` (new, pubsub.rs tests): `FakePubSubIo::new(vec![Ok(()), Ok(())]).with_messages_per_connect(vec![vec![fake_msg("ch", "m1")], vec![fake_msg("ch", "m2")]])` (stream ends after batch 1, forcing one reconnect + re-subscribe; second batch's delivery cancels the token), `on_ready` increments an `Arc<AtomicUsize>` → run to completion under timeout → assert the counter is exactly 1 (fired on first connect only, not on the re-subscribe).
- `reconnect_budget_exhaustion_is_transient` (new, pubsub.rs tests, unconditional — even if it overlaps an existing classification test, the pubsub-path-specific assertion is worth its own test): `FakeTopology` returning transient-failing outcomes until the retry policy budget is exhausted, a small `NetworkRetryPolicy { enabled: true, max_attempts: 2, initial_delay: Duration::from_millis(1), multiplier: 1.0, max_delay: Duration::from_millis(1), jitter_factor: 0.0, max_attempts_absolute: None }` → `pubsub_session` returns `Err` → assert `is_transient_redis_error(&err)` is true (keeps startup fail-fast semantics per ADR-0007).

**Acceptance:**
- `cargo test -p camel-component-redis --lib` passes including the four new tests.
- `rg -n "mark_ready" crates/components/camel-redis/src/consumer.rs` shows exactly two hits total: one inside the `run_pubsub_consumer` `on_ready` closure, one untouched in `run_queue_consumer`; none before the `tokio::select!`.
- `rg -n "PUBSUB CHANNELS" crates/camel-test/tests/redis_test.rs` returns zero matches (de-mask re-applied with the fix).
- `cargo test -p camel-test --test redis_test --features integration-tests redis_consumer_pubsub_mode -- --exact` passes with the fix in place.
- `cargo fmt --check --all` and `cargo clippy -p camel-component-redis --all-targets -- -D warnings` exit 0.

- [x] 1.2

### Task 1.3: Prove the fix live — stress battery, fail-fast, producer receipt

**Files:**
- `crates/camel-test/tests/redis_test.rs` (modified)

**Steps:**
1. Stress battery (green evidence): run with failure counting so the shell command itself fails on any red trial: `fails=0; for i in $(seq 1 20); do cargo test -p camel-test --test redis_test --features integration-tests redis_consumer_pubsub_mode -- --exact || fails=$((fails+1)); done; echo "GREEN BATTERY FAILURES: $fails/20"; test $fails -eq 0`. All 20 must pass (final `test` exits 0). Record the count in the report.
2. Add `pubsub_startup_fails_fast_on_unreachable_broker` (new test in redis_test.rs): build `CamelTestContext` with `RedisComponent::with_config(...)` where the `RedisConfig` carries `reconnect: NetworkRetryPolicy { enabled: true, max_attempts: 2, initial_delay: Duration::from_millis(100), multiplier: 1.0, max_delay: Duration::from_millis(100), jitter_factor: 0.0, max_attempts_absolute: None }` (every field set explicitly; total retry time ≤ ~300ms — set the `reconnect` pub field on the `RedisConfig` value if no builder exists). Add a SUBSCRIBE **consumer route**: `RouteBuilder::from("redis://127.0.0.1:1?command=SUBSCRIBE&channels=dead").to("mock:never")` (SUBSCRIBE is consumer-only; a producer-side SUBSCRIBE would be rejected for the wrong reason). Wrap `h.start().await` in `futures::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(h.start()))` inside `tokio::time::timeout(Duration::from_secs(30), ...)` → assert `Err` + `is_panic()` (the panic is the harness `expect` at harness.rs:271-273) observed within 30s — no hang. The panic path works because `await_ready`'s sender drops when the consumer task dies on retry exhaustion (camel-component-api/src/consumer.rs:158-162), so start() resolves Err — well within budget — instead of hanging.
3. In `redis_pubsub_producer` (redis_test.rs:261), add the subscriber-side assertion: BEFORE `h.start().await`, open a raw subscriber on the shared plaintext fixture with every step deadline-bounded — `tokio::time::timeout(5s, client.get_async_pubsub())`, then `tokio::time::timeout(5s, pubsub.subscribe("mychannel"))` — and spawn a task that pushes the first `on_message()` payload into a `tokio::sync::oneshot`; after the route delivers to `mock:result`, await the subscriber channel with a 5s `tokio::time::timeout` → assert the received payload equals `hello world` (the route publishes it via `.set_body("hello world")` + the `CamelRedis.Channel` header — PUBLISH consumes the exchange BODY, not `CamelRedis.Value`; implementation-note deviation from an earlier draft of this step that used the header recipe).

**Tests:** (executable spec — name, arrange, act, assert)
- `redis_consumer_pubsub_mode` (from 1.1): 20/20 consecutive green invocations post-fix, command exactly as in the spec scenario.
- `pubsub_startup_fails_fast_on_unreachable_broker` (new): short-retry RedisConfig + SUBSCRIBE consumer route to 127.0.0.1:1 → `h.start()` under 30s outer timeout → panics (catch_unwind on the harness `expect`) without hanging.
- `redis_pubsub_producer` (extended): route publishes `hello world` on `mychannel` → a real raw `get_async_pubsub` subscriber on `mychannel` receives the exact payload within 5s.

**Acceptance:**
- 20/20 battery green recorded in the worker report (exact command + counts).
- `cargo test -p camel-test --test redis_test --features integration-tests` passes fully (all tests in the file).
- No `std::thread::sleep`/`tokio::time::sleep` added to any test body in this task (`cargo xtask lint-test-sleep` clean on the file; waits use existing `wait_until` or bounded `tokio::time::timeout` only).
- `cargo fmt --check --all` and `cargo clippy -p camel-test --all-targets -- -D warnings` exit 0.

- [x] 1.3

## Phase 2: Standalone TLS CA wiring

### Task 2.1: Propagate tls_ca_cert onto the endpoint config

**Files:**
- `crates/components/camel-redis/src/config.rs` (modified)
- `crates/components/camel-redis/src/executor.rs` (modified — struct literal at ~line 707)
- `crates/components/camel-redis/src/topology.rs` (modified — test struct literal at ~line 620)
- `crates/components/camel-redis/src/consumer_tests.rs` (modified — struct literal at ~line 12)
- `crates/components/camel-redis/tests/config_roundtrip.rs` (modified — struct literal at ~line 12)
- `crates/services/camel-redis-repo/src/executor.rs` (modified — struct literal at ~line 596)
- `crates/services/camel-redis-repo/src/connection.rs` (modified — struct literal at ~line 97)

**Steps:**
1. Add field `pub tls_ca_cert: Option<String>` to `RedisEndpointConfig` (after `ssl`), doc-commented: "File path to a CA certificate (PEM) for TLS verification. Filled by `apply_defaults()` from global config; not a URI parameter."
2. In the manual `Debug for RedisEndpointConfig` impl, render it via the existing redaction helper pattern used by `RedisConfig`'s manual Debug (config.rs:285 `redacted_opt`) — path is metadata but keep symmetric redaction with the global config's rendering.
3. In `apply_defaults` (config.rs:807): when the endpoint's `tls_ca_cert` is `None`, fill it from `defaults.tls_ca_cert` (same precedence as the other `None`-filled fields like `host`/`port`).
4. In `resolve_defaults` (the post-apply resolution, same place other globals resolve): leave the field untouched — it stays a path until topology construction reads it.
5. `RedisEndpointConfig` has no `Default` impl — it is built via exhaustive struct literals. Update every literal site (the Files list above; verify with `rg -n "RedisEndpointConfig \{" crates/` and cover any site the anchor lines missed) to add `tls_ca_cert: None`, including the two cross-crate `camel-redis-repo` sites.

**Tests:** (executable spec — name, arrange, act, assert)
- `apply_defaults_propagates_tls_ca_cert` (new, config.rs tests): `RedisEndpointConfig` from `from_uri("redis://h:6379")` + `RedisConfig::default().with_tls_ca_cert("/path/ca.pem")` → `apply_defaults(&mut ep, &cfg)` → assert `ep.tls_ca_cert == Some("/path/ca.pem".into())`.
- `apply_defaults_keeps_endpoint_ca_over_global` (new): endpoint with `tls_ca_cert = Some("/ep/ca.pem")` + global `/global/ca.pem` → `apply_defaults` → endpoint value unchanged (`/ep/ca.pem`).
- `endpoint_config_debug_redacts_tls_ca_cert` (new): endpoint with the field set → `format!("{:?}", ep)` does not contain the path string.
- `from_uri_does_not_parse_ca_from_uri` (new): `from_uri("rediss://h:6379")` → `tls_ca_cert` is `None` (URI params must not set it).

**Acceptance:**
- `cargo test -p camel-component-redis --lib` passes including the four new tests.
- `rg -n "tls_ca_cert" crates/components/camel-redis/src/config.rs` shows field + builder/redaction + apply_defaults + tests.
- `cargo fmt --check --all` and `cargo clippy -p camel-component-redis --all-targets -- -D warnings` exit 0.

- [x] 2.1

### Task 2.2: Read the CA and build the TLS client with it

**Files:**
- `crates/components/camel-redis/src/topology.rs` (modified)

**Steps:**
1. `StandaloneTopology`: add private field `ca_pem: Option<Vec<u8>>`, gated `#[cfg(feature = "tls")]` (field and its uses — gating read AND store under the feature avoids feature-less `dead_code` under `-D warnings`). PRESERVE the existing public constructor `pub fn new(config: &RedisEndpointConfig)` unchanged (it is re-exported public API — changing its arity breaks external callers; existing call sites in topology.rs tests and pubsub.rs:495 keep compiling untouched). Add `#[cfg(feature = "tls")] pub(crate) fn new_with_ca(config: &RedisEndpointConfig, ca_pem: Option<Vec<u8>>)` — same body as `new` plus storing the PEM. `new` delegates to `new_with_ca(config, None)` under the tls feature.
2. `topology_from_config` (topology.rs:331): under `#[cfg(feature = "tls")]`, read the CA file ONLY inside the `TopologyKind::Standalone` branch AND only when `config.is_ssl_enabled()` — a configured CA on a plaintext endpoint or a sentinel endpoint is ignored without any filesystem access (out of scope, follow-up rc-hbde6; a step comment says so). Read with `std::fs::read` → on error return the SAME error construction style `validate_tls` uses (Config-class error, message contains the path but never file contents, no transient-classifier words) so `is_transient_redis_error` returns false for it. Standalone branch: tls feature → `StandaloneTopology::new_with_ca(config, maybe_ca)`; feature-less → `StandaloneTopology::new(config)` (validate_tls has already rejected TLS endpoints feature-less, so no CA path is ever needed there).
3. `StandaloneTopology::resolve`: in the TLS case (`ConnectionAddr::TcpTls` present in `self.addr`) AND `self.ca_pem.is_some()` (both under `#[cfg(feature = "tls")]`), branch to `Client::build_with_tls(info, redis::TlsCertificates { client_tls: None, root_cert: self.ca_pem.clone() })` instead of `Client::open(info)` — `TlsCertificates::root_cert` is already `Option<Vec<u8>>` (redis-1.6.0/src/tls.rs:29), so `self.ca_pem.clone()` assigns directly; wrapping it in `Some(...)` again would be a double-Option type error. Map errors identically. The non-CA path stays exactly `Client::open`.
4. Sentinel topology: leave untouched (covered by step 2's gate). Add a code comment at the sentinel branch: CA trust on the sentinel surface is follow-up bd rc-hbde6.

**Tests:** (executable spec — name, arrange, act, assert; all network-free — client construction never connects. The first three are `#[cfg(feature = "tls")]`; `plaintext_endpoint_ignores_configured_ca` runs ungated because it passes under both builds.)
- `ca_configured_topology_stores_ca_and_resolves` (new, topology.rs tests, `#[cfg(feature = "tls")]`): a fixed valid self-signed CA PEM embedded as a byte-string constant in the test (rcgen is NOT a dev-dep of this crate — do not add one) written to a `tempfile` path → `topology_from_config` with a standalone endpoint (`ssl = Some(true)`) + that path → `.resolve(ServerKind::Master).await` returns `Ok(client)` AND the storage assertion holds via a `#[cfg(feature = "tls")] pub(crate) fn ca_pem(&self) -> Option<&[u8]>` accessor added on `StandaloneTopology` in this task. NOTE: `Ok` alone does not prove branch selection (`Client::open` also accepts TcpTls under the feature) — the storage assertion plus the Task 3.2 wrong-CA live rejection together prove the branch.
- `ca_absent_topology_keeps_default_constructor` (new, topology.rs tests, `#[cfg(feature = "tls")]`): same endpoint without `tls_ca_cert` → `resolve` returns `Ok` and the accessor reports `None`.
- `plaintext_endpoint_ignores_configured_ca` (new, ungated): standalone endpoint with `ssl = Some(false)` + `tls_ca_cert = Some("/nonexistent/ca.pem")` → `topology_from_config` returns `Ok` (the CA path is never read for a plaintext endpoint — no filesystem access, no error).
- `unreadable_ca_file_fails_closed` (new, topology.rs tests, `#[cfg(feature = "tls")]`): standalone endpoint with `ssl = Some(true)` + `tls_ca_cert = Some("/nonexistent/ca.pem")` → `topology_from_config` returns `Err` whose message contains `/nonexistent/ca.pem`, `is_transient_redis_error(&e)` is false, and no connection is attempted (error occurs before resolve).
- `tls_feature_absent_still_fails_closed` (existing test `topology_from_config_rejects_tls_without_feature`, topology.rs:482): unchanged and green.

**Acceptance:**
- `cargo test -p camel-component-redis --lib` and `cargo test -p camel-component-redis --lib --features tls` both pass (four new tests: three tls-gated + one ungated; existing feature-absent test unchanged and green).
- `rg -n "pub fn new\(" crates/components/camel-redis/src/topology.rs` shows the StandaloneTopology constructor still takes only `&RedisEndpointConfig` (public arity preserved; `new_with_ca` is `pub(crate)`).
- `cargo fmt --check --all` and `cargo clippy -p camel-component-redis --all-targets -- -D warnings` and the same with `--features tls` exit 0.

- [x] 2.2

### Task 2.3: Document tls_ca_cert trust semantics

**Files:**
- `docs/src/components/redis.md` (modified)
- `crates/components/camel-redis/CONTEXT.md` (modified)

**Steps:**
1. `docs/src/components/redis.md`: extend the TLS section — `tls_ca_cert` is a file path (PEM), read at endpoint creation, trusted as the root for standalone `rediss://` connections; unreadable file = fail-closed Config error naming the path; no insecure bypass; sentinel surface is follow-up bd rc-hbde6.
2. `crates/components/camel-redis/CONTEXT.md`: add a subsection next to the existing `effective_tls`/`validate_tls` entries describing the CA wiring point (`StandaloneTopology` CA branch, `build_with_tls`), the fail-closed read, and the standalone-only scope. Match the file's existing entry style (fn names + file anchors).

**Tests:** (executable spec — name, arrange, act, assert)
- Doc lint gates: `cargo xtask lint-context-citations` exits 0 (CONTEXT.md citations valid) and `cargo xtask schema --check` exits 0 (docs build schema untouched).
- `rg -n "tls_ca_cert" docs/src/components/redis.md crates/components/camel-redis/CONTEXT.md` shows the new sections.

**Acceptance:**
- Both gates exit 0; docs render (no broken anchors introduced — keep heading levels consistent with siblings).

- [x] 2.3

## Phase 3: rediss:// live coverage

### Task 3.1: TLS fixture and feature plumbing in camel-test

**Files:**
- `crates/camel-test/Cargo.toml` (modified)
- `crates/camel-test/tests/support/redis.rs` (modified — new TLS fixture inside the existing module; `support/mod.rs` already registers `redis` and needs NO change)
- `crates/camel-test/tests/redis_tls_test.rs` (new — minimal smoke test this task; full suite in 3.2)

**Steps:**
1. `crates/camel-test/Cargo.toml`: add `"camel-redis-repo/tls"` to the `integration-tests` feature list (next to `"dep:camel-redis-repo"`).
2. In `support/redis.rs` add `pub(crate) async fn shared_redis_tls() -> &'static RedisTlsFixture` (lazy via the same `tokio::sync::OnceCell` idiom `shared_redis` uses). `RedisTlsFixture` struct fields: `host: String`, `port: u16`, `ca_pem: String` (PEM bytes as string), `ca_file: std::path::PathBuf` (CA file path inside a retained tempdir), `_ca_dir: tempfile::TempDir` (the fixture MUST own the `TempDir` for the whole process lifetime — dropping it would delete `ca_file` from disk), and `_container: testcontainers::ContainerAsync<GenericImage>` (the fixture MUST own the started container — a bootstrap-local container drops when initialization returns and Redis stops before any test uses it). Provide `Display`/helper rendering `rediss://{host}:{port}` and a `ca_path_string(&self) -> String` convenience implemented as `self.ca_file.to_str().expect("CA tempdir path is UTF-8").to_string()` (tempdir paths are UTF-8 on the Linux hosts this suite runs on; non-UTF8 is a test-infra bug worth panicking on).
3. Fixture bootstrap: use `rcgen` (already a dev-dep) to generate a CA cert + a server cert signed by it with `SanType::IpAddress(IpAddr::V4(127.0.0.1))`; serialize CA PEM, server cert PEM, server key PEM. Start a `GenericImage::new("redis", "7-alpine")` following the sentinel test's self-provisioning idiom: write the three PEMs into the container (e.g. `with_cmd` sh script heredoc-printing the PEM contents to `/tmp/tls/`, mirroring redis_sentinel_test.rs:105's inline-script pattern), then run `redis-server --tls-port 6379 --port 0 --tls-cert-file /tmp/tls/server.pem --tls-key-file /tmp/tls/server.key --tls-ca-cert-file /tmp/tls/ca.pem --tls-auth-clients no`, publish 6379, `WaitFor::message_on_stdout("Ready to accept connections tls")`. Also write the CA PEM to the process-side tempdir file (for `tls_ca_cert` path config).
4. No `#[ignore]`; the fixture lives inside the existing `support::redis` module (compiled under its existing `integration-tests` gating — support/redis.rs is only reachable from integration test binaries; support/mod.rs needs no change).
5. When starting the container in the OnceCell initializer, MOVE the started `ContainerAsync<GenericImage>` into the fixture being constructed (store it in `_container`) before returning the `&'static` reference.

**Tests:** (executable spec — name, arrange, act, assert)
- `redis_tls_fixture_boot` (new, crates/camel-test/tests/redis_tls_test.rs — minimal smoke for this task; the full suite lands in 3.2): `shared_redis_tls()` → fixture port is > 0, `ca_pem` starts with `-----BEGIN CERTIFICATE-----`, `ca_file` exists and its contents equal `ca_pem`, container stdout reached the TLS-ready message (implicit in OnceCell success).

**Acceptance:**
- `cargo test -p camel-test --test redis_tls_test --features integration-tests redis_tls_fixture_boot` passes (requires Docker; the redis:7-alpine image must be present).
- `cargo tree -p camel-test --features integration-tests -i redis` (or `--edges features`) shows the `tls-rustls-*` features active.
- `cargo fmt --check --all` and `cargo clippy -p camel-test --all-targets -- -D warnings` exit 0.

- [x] 3.1

### Task 3.2: rediss:// round-trip and negative controls through the repo path

**Files:**
- `crates/camel-test/tests/redis_tls_test.rs` (modified — full suite)

**Steps:**
1. `#![cfg(feature = "integration-tests")]` file header; `mod support;` wiring mirroring redis_sentinel_test.rs; `install_crypto_provider()` call (same as redis_test.rs) so rustls has a process crypto provider.
2. `rediss_round_trip_through_cache_repository`: fixture → `RedisEndpointConfig::from_uri(&format!("rediss://{host}:{port}"))` → set `endpoint.tls_ca_cert = Some(fixture.ca_path_string())` (the field is `Option<String>`; the helper does the owned UTF-8 conversion with explicit failure handling) → `RedisCacheRepository::connect("tls-live", &endpoint, "tlstest", Duration::from_secs(300)).await` → wrap the whole round-trip in `tokio::time::timeout(10s, ...)`: `set("k1", value)` → `get("k1")` returns the inserted value (value equality, not just existence — proves the full command path carries payloads over TLS) → `invalidate("k1")` → `get("k1")` returns absent. Use the cache repo's actual method names (`set`/`get`/`invalidate` or their exact equivalents — read crates/services/camel-redis-repo/src/cache_repo.rs and use the real names; the contract is value round-trip + removal).
3. `plaintext_client_against_tls_port_fails`: raw `redis::Client::open(format!("redis://{host}:{port}"))` (plaintext scheme against the TLS-only port) → any command (`get_multiplexed_async_connection` or a PING) errors within a bounded `tokio::time::timeout(5s)` with an IO/protocol error, not success.
4. `rediss_client_against_plaintext_port_fails`: raw `redis::Client::open(format!("rediss://{shared_redis plaintext host:port}"))` → connect attempt fails within a bounded 5s timeout (the TLS handshake against a plaintext listener fails regardless of root store).
5. `wrong_ca_is_rejected`: fixture endpoint but `tls_ca_cert` = a SECOND rcgen CA generated in-test (write to temp file) → `RedisCacheRepository::connect(...)` errors (TLS verification failure) within a bounded timeout — proving the CA actually verifies the server cert.
6. Negative-control boundedness: every failure assertion uses `tokio::time::timeout` with a deadline (ADR-0069 s13 precedent); no sleeps.

**Tests:** (executable spec — name, arrange, act, assert)
- `rediss_round_trip_through_cache_repository`: as step 2 → connect Ok, get returns the set value, then absence after invalidate.
- `plaintext_client_against_tls_port_fails`: as step 3 → Err within 5s.
- `rediss_client_against_plaintext_port_fails`: as step 4 → Err within 5s.
- `wrong_ca_is_rejected`: as step 5 → connect Err (certificate verification) within 5s.

**Acceptance:**
- `cargo test -p camel-test --test redis_tls_test --features integration-tests` passes fully (5 tests incl. 3.1 smoke).
- `rg -n "rediss://" crates/camel-test/tests/` shows live-test occurrences (discoverability scenario).
- `cargo fmt --check --all` and `cargo clippy -p camel-test --all-targets -- -D warnings` exit 0; `cargo xtask lint-test-sleep` clean on the new file.

- [x] 3.2
