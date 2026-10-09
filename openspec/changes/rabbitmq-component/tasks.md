# Tasks: rabbitmq-component

## Phase 1: Foundation + producer

### crates/components/camel-rabbitmq (crate skeleton, config, masking)

#### Task 1.1: Crate skeleton with broker config resolution and credential redaction

**Files:**
- `crates/components/camel-rabbitmq/Cargo.toml` (new)
- `crates/components/camel-rabbitmq/src/lib.rs` (new)
- `crates/components/camel-rabbitmq/src/config.rs` (new)
- `crates/components/camel-rabbitmq/README.md` (new — stub: scheme, one broker config example)
- `Cargo.toml` (modified — workspace members + `[workspace.dependencies]` entries)
- `scripts/xtask/trustpub-registrations.toml` (modified — publish-order entry)

**Steps:**
1. Add `crates/components/camel-rabbitmq` to workspace members in root `Cargo.toml`. Package name `camel-component-rabbitmq`, `[lints] workspace = true`, no lapin dependency yet (task 1.3 adds it). Dependencies: `camel-component-api`, `camel-api`, `tokio`, `async-trait`, `tracing`, `bytes`, `serde`, `serde_json`, `toml`, `thiserror` — all `workspace = true` (match camel-jms versions). Add `camel-component-rabbitmq = { path = "crates/components/camel-rabbitmq", version = "=0.56.0" }` to `[workspace.dependencies]`.
2. Add a `[[crates]] name = "camel-component-rabbitmq" state = "new-unpublished"` entry to `scripts/xtask/trustpub-registrations.toml` at the same relative publish-order position the camel-component-jms entry holds. Per the publish-registration spec, `new-unpublished` is Case B: `cargo xtask lint-publish-registration` exits non-zero reporting exactly this crate until the owner performs the manual classic-token first publish and registers trustpub.
3. In `src/config.rs` define `pub struct RabbitBrokerConfig { pub url: String, pub username: Option<String>, pub password: Option<SecretString>, pub vhost: Option<String> }` with serde `Deserialize` + `#[serde(deny_unknown_fields)]` keyed by `[components.rabbitmq.brokers.<name>]`. Master amendment supersedes the original TLS field/type: unsupported `tls` settings must fail closed and list the supported broker params; real overrides are tracked in `rc-l8ohw`. Implement `SecretString` with a manual `Debug` printing `<redacted>`.
4. Define `pub struct RabbitComponentConfig { pub brokers: HashMap<String, RabbitBrokerConfig>, pub reconnect: Option<NetworkRetryPolicy> }` with `Deserialize` + `#[serde(deny_unknown_fields)]` and a `rabbitmq_reconnect_default() -> NetworkRetryPolicy` mirroring `camel-jms/src/config.rs::jms_reconnect_default` (max_attempts 0, initial_delay 5 s, multiplier 2.0, max_delay 30 s, jitter 0.0, enabled true).
5. In `src/config.rs` implement `pub fn resolve_broker_name(brokers: &HashMap<String, RabbitBrokerConfig>, requested: Option<&str>) -> Result<String, CamelError>` with the jms rules from `camel-jms/src/component.rs:170-195`, except the ambiguous-broker message which MUST carry the broker count per this change's spec: `Multiple RabbitMQ brokers configured ({n}: {names}); specify one with ?broker=` (jms prints names only — spec requires the number). Explicit name must exist (error names `[components.rabbitmq.brokers]`); `None` + one broker selects it; `None` + zero errors naming the Camel.toml section.
6. Do NOT define a local URL redactor: every credential-bearing URL in logs/errors goes through the canonical `camel_api::redact::redact_url` (`crates/camel-api/src/redact.rs`; `pub mod redact` in `camel-api/src/lib.rs:48`; ADR-0076). The helper takes only `raw: &str` and window-masks the authority userinfo to `***@` (e.g. `amqp://u:p@h:5672/` → `amqp://***@h:5672/`).
7. `src/lib.rs` declares `pub mod config;` (only modules landed so far).

**Tests:** (unit, in `src/config.rs` `#[cfg(test)]`)
- `resolve_broker_name_single_broker_selects_implicitly`: one broker `main` → `resolve_broker_name(map, None)` → Ok(`"main"`)
- `resolve_broker_name_explicit_unknown_errors`: map has `main`, request `"ghost"` → Err whose message contains `components.rabbitmq.brokers`
- `resolve_broker_name_ambiguous_errors`: two brokers `main`/`backup`, `None` → Err whose message contains `broker=`, the count `2`, and both names (spec-required count; diverges from jms names-only)
- `resolve_broker_name_empty_errors`: zero brokers, `None` → Err naming the Camel.toml section
- `broker_config_debug_redacts_password`: `format!("{:?}", cfg)` with password `hunter2` → output contains `<redacted>` and not `hunter2`
- `broker_config_rejects_unknown_field`: deserializing a broker section with `bogus = 1` fails (deny_unknown_fields)
- `redact_url_canonical_masks_password`: `camel_api::redact::redact_url("amqp://u:p@h:5672/")` == `amqp://***@h:5672/` (asserts the canonical helper's real signature/output; the crate defines no local redactor)
- `reconnect_default_matches_jms_shape`: `rabbitmq_reconnect_default()` → enabled, max_attempts 0, initial_delay 5 s, multiplier 2.0, max_delay 30 s
- (master-amended: fail-closed TLS boot rejection; rc-l8ohw) `RabbitTlsConfig` and the `RabbitBrokerConfig.tls` field are REMOVED: the former `tls.ca_cert`/`tls.server_name` keys were parsed and silently ignored (fail-open). `deny_unknown_fields` now makes any `tls.*` key fail closed at the only deserialize entrypoint (`RabbitMqBundle::from_toml`), gated by the task 1.5 boot tests; direct struct construction can no longer express `tls` at all. Tracked in bd rc-l8ohw.
- command: `cargo test -p camel-component-rabbitmq --lib`
- expected: all green before 1.3 (pure logic)

**Acceptance:**
- `cargo build -p camel-component-rabbitmq` and `cargo clippy -p camel-component-rabbitmq -- -D warnings` exit 0
- `cargo xtask lint-publish-registration` exits non-zero reporting exactly one Case B for `camel-component-rabbitmq` (`new-unpublished`; owner action = manual classic-token first publish, then register trustpub), with no order/missing/stale findings
- `cargo fmt --check` clean; no `unwrap()` in new files

- [x] 1.1

### crates/components/camel-rabbitmq (URI metadata)

#### Task 1.2: URI parsing and metadata descriptor for the P1 option set

**Files:**
- `crates/components/camel-rabbitmq/src/config.rs` (modified)
- `crates/components/camel-rabbitmq/src/metadata.rs` (new)
- `crates/components/camel-rabbitmq/src/lib.rs` (modified)

**Steps:**
1. In `src/config.rs` add `pub struct RabbitEndpointConfig` with fields `broker: Option<String>, exchange: String, queue: Option<String>, routing_key: Option<String>, persistent: bool, content_type: Option<String>` and `pub fn from_uri(uri: &str) -> Result<Self, CamelError>` using `camel_component_api::parse_uri` (pattern: `camel-kafka/src/config.rs::KafkaEndpointConfig::from_uri`, line 485). Scheme must be `rabbitmq`; the path segment is the exchange name where empty or `default` normalizes to `""` (default exchange). Query params: `broker`, `queue`, `routingKey`, `persistent` (bool, default true), `contentType`. Unknown query params error with the param name (fail-closed like kafka).
2. Add `pub fn target(&self) -> (String, String)` returning `(exchange, routing_key)` where routing_key falls back to the queue name when `routingKey` is absent, erroring at `from_uri` time when both `routingKey` and `queue` are absent.
3. Create `src/metadata.rs` with the jms pattern (`camel-jms/src/metadata.rs`): `#[derive(UriConfig)] #[uri_scheme = "rabbitmq"] #[uri_config(skip_impl, descriptor, metadata(scheme = "rabbitmq", description = "RabbitMQ AMQP 0-9-1 messaging", producer), crate = "camel_component_api")] pub(super) struct RabbitMqMetadataDescriptor` — P1 advertises PRODUCER ONLY (no `consumer` flag until task 2.1; P1 has no consumer). Carrying ONLY the P1 fields: `_broker: Option<String>`, `_queue: Option<String>`, `_routing_key: Option<String>`, `_persistent: bool` (default "true"), `_content_type: Option<String>`.
4. Define `pub const P1_OPTIONS: &[&str] = &["broker", "queue", "routingKey", "persistent", "contentType"];` — later phases append their const lists; the parity test asserts descriptor names == the union of landed phase consts.
5. Parity test asserting sorted descriptor names equal sorted `P1_OPTIONS`, `persistent.default_value == Some("true")`, every option `required == false`, and the descriptor capability flags are `producer == true, consumer == false` in P1.

**Tests:**
- `endpoint_config_parses_default_exchange`: `from_uri("rabbitmq:default?queue=orders")` → exchange `""`, routing key `orders` via `target()`
- `endpoint_config_explicit_routing_key_wins`: `from_uri("rabbitmq:ex?queue=q&routingKey=rk")` → `target() == ("ex", "rk")` (gates the routingKey option non-vacuously)
- `endpoint_config_routing_key_falls_back_to_queue`: `from_uri("rabbitmq:ex?queue=q")` → target `("ex", "q")`
- `endpoint_config_requires_queue_or_routing_key`: `from_uri("rabbitmq:ex")` → Err naming `queue`/`routingKey`
- `endpoint_config_rejects_unknown_param`: `from_uri("rabbitmq:ex?queue=q&bogus=1")` → Err containing `bogus`
- `endpoint_config_rejects_wrong_scheme`: `from_uri("amqp:ex")` → Err
- `endpoint_config_parses_content_type`: `from_uri("rabbitmq:ex?queue=q&contentType=application/json")` → `content_type == Some("application/json")`
- `metadata_uri_options_parity_p1`: descriptor names == `P1_OPTIONS`; `persistent` default `Some("true")`, not required; capabilities producer-only
- command: `cargo test -p camel-component-rabbitmq --lib`
- expected: pass

**Acceptance:**
- `cargo clippy -p camel-component-rabbitmq -- -D warnings` exits 0; descriptor compiles (metadata harvest site wired in 1.5)

- [x] 1.2

### crates/components/camel-rabbitmq (connection manager)

#### Task 1.3: lapin connection manager with ring TLS, retry policy, generation counter

**Files:**
- `crates/components/camel-rabbitmq/Cargo.toml` (modified)
- `crates/components/camel-rabbitmq/src/connection.rs` (new)
- `crates/components/camel-rabbitmq/src/lib.rs` (modified)
- `Cargo.toml` (modified — `[workspace.dependencies]` lapin entry)
- `Cargo.lock` (modified — generated)

**Steps:**
1. Add to workspace deps: `lapin = { version = "4.12", default-features = false, features = ["tokio", "rustls--ring", "rustls-native-certs"] }`. Add `lapin.workspace = true` to the crate. VERIFY the feature names against lapin 4.12.0's actual feature table (`cargo add lapin --dry-run` or docs.rs): required properties are tokio executor integration, rustls TLS with the ring provider, system root store; the bare `rustls` feature pulls aws-lc and MUST NOT be used. Record the final feature list in a Cargo.toml comment.
2. `src/connection.rs`: `pub struct RabbitConnectionManager { url: String, inner: tokio::sync::RwLock<ManagerState>, retry: NetworkRetryPolicy, connect_fn: Arc<dyn Fn(&str) -> ConnectFuture + Send + Sync>, status_tx: tokio::sync::watch::Sender<ConnStatus>, cancel: tokio_util::sync::CancellationToken }` with `ManagerState { conn: Option<Arc<lapin::Connection>>, generation: u64 }`, `enum ConnStatus { Disconnected, Connecting, Connected }`, and `type ConnectFuture = Pin<Box<dyn Future<Output = Result<lapin::Connection, lapin::Error>> + Send>>`. Construction resolves `url` from the broker configuration and installs the real `lapin::Connection::connect` adapter; `connect_fn` IS the test seam. Never expose `url` through Debug. Add `tokio-util.workspace = true` (CancellationToken; jms already depends on it).
3. `async fn connect(self: &Arc<Self>) -> Result<(), CamelError>` — single-flight: `ensure_connecting()` (step 4) claims the flight and spawns the detached owner when the watched status is Disconnected, then `connect` awaits `wait_until_settled()` (Connected → Ok; Disconnected → Err naming the redacted URL). The detached owner is the sole writer of the Connecting→Connected/Disconnected transitions, so an abandoned caller future only abandons the waiter and can never wedge `Connecting` (master-amended, reviewer finding (a)). The retry core delegates to the detached-input helper via `self.url.clone()` and `|| connect_fn(&url)`. The helper signature is `(policy, scheme, operation, op, is_retryable, cancel, metrics)` at `crates/components/camel-component-api/src/network_retry.rs:350`; real callers include camel-container and camel-ws, not jms. `metrics` is None in 1.3. On success store the connection, increment generation and broadcast Connected; exhausted retries broadcast Disconnected and return Err. Error/log URLs use `camel_api::redact::redact_url(&url)`.
4. `pub fn ensure_connecting(self: &Arc<Self>)` — when the watched status is Disconnected, performs the single-flight transition then spawns a detached reconnect task capturing ONLY `Weak<Self>` plus cloned inputs (`url`, `retry`, `connect_fn`, `cancel`, `status_tx`). No strong manager reference survives an await. Factor `async fn run_reconnect(weak: Weak<Self>, url: String, retry: NetworkRetryPolicy, connect_fn: Arc<dyn Fn(&str) -> ConnectFuture + Send + Sync>, cancel: CancellationToken, status_tx: watch::Sender<ConnStatus>)`. Race the entire retry future against cancellation: `tokio::select! { biased; _ = cancel.cancelled() => return, result = retry_async_cancelable(&retry, "rabbitmq", "connect", || connect_fn(&url), |_: &lapin::Error| true, &cancel, None) => result }`. The helper cancels retry sleeps only; this outer race drops even a pending connect attempt. The spawned path uses `Arc::downgrade(self)`; caller-owned `connect` delegates through `ensure_connecting` (step 3). Upgrade the weak reference only after the result to publish connection/generation/status; if upgrade fails, exit.
5. `pub async fn connection_within(self: &Arc<Self>, bound: Duration) -> Result<(Arc<lapin::Connection>, u64), CamelError>` — `ensure_connecting()` then `tokio::time::timeout(bound, wait status == Connected)`; on timeout or exhausted retries Err with `disconnected` (bounded, never hangs).
6. `pub fn note_failure(self: &Arc<Self>)` — `claim_failure_clear()` captures the current generation, then wins the `Connected -> Disconnected` transition; only that winner clears the stored connection and only when the generation still matches (`ManagerState::clear_if_generation`; keep the generation, the next successful connect bumps it). A `Connecting` flight owns the slot, so an in-window failure cannot clear the connection `run_reconnect` is about to publish (master-amended, reviewer findings (b) plus final-fix finding 1). Connection-level broker-side failures reach `note_failure` through lapin 4.12 `Connection::events_listener()` on `lapin::Event::Error(_)`; a closed event stream (`None`) calls `note_failure` too, since the live connection is gone (final-fix finding 3) — lapin 4 has no `Connection::on_error`.
7. `pub async fn publisher_channel(&self) -> Result<(lapin::Channel, u64), CamelError>` — reads the stored connection + generation and `create_channel()`.
8. `impl Drop for RabbitConnectionManager` cancels `self.cancel`; the detached reconnect task (step 4) observes the cancellation and exits, and its `Weak<Self>` upgrade then yields `None` (no strong owner keeps the manager alive across the retry, and the in-flight attempt is dropped).
9. `pub const PUBLISH_DISCONNECTED_BOUND: Duration = Duration::from_secs(2)` — the producer's `connection_within` bound (task 1.4).

**Tests:**
- `retry_policy_bounds_connect_attempts`: `NetworkRetryPolicy { enabled: true, max_attempts: 2, initial_delay: 1 ms, .. }` + counting failing `connect_fn` → `connect().await` Err AND the fake recorded exactly 2 attempts
- `note_failure_spawns_single_reconnect`: two rapid `note_failure()` calls → the counting `connect_fn` shows exactly 1 in-flight/slow attempt (single-flight; pends via a gate future)
- `connection_within_bounded_when_unreachable`: `connect_fn` pends forever → `connection_within(100 ms)` returns Err within ~200 ms
- `dropping_last_manager_arc_cancels_pending_reconnect`: `connect_fn` pends forever (its live future tracked by a Drop guard); `manager.ensure_connecting()` spawns the detached reconnect; capture a `Weak<Self>` then drop the last `Arc` → the pending attempt future is dropped (guard observed firing) and the `Weak::upgrade()` is `None`
- `retry_policy_defaults_match_jms`: manager built from `rabbitmq_reconnect_default()` stores max_attempts 0 / initial 5 s (assert on stored policy)
- `caller_abandoned_connect_does_not_wedge_single_flight` (master-amended): setup — `connect_fn` returns a gated future that notifies `started`, awaits `release`, then fails; policy `NetworkRetryPolicy { enabled: true, max_attempts: 1, initial_delay: 1 ms, .. }`; spawn `manager.connect()`, await `started`, then `caller.abort()`. action — bounded `connection_within(50 ms)` while the attempt is still pending. assert — status stays `Connecting` after the abort (owner not cancelled); after `release.notify_one()` status becomes `Disconnected` and the waiter errs; `ensure_connecting()` then admits a second attempt (`attempts == 2`). command — `cargo test -p camel-component-rabbitmq --lib caller_abandoned_connect_does_not_wedge_single_flight`.
- `deferred_failure_clear_does_not_erase_new_generation` (master-amended): setup — publish a NEWER generation through the state seam (`state.generation = 1`, status `Connected`) WITHOUT constructing a fake `lapin::Connection`; then simulate the `run_reconnect` store-then-publish window (`state.generation = 2`, status `Connecting`). action — apply the captured stale clear via `apply_failure_clear(0)`; then call `claim_failure_clear()` and `note_failure()` in the Connecting window. assert — `apply_failure_clear(0)` returns `false`, generation stays `1`, status stays `Connected`; `apply_failure_clear(1)` still returns `true`; in the Connecting window `claim_failure_clear()` is `None`, `note_failure()` leaves status `Connecting` and generation `2`. command — `cargo test -p camel-component-rabbitmq --lib deferred_failure_clear_does_not_erase_new_generation`.
- `runtime_parent_cancel_stops_pending_reconnect` (master-amended: derived cancellation remedy): a fake `ComponentContext` hands the manager a swappable, externally created parent shutdown token (test-scope root, lint-exempt; no route-scoped token). With a pending connect attempt (Drop guard), `parent.cancel()` drops the attempt future (guard fires) and the flight settles to `Disconnected` — never stranded `Connecting`. Swapping a fresh parent into the same slot (runtime restart) admits a second attempt (`attempts == 2`). command — `cargo test -p camel-component-rabbitmq --lib runtime_parent_cancel_stops_pending_reconnect`.
- `dropping_last_manager_arc_cancels_pending_reconnect` (extended, master-amended): the no-context manager mints no token; dropping the last `Arc` aborts and drains its tracked-task registry (asserted empty) so the pending attempt future is dropped. command — `cargo test -p camel-component-rabbitmq --lib dropping_last_manager_arc_cancels_pending_reconnect`.
- `runtime_parent_cancel_demotes_live_connection_and_rearms` (master-fix, derived cancellation remedy): with the live-connection state published through the seam (`Connected` + generation 1; no fake `lapin::Connection`), the error listener's parent-cancel demotion (`demote_connection`) settles the manager to `Disconnected` and clears the slot WITHOUT spawning a reconnect against the cancelled parent (`attempts == 0`); a second call is idempotent (no repeat claim / respawn loop). A fresh parent in the same slot then re-arms and admits exactly one new attempt. This closes the latent defect where the listener broke silently and left a dead connection marked `Connected` (restart never re-armed). command — `cargo test -p camel-component-rabbitmq --lib runtime_parent_cancel_demotes_live_connection_and_rearms`.
- command: `cargo test -p camel-component-rabbitmq --lib`
- expected: pass (original 5 + master-amended regressions: caller-abandoned, deferred-clear, parent-cancel pending, parent-cancel demotion; 30 tests total)
- aws-lc guard (acceptance, component-OWNED edges only): the component selects lapin features exactly `["tokio", "rustls--ring", "rustls-native-certs"]` (no bare `rustls`, no `rustls--aws_lc_rs`); evidence `cargo tree -p camel-component-rabbitmq -e features -i lapin` plus `cargo tree -p camel-component-rabbitmq -e features -i aws-lc-rs`, where lapin's `rustls--ring` selects only `rustls feature "ring"` while `rustls feature "aws_lc_rs"` is enabled by `camel-component-api`'s `rustls feature "default"`. Pre-existing workspace `aws-lc-rs` reachability through `camel-component-api`/`camel-auth`/workspace-`rustls` default features is OUT OF SCOPE and tracked in bd `rc-2bofp`; this task performs no shared-dependency fix (master ruling 1).

**Acceptance:**
- Component-OWNED TLS edge is aws-lc-free: `cargo tree -p camel-component-rabbitmq -e features -i lapin` shows the component selecting only `tokio`/`rustls--ring`/`rustls-native-certs`, and `cargo tree -p camel-component-rabbitmq -e features -i aws-lc-rs` shows `rustls feature "aws_lc_rs"` entering via `camel-component-api`'s `rustls feature "default"`, not via any lapin-selected feature. The literal zero-reverse-graph gate (`cargo tree -p camel-component-rabbitmq -i aws-lc-rs` empty) is NOT a gate here: pre-existing reachability via `camel-component-api`/`camel-auth`/workspace-`rustls` defaults is tracked in bd `rc-2bofp` (master ruling 1).
- `cargo build -p camel-component-rabbitmq` exits 0; clippy `-D warnings` clean; Cargo.toml comment records the verified feature list
- `cargo test -p camel-component-rabbitmq --lib` green (original 5 + 2 master-amended regression tests); `cargo fmt --check --all` clean

**Master ledger (task 1.3 amendment, 2026-10-08):**
- Master ruling 1 — aws-lc ergonomics: replaced the impossible whole-component zero-reverse-graph requirement with the component-OWNED edge check (`cargo tree -p camel-component-rabbitmq -e features -i lapin` and `-i aws-lc-rs`). Pre-existing workspace reachability via `camel-component-api`/`camel-auth`/workspace-`rustls` defaults is out of scope, tracked in bd `rc-2bofp`; no shared-dependency fix is performed here.
- Master ruling 2 — authorized regression amendment: fixed reviewer findings (a) caller-abandoned `connect()` wedging `Connecting` (the detached owner now always holds the flight) and (b) deferred `note_failure` clear erasing a newer connection (now generation-checked via `ManagerState::clear_if_generation`). Both are gated by the two master-amended tests above, written red first. Original five tests remain green.
- Minor citation fix: lapin 4.12 delivers connection errors via `Connection::events_listener()` + `lapin::Event::Error(_)` (there is no `Connection::on_error`).
- Master final-fix round (r_glm APPROVE-WITH-FINDINGS): (1) `note_failure` now only clears a slot whose `Connected -> Disconnected` transition it wins (`claim_failure_clear`, which always attempts the transition and captures the generation first), so a failure in the `run_reconnect` store-then-publish window cannot erase the fresh connection and lock contention cannot suppress recovery; the existing `deferred_failure_clear_does_not_erase_new_generation` test was extended (same (b) intent) to cover that window. (2) No refactor of the try-write/spawn clear path — the fast path plus generation-guarded deferred fallback is the minimal necessary shape. (3) A closed `events_listener` stream now calls `note_failure` before breaking, matching `Event::Error`; not unit-testable with existing seams because a `lapin::Connection` cannot be constructed off-broker (gap recorded here, exercised by task 2.x docker tests).
- Master remedy (master-amended: derived cancellation remedy, 2026-10-08): `RabbitConnectionManager` no longer mints a root `CancellationToken::new()` — that 8th site had pushed `cargo xtask lint-cancel-tokens` to 8 > max 7; the removal restores 7 == max. Cancellation is now derived: the manager takes an optional `Arc<dyn ComponentContext>` lifecycle supplier (`with_lifecycle_context`; `from_broker_config` unchanged, `RabbitMqComponent::with_lifecycle_context` threads a bundle-installed slot-bound context into each manager). `ensure_connecting` derives a manager-local `child_token()` of the CURRENT `shutdown_token()` at every activation (not at construction, not from a route/consumer token), so a runtime stop cancels a pending attempt while the manager survives for the next start; `Drop` cancels only that child, never the parent. With no bound context the manager mints no token and records detached-task `JoinHandle`s in a `TaskRegistry`, aborting/draining them on `Drop` to drop pending retry futures and error listeners. `run_reconnect` uses `retry_async_cancelable` under an outer cancel race when a child is bound, else `retry_async` + abort-on-Drop; every cancelled or failed flight resets the watched status to `Disconnected`, so a runtime restart is never stranded in `Connecting`. Constructor change: `new`/`from_broker_config` keep their signatures (existing tests preserved) and gain the `with_lifecycle_context` builder. Test-only dev-dep `camel-language-api` added (the fake `ComponentContext`'s `resolve_language` return type). Bundle wiring is task 1.5 (see amendment).
- Reviewer defect fix (2026-10-08, fixed now — NOT deferred to 1.5): the live-connection error listener's parent-cancel branch previously `break`ed silently, leaving the dead connection stored and the watched status `Connected`, so a runtime restart never re-armed. The demotion half of `note_failure` is factored into `fn demote_connection(self: &Arc<Self>) -> bool` (generation-guarded clear of the `Connected -> Disconnected` transition); `note_failure` is now `demote_connection() + ensure_connecting()`, and the listener's `wait_cancelled` branch calls `demote_connection()` only, then breaks. Demoting without re-arming is deliberate: against an already-cancelled parent a reconnect would only spawn a doomed task (hot-loop risk), so re-arming is left to the next activation with a fresh parent. No fresh root token is minted; the existing state seam verifies the fix (no fake `lapin::Connection`).
- Sleep-gate fix (2026-10-08): the two amended cancellation tests (`dropping_last_manager_arc_cancels_pending_reconnect`, `runtime_parent_cancel_stops_pending_reconnect`) no longer settle with a 10 ms sleep. `DropFlag` now also signals a `Notify` on drop; the first test awaits that signal, the second awaits the `status_tx` watch change to `Disconnected`. Test intents are unchanged and no tests were added; this keeps the lexical `lint-test-sleep` count at the 471 ratchet (a paused clock does not waive the scanner).

- [x] 1.3

### crates/components/camel-rabbitmq (producer)

#### Task 1.4: Producer — publish mapping, confirms-always-on, bounded disconnected fail, publish metrics

**Files:**
- `crates/components/camel-rabbitmq/src/producer.rs` (new)
- `crates/components/camel-rabbitmq/src/component.rs` (new)
- `crates/components/camel-rabbitmq/src/lib.rs` (modified)

**Steps:**
1. `src/component.rs`: `pub struct RabbitMqComponent { managers: HashMap<String, Arc<RabbitConnectionManager>>, brokers: HashMap<String, RabbitBrokerConfig>, retry: NetworkRetryPolicy }` implementing `camel_component_api::Component`: `scheme()` returns `"rabbitmq"`, `create_endpoint(uri, ctx)` parses `RabbitEndpointConfig::from_uri`, resolves the broker name via `resolve_broker_name`, looks up its manager (create-once per broker name), calls `ctx.register_current_route_health_check(Arc::new(RabbitHealthCheck::new(manager.clone())))` (kafka shape, `camel-kafka/src/lib.rs:117-119` — health lands in task 1.5; until then this call site compiles against a stub-free crate only after 1.5, so THIS task leaves the health line out and 1.5 adds it), and returns an endpoint owning config + manager.
2. Endpoint implements the `Endpoint` trait (`camel-component-api/src/endpoint.rs:27`): `create_producer(rt, ctx)` returns `RabbitProducer { config, manager, rt }` where `rt: Arc<dyn RuntimeObservability>` arrives as the trait argument (NOT from the component context — camel-ws `crates/components/camel-ws/src/client_consumer.rs` uses the same injection point); `create_consumer` in P1 returns `CamelError::Config("rabbitmq consumer not available in this build")` (task 2.1 replaces it).
3. `src/producer.rs`: `impl tower::Service<Exchange> for RabbitProducer` (camel-jms `producer.rs:110` shape — `poll_ready`/`call`).
4. `call` flow: `manager.connection_within(PUBLISH_DISCONNECTED_BOUND).await?` (bounded fail while disconnected, never hangs); map body to `Vec<u8>` (match jms body mapping); build properties via `pub(crate) fn build_properties(persistent: bool, content_type: Option<&str>, headers: &camel_api::Headers) -> BasicProperties` — `delivery_mode` 2/1 by `persistent`, `content_type` from the URI option first then the exchange's `contentType` header, and every NON-reserved header (`contentType`, `contentEncoding`, `priority`, `messageId`, `correlationId`, `replyTo`, `expiration`, `timestamp` are reserved) placed into the `BasicProperties.headers` FieldTable as LongString values (task 2.4 moves this into `headers.rs` and adds full two-way mapping); `channel.basic_publish(exchange, routing_key, BasicPublishOptions::default(), payload, props).await?` with `(exchange, routing_key)` from `config.target()`.
5. Confirms are ON from P1 (deterministic failure detection): the producer channel calls `channel.confirm_select()` once at creation; every publish awaits the PublisherConfirm under `pub(crate) const DEFAULT_CONFIRM_TIMEOUT: Duration = Duration::from_secs(5)` — a channel-close error (e.g. 404 missing exchange) or confirm nack counts as a publish failure. The `confirmTimeout` URI option only SURFACES in P3 (task 3.1) to override the default — no option is parsed early.
6. Emit `camel_component_operations_total{component="rabbitmq", operation="publish", outcome=success|failure}` through the `RuntimeObservability` handle after each publish.
7. On connection/channel-death error classes, call `manager.note_failure()` so the reconnect loop owns recovery.

**Tests:**
- `build_properties_persistent_default_and_override`: persistent=true → `delivery_mode == Some(2)`; `persistent=false` → 1; URI `contentType` wins over the header; header fallback used when URI option absent
- `build_properties_maps_free_form_headers`: `camel_api::Headers` with `x-custom=abc` → FieldTable carries `x-custom` LongString `abc`; reserved names do NOT land in the FieldTable
- `metadata parity re-assert`: producer uses `config.target()` (unit re-assert of 1.2 targets)
- `publish_disconnected_fails_bounded`: manager stub with pending `connect_fn` → `call` returns Err within `PUBLISH_DISCONNECTED_BOUND + 500 ms` (tokio elapsed assert)
- `publish_outcome_mapping`: `RecordingRuntimeObservability::new(true)` (camel-component-api `test_support.rs:193`, dev-dep `camel-component-api = { workspace = true, features = ["test-support"] }`) wired as `rt` → success/failure tuple mapping asserted at the emission seam `fn record_publish(rt, outcome)` (docker end-to-end in 1.6)
- `channel_publish_failure_preserves_connection_and_next_publish_succeeds` (master-authorized, P1 exit ledger): real docker fixture. Setup — declare a durable queue; build a `RabbitConnectionManager::from_broker_config` against the fixture and capture the live connection `Arc` identity plus generation via `connection_within`; subscribe the manager status watch. Action — publish to a non-existent exchange with a producer on that manager (soft 404 surfaces through the confirm wait), then publish to the default exchange targeting the queue with a second producer. Assert — the 404 returns `Err`; the status watch observes no change within a 250 ms bound (the listener must ignore the soft error); `connection_within` returns the same connection `Arc` (`Arc::ptr_eq`) and the same generation; the second publish succeeds and `basic_get` returns the body. The test lives in `src/connection.rs` and reuses `tests/common` through a test-only `#[path]` include, so it reads the private identity/generation/status seam without a production test hook. Command — `RABBITMQ_ITEST=1 cargo test -p camel-component-rabbitmq --lib channel_publish_failure_preserves_connection_and_next_publish_succeeds`.
- command: `cargo test -p camel-component-rabbitmq --lib`
- expected: pass

**Acceptance:**
- `cargo test -p camel-component-rabbitmq --lib` green; clippy `-D warnings` clean; fmt clean; lint-cancel-tokens/lint-unbounded-wait clean on new files

**Master ledger (task 1.4 amendment, 2026-10-09):**
- Master ruling — channel isolation: a channel-scoped publisher error recreates ONLY the channel; a full reconnect happens ONLY for connection-level errors. Both the producer (`mark_channel_dead`) and the connection error listener (`spawn_error_listener`) classify through `is_connection_level_error`: AMQP soft errors (404 NOT_FOUND, 406 PRECONDITION_FAILED) are channel-scoped; all other errors are connection-scoped (fail-safe). Verified empirically that lapin 4.12 delivers a channel-close soft error to `Connection::events_listener()` as `Event::Error`, so the producer-only fix still failed the regression and the listener had to classify too.
- `publisher_channel` removed: it had no callers. The task 2.3 consumer seam (`consumer_channel`) re-introduces the channel+generation accessor. No production public test hook was added; the regression reads the private seam from an in-crate test.
- README records the P1 limitation: of the eight reserved basic-property names only `contentType` is mapped outbound; the full two-way mapping remains task 2.4. The phase-2.4 spec is unchanged.
- TLS remains out of scope for this fix and is tracked in bd `rc-l8ohw` (fail-closed boot rejection already landed at 9cef07b4); no TLS work is planned here.
- Regression written red first: it failed at the bounded status assertion against the pre-amendment `mark_channel_dead`, failed again with only the producer fix (the listener still demoted the connection), then passed with both classified. The `tests/publish_roundtrip.rs` fixture remains exactly four tests; the new test is a lib test (38 lib tests total).

- [x] 1.4

### crates/components/camel-rabbitmq + camel-bundles + camel-cli (wiring)

#### Task 1.5: Health check, bundle registration, lint catalog, regular-CLI wiring, goldens

**Files:**
- `crates/components/camel-rabbitmq/src/health.rs` (new)
- `crates/components/camel-rabbitmq/src/bundle.rs` (new)
- `crates/components/camel-rabbitmq/src/component.rs` (modified — health registration line)
- `crates/components/camel-rabbitmq/src/connection.rs` (modified — behavior-identical cancellation diagnostic block to satisfy lint-secrets)
- `crates/components/camel-rabbitmq/src/lib.rs` (modified)
- `crates/camel-bundles/Cargo.toml` (modified)
- `crates/camel-bundles/src/lib.rs` (modified)
- `crates/camel-cli/Cargo.toml` (modified)
- `crates/camel-cli/src/lib.rs` (modified — lint catalog)
- `crates/camel-cli/tests/feature_profiles.rs` (modified)
- `crates/camel-cli/tests/fixtures/default-deptree.txt` (modified — regenerated)

**Steps:**
1. `src/health.rs` kafka shape (`camel-kafka/src/health.rs`): `trait RabbitProbe: Send + Sync { fn probe(&self) -> ProbeFuture }`, real probe = `manager.connection_within(timeout)`; `pub struct RabbitHealthCheck { probe: Arc<dyn RabbitProbe>, timeout: Duration }` (timeout 5 s) implementing `camel_api::AsyncHealthCheck` returning `CheckResult::unhealthy(err)` on error or timeout.
2. `src/component.rs` (modified): in `create_endpoint` add `ctx.register_current_route_health_check(Arc::new(RabbitHealthCheck::new(Arc::clone(&manager))))` (kafka `lib.rs:117-119` shape).
3. `src/bundle.rs` kafka shape, extended with the lifecycle context (master-amended: derived cancellation remedy): `pub struct RabbitMqBundle { config: RabbitComponentConfig, lifecycle_context: Option<Arc<dyn camel_component_api::ComponentContext>> }` implementing `ComponentBundle` with `config_key() == "rabbitmq"`, `from_toml(value)` deserializing `RabbitComponentConfig` (deny_unknown_fields via 1.1) with `lifecycle_context: None`, an inherent `pub fn with_lifecycle_context(self, context: Arc<dyn ComponentContext>) -> Self` (mirrors the Wasm bundle's captured context), and `register_all` building `RabbitMqComponent::new(self.config)`, applying `.with_lifecycle_context(ctx)` when `Some`, and registering the resulting component. Unit tests keep the `TestRegistrar` pattern; `from_toml`-only tests exercise the `None` (abort-on-Drop) path.
4. camel-bundles `Cargo.toml`: optional dep `camel-component-rabbitmq`, feature `rabbitmq = ["dep:camel-component-rabbitmq"]`, AND add `"rabbitmq"` to camel-bundles `default` feature list (line ~77 — mqtt/jms are default there, so regular builds register it without extra flags).
5. camel-bundles `src/lib.rs`: in the boot cascade registration site (cfg-gated block where kafka/mqtt register) register RabbitMQ from `[components.rabbitmq]`. Unlike the plain `register_bundle::<RabbitMqBundle>(ctx, config)` seam, the rabbitmq branch MUST construct the bundle with a slot-bound lifecycle context so managers observe the runtime shutdown token — the bundle holds the `ComponentRegistrar` only, so the context must be injected before `register_all`. Follow the Wasm pattern (`crates/camel-bundles/src/lib.rs:429-441`): build `let lifecycle: Arc<dyn camel_component_api::ComponentContext> = Arc::new(camel_core::RegistryComponentContext::new(ctx.registry_arc(), Some(ctx.metrics()), camel_component_api::ComponentContext::component_metrics_enabled(&*ctx)).with_shutdown_slot(ctx.shutdown_token_slot()));`, then `let bundle = bundle_from_config::<camel_component_rabbitmq::RabbitMqBundle>(config)?.with_lifecycle_context(lifecycle);` and `<camel_component_rabbitmq::RabbitMqBundle as camel_component_api::ComponentBundle>::register_all(bundle, ctx);`. `RegistryComponentContext` holds only a `Weak` registry plus the shared shutdown slot (never the `CamelContext`), so the component-lifetime context creates no reference cycle and resolves the CURRENT token across stop/start; do not snapshot a token at construction. Extend the existing `boot_registers_all_bundles_from_fixture_config` test with `#[cfg(feature = "rabbitmq")] assert!(registry resolves scheme "rabbitmq")` and the slim twin `boot_slim_registers_core_without_bridges` with the negative assertion. Note: the error-listener parent-cancel demotion (settling `Disconnected` on a runtime stop so a restart re-arms) is already implemented and tested in task 1.3 via `demote_connection` — fixed now, not deferred to this wiring; this step only injects the lifecycle context.
6. camel-cli `Cargo.toml`: optional dep line near jms (line ~66), feature `rabbitmq = ["dep:camel-component-rabbitmq", "camel-bundles/rabbitmq"]`, add `"rabbitmq"` to `flavor-regular` (line ~177) and to the standalone legacy `full` list (line ~155).
7. camel-cli `src/lib.rs` lint catalog: next to the mqtt/kafka `register_bundle_empty!` calls (lines ~150-180) add `#[cfg(feature = "rabbitmq")] register_bundle_empty!(ctx, camel_component_rabbitmq::RabbitMqBundle);` — this is the metadata harvest site behind the `camel lint` cascade.
8. `crates/camel-cli/tests/feature_profiles.rs`: add `"camel-component-rabbitmq v"` to `REGULAR_REQUIRED_PREFIXES` (line ~460).
9. Regenerate the golden with the command documented in `feature_profiles.rs` header: `CARGO_TERM_COLOR=never cargo tree -p camel-cli -e features,no-dev --prefix none --locked | sed -E 's| \(/[^)]*\)||g; s| \(\*\)||g; s| \[\*\]||g' | LC_ALL=C sort -u > crates/camel-cli/tests/fixtures/default-deptree.txt` (run in worktree).

**Tests:**
- `health_unhealthy_on_probe_error` (unit, health.rs): fake probe returning Err → `check()` yields Unhealthy
- `health_probe_timeout_bounds_wait` (unit): fake probe sleeping 10 s, `RabbitHealthCheck` timeout 50 ms (test override) → Unhealthy within ~1 s
- `rabbit_bundle_from_toml_empty_registers_scheme` (unit, bundle.rs): TestRegistrar pattern from `camel-kafka/src/bundle.rs` tests — empty toml → Ok; register_all registers scheme `rabbitmq`
- `rabbit_bundle_from_toml_rejects_unknown_key`: toml with `bogus = 1` → Err (deny_unknown_fields)
- `tls_ca_cert_rejected_at_boot` (master-amended: fail-closed TLS boot rejection; rc-l8ohw): a broker with a valid `url` plus `[brokers.main.tls] ca_cert = ...` → `RabbitMqBundle::from_toml` Err naming `tls`, the broker section, and the supported `url`/`username`/`password`/`vhost`; asserts no secret (password) or tls value is echoed
- `tls_server_name_rejected_at_boot` (master-amended: fail-closed TLS boot rejection; rc-l8ohw): `tls = { server_name = ... }` → same named Err listing supported params, server_name value not echoed
- `empty_tls_section_rejected_at_boot` (master-amended: fail-closed TLS boot rejection; rc-l8ohw): an empty `tls = {}` section is rejected at boot, never silently ignored
- `boot_registers_all_bundles_from_fixture_config` extension (camel-bundles lib): with feature on, registry resolves `rabbitmq`
- `regular_closure_includes_rabbitmq` (camel-cli): `cargo test -p camel-cli --test feature_profiles` — closure tests + updated REQUIRED list + regenerated golden all pass
- `slim_plus_rabbitmq_resolves_rabbitmq_only` (feature_profiles.rs): clone `slim_plus_grpc_resolves_grpc_only` (line ~499) with `--no-default-features --features flavor-slim,rabbitmq` → `camel-component-rabbitmq v` and `lapin v` present, `SLIM_FORBIDDEN_PREFIXES` absent
- command: `cargo test -p camel-component-rabbitmq --lib && cargo test -p camel-bundles --lib && cargo test -p camel-cli --test feature_profiles`
- expected: pass after golden regen

**Acceptance:**
- `cargo build --workspace` exits 0
- `cargo tree -p camel-cli --features flavor-regular -e no-dev | grep camel-component-rabbitmq` non-empty
- `cargo xtask lint-gate-forwarding` exits 0; `cargo xtask lint-publish-registration` exits non-zero with exactly the expected one Case B for `camel-component-rabbitmq` (`new-unpublished`) and no other findings
- `cargo xtask schema --check` exits 0 (type-level gate — the real metadata gate is the lint-catalog registration in step 7 plus the parity test)
- clippy clean on all touched crates

- [x] 1.5

### crates/components/camel-rabbitmq (docker fixture + publish integration)

#### Task 1.6: Shared RabbitMQ docker test fixture and publish integration test

**Files:**
- `crates/components/camel-rabbitmq/tests/common/mod.rs` (new)
- `crates/components/camel-rabbitmq/tests/publish_roundtrip.rs` (new)
- `crates/components/camel-rabbitmq/src/connection.rs` (modified — replace branch-introduced settling sleeps with existing DropFlag/Notify and watch synchronization)

**Steps:**
1. `tests/common/mod.rs` with `#![allow(dead_code)]` (helpers accrue across phases): `pub enum Gate { Skip(String), Run }` + pure core `pub fn gate(itest: Option<&str>, docker_ok: impl Fn() -> bool) -> Gate` — `itest` None → `Gate::Skip` carrying the notice naming `RABBITMQ_ITEST=1`; `itest` Some + `docker_ok()` false → panic `infra-unavailable: rabbitmq tier requires docker (RABBITMQ_ITEST=1)`; else Run. `pub fn require_fixture() -> Option<RabbitFixture>` reads `std::env::var("RABBITMQ_ITEST")`, applies `gate()`; on Skip it `eprintln!`s the notice and returns None; on Run it builds the fixture. Every docker test starts with `let Some(fx) = common::require_fixture() else { return; };`.
2. `pub struct RabbitFixture { container_id: String, amqp_url: String }`: `docker run -d --name rmq-itest-<pid>-<nanos> -p 127.0.0.1:<port>:5672 -e RABBITMQ_DEFAULT_USER=rmq -e RABBITMQ_DEFAULT_PASS=rmq rabbitmq:3.13-alpine` (NO `--rm` — Drop runs `docker rm -f` and `--rm` would race `docker restart` in task 2.5). The host side is bound to `127.0.0.1` so the actual bind matches the loopback URL the fixture hands out, and the named default credentials are FIXTURE-ONLY (not application secrets): the official image's default `guest` user authenticates only from the container loopback, while a host connection arrives from the docker bridge gateway and is refused, so the fixture must create a named default user/pass. Readiness = real lapin connect+channel+close polling up to 60 s; `Drop` runs `docker rm -f <id>`. Fixture helpers: `declare_queue(name)`, and (landed with the tasks that need them) `start_on_port(port)`, `restart()`, `pause()`/`unpause()`.
3. `tests/publish_roundtrip.rs`: `#[tokio::test] async fn publish_then_basic_get_round_trip()` — `let Some(fx) = require_fixture() else { return };`; declare a durable queue via `fx.declare_queue`; build `RabbitMqComponent` with one broker at the fixture URL; `create_endpoint("rabbitmq:default?queue=<q>", …)`; `create_producer(rt, ctx)` with `rt = RecordingRuntimeObservability::new(true)`; `call` one exchange with body `hello` and header `x-custom=abc`; raw-lapin `basic_get` → assert payload `hello`, header `x-custom=abc` present, `delivery_mode == 2`; AND `rt` recorded `("rabbitmq", "publish", "success")` exactly once (P1 has no consumer; verification is basic_get per design).
4. Same file: `#[tokio::test] async fn publish_to_missing_exchange_counts_failure()` — publish to exchange `ghost.<rand>` → producer returns Err (channel 404 close surfaces through the confirm wait) and `rt` recorded `("rabbitmq", "publish", "failure")`.
5. Same file: `#[test] fn gate_rules_binary()` — `gate(None, || true)` → Skip with notice naming `RABBITMQ_ITEST=1`; `gate(Some("1"), || false)` panics: assert with `#[should_panic(expected = "infra-unavailable")]` on a wrapper fn.

**Tests:**
- `publish_then_basic_get_round_trip`: gates "default exchange publish target", "persistent default publish", "free-form header round trips" producer half, "fixture round trip", "publish outcome counted" success half
- `publish_to_missing_exchange_counts_failure`: gates "publish outcome counted" failure half (404 via confirms)
- `gate_rules_binary`: gates "unset gate prints notice and does not run" + "gated tier without docker panics"
- pre-step (activated tier, agent-owned disk): disk guard (`df`) then `timeout 300 docker pull rabbitmq:3.13-alpine` BEFORE the `RABBITMQ_ITEST=1` cargo invocation, so the image pull runs under the agent's disk ownership rather than inside the fixture's bounded `docker run` (the existing 30 s `DOCKER_CMD_TIMEOUT` is a docker-command bound, not an image-pull budget)
- command: `RABBITMQ_ITEST=1 cargo test -p camel-component-rabbitmq --test publish_roundtrip` and (without env) `cargo test -p camel-component-rabbitmq --test publish_roundtrip` (notice path, tests return early, exit 0)
- expected: pass with docker (after the pre-pull pre-step); ungated run exits 0 with notice

**Acceptance:**
- Both invocations exit 0; no leftover `rmq-itest-*` containers after the run

**Master ledger (task 1.6 draft fixes, 2026-10-08):**
- Loopback port mapping: the host side binds `127.0.0.1:<port>:5672`, so the actual bind matches the `amqp://…@127.0.0.1:<port>` fixture URL instead of binding all interfaces.
- Fixture-only credentials: `RABBITMQ_DEFAULT_USER=rmq` / `RABBITMQ_DEFAULT_PASS=rmq` are FIXTURE-ONLY, not application secrets. This is justified by the actual RabbitMQ restriction: the official image's `guest` user authenticates only from the container loopback, and a host connection arrives from the docker bridge gateway, so the fixture must create a named default user/pass. Owner defaults and normative spec behavior are unchanged.
- Third gate removed: the local image-presence check (`docker image inspect` then panic) is removed as unblessed gate state — `docker run` pulls a missing image on demand. The activated tier instead owns the controlled pre-pull pre-step above so the agent owns the disk cost. No spec or normative-behavior edit.
- `nanos()` is now `pub(crate)` in `tests/common/mod.rs` and reused by both test binaries (the `publish_roundtrip` duplicate is removed); `publish_roundtrip` stays at exactly four tests.

**Verification ledger (task 1.6):** Runtime startup-failure cleanup is structurally reviewed, but deterministic failure injection needs a new Docker-command test seam absent from the blessed plan; no extra test was invented. Existing four tests pass against the real broker. Earlier timeout failures are retained in logs. The worker's before/after counts were not logged; conductor subsequently ran `timeout 30 docker ps -a --filter "name=rmq-itest-" --format "{{.ID}} {{.Names}} {{.Status}}"` and observed no matching containers (exit 0, disk 76%).

- [x] 1.6

## Phase 2: Consumer + delivery semantics

### crates/components/camel-rabbitmq (consumer core)

#### Task 2.1: Consumer loop with explicit readiness and clean stop

**Files:**
- `crates/components/camel-rabbitmq/src/consumer.rs` (new)
- `crates/components/camel-rabbitmq/src/component.rs` (modified)
- `crates/components/camel-rabbitmq/src/metadata.rs` (modified — consumer capability)
- `crates/components/camel-rabbitmq/tests/common/mod.rs` (modified — `start_on_port`)
- `crates/components/camel-rabbitmq/tests/consumer_readiness.rs` (new)

**Steps:**
1. `src/consumer.rs`: `pub(crate) struct InboundDelivery { pub payload: Vec<u8>, pub props: BasicProperties, pub headers: FieldTable, pub redelivered: bool, pub acker: Box<dyn DeliveryAcker> }` and `#[async_trait] pub(crate) trait DeliveryAcker: Send + Sync { async fn ack(&self) -> Result<(), CamelError>; async fn nack(&self, requeue: bool) -> Result<(), CamelError>; }` implemented for `lapin::acker::Acker` (unit-test fakes implement the same trait). A lapin-delivery→InboundDelivery adapter converts each `Delivery`.
2. Consumer implements the component-api `Consumer` trait (camel-jms `consumer.rs` shape) with `startup_mode() == ConsumerStartupMode::Explicit` (`camel-component-api/src/consumer.rs:46-60`): start connects via `manager.connection_within(start_bound)`, creates the channel, THEN `ctx.mark_ready()`; start errors call `ctx.mark_failed` (used by 3.3/3.4 topology failures). No custom ConsumerState watch — the Explicit startup protocol IS the readiness gate.
3. Consume engine `pub(crate) async fn run_loop<S: Stream<Item = InboundDelivery>>(stream, ctx, disposition_cfg, manager_generation_channel_pair, cancel)` — generic over the delivery stream so unit tests drive it with a fake stream. Body mapping in THIS TASK is minimal (payload bytes only); task 2.4's `headers.rs` mapping supersedes it. Each delivery → `send_and_wait` into the route; disposition per task 2.2.
4. Stop: cancel the engine token, drain in-flight route futures with the component-api stop deadline, join the loop task. No `#[ignore]` markers; no test sleeps (channels/watch only).
5. `component.rs`: `create_consumer(rt)` now returns the real consumer when the endpoint config has `queue`; without `queue` it errors naming `queue` (a consumer needs an explicit queue). Metadata descriptor gains the `consumer` capability flag; parity test asserts producer+consumer from now on.

**Tests:**
- `consumer_stop_joins_loop` (unit): `run_loop` over a fake `InboundDelivery` stream with a blocked route future → stop() → loop task `is_finished()` within deadline
- `create_consumer_without_queue_errors` (unit): endpoint from `rabbitmq:ex?routingKey=rk` → create_consumer Err naming `queue`
- `metadata_capabilities_gain_consumer` (unit): descriptor flags producer+consumer true
- `consumer_not_ready_until_connected` (docker, `tests/consumer_readiness.rs`): reserve a free port; consumer with reconnect policy initial_delay 50 ms pointed at that port; `StartupSignal::pair()` + `ctx.with_startup(signal)`; `timeout(500 ms, rx.await_ready())` must be Err (not ready while broker absent); `RabbitFixture::start_on_port(port)`; `timeout(30 s, rx.await_ready())` must be Ok (gates "consumer not ready until connected"; also gates the docker half of "retry policy owns reconnect")
- command: `cargo test -p camel-component-rabbitmq --lib && RABBITMQ_ITEST=1 cargo test -p camel-component-rabbitmq --test consumer_readiness`
- expected: pass

**Acceptance:**
- lib + docker readiness green; clippy/fmt clean; lint-unbounded-wait + lint-cancel-tokens clean

**Master ledger (task 2.1, 2026-10-09):**
- `run_loop` ships with the 3-arg signature `(stream, ctx, cancel)`; task 2.2 grows it with the disposition config and task 2.5 with the generation/channel pair, so no unused parameters are threaded early.

- [x] 2.1

### crates/components/camel-rabbitmq (disposition)

#### Task 2.2: Ack-after-route and failure disposition with requeueOnFailure option

**Files:**
- `crates/components/camel-rabbitmq/src/consumer.rs` (modified)
- `crates/components/camel-rabbitmq/src/config.rs` (modified)
- `crates/components/camel-rabbitmq/src/metadata.rs` (modified)
- `crates/components/camel-rabbitmq/tests/consume_delivery.rs` (new)

**Steps:**
1. Extract `pub(crate) fn disposition(route_result: &Result<(), CamelError>, requeue_on_failure: bool) -> Disposition` with `enum Disposition { Ack, Nack { requeue: bool } }`: Ok → Ack; Err → Nack { requeue: requeue_on_failure } (default false). Applied via `pub(crate) async fn apply_disposition(d: Disposition, generation_matches: bool, acker: &dyn DeliveryAcker)` — generation mismatch drops the ack/nack (stale-tag logic wired in 2.5; the seam exists from here).
2. Consumer applies disposition AFTER `send_and_wait` resolves (never before — kafka/mqtt contract).
3. Add option `requeueOnFailure` (bool, default false) to `RabbitEndpointConfig::from_uri` + descriptor + `P2_OPTIONS` const; extend the parity test union.
4. Option doc comment: warn about hot requeue loops without a broker-side delivery limit (renders in README task 5.1).
5. Docker-test harness shape (used by all consume_delivery tests): build a real `ConsumerContext` via `ConsumerContext::new(tx, token, route_id)` (component-api shape); the test answers each `ExchangeEnvelope.reply_tx` with Ok or Err; holding an envelope un-answered = a blocked route (no sleeps).

**Tests:**
- `disposition_ok_acks`: Ok → Disposition::Ack
- `disposition_err_default_rejects_without_requeue`: Err, false → Nack { requeue: false }
- `disposition_err_opt_in_requeues`: Err, true → Nack { requeue: true }
- `apply_disposition_stale_generation_drops` (unit): recording fake acker + `generation_matches=false` → acker receives NOTHING, no error
- `metadata_uri_options_parity_p2_partial`: names == P1 ∪ {requeueOnFailure}
- `ack_only_after_send_and_wait` (docker, consume_delivery.rs): fixture queue with one message; envelope held un-answered (route blocked mid-flight); while blocked, cancel/close the consumer → broker requeues → raw lapin `basic_get(no_ack=false)` returns Some with `redelivered=true` (requeue-probe proves the message was still unacked mid-route — passive declare's message_count excludes unacked and basic_get-None is ambiguous); `basic_nack(requeue=true)` the probe delivery; answer the held envelope Ok; restart consumer → the redelivered message is acked after completion; no third delivery appears (gates "ack follows route completion")
- `failed_route_rejects_to_dlx` (docker): queue declared with `x-dead-letter-exchange` to a DLX+DLQ; envelope answered Err; default options → message lands on DLQ (`basic_get` on DLQ returns it) (gates "failed route rejects to DLX")
- `requeue_opt_in_redelivers` (docker): `requeueOnFailure=true`, envelope answered Err → a second envelope for the same payload arrives (2.4 adds the `rabbitmq.redelivered=true` header assert to this test) (gates "requeue opt-in")
- command: `cargo test -p camel-component-rabbitmq --lib && RABBITMQ_ITEST=1 cargo test -p camel-component-rabbitmq --test consume_delivery`
- expected: pass

**Acceptance:**
- Both tiers green; clippy/fmt clean

**Master ledger (task 2.2, 2026-10-09):**
- Continuation amendment: route transport failure review fix (blocking review, not a fresh spec bless or explicit new-scope ruling). `send_and_wait`'s `Err(CamelError::ChannelClosed)` — the route mpsc receiver is gone or the reply oneshot was dropped — is a transport/lifecycle loss, NOT a business route failure. `run_loop` now recognizes it BEFORE disposition, abandons the delivery with NO ack/nack, and exits. An unexpected exit closes only this consumer's own channel (a clone owned by the engine) so the broker requeues the still-unacked delivery and cancels the consumer registration; the shared connection is never reset. The normal stop path still owns its close (the engine skips cleanup when it exits via the cancel arm), so no double close on cancel. Business `Err` still rejects without requeue by default; a successful route still acks; cancellation abandonment is unchanged.
- Regression added: `route_transport_closed_abandons_delivery_without_disposition` and `route_receiver_closed_abandons_delivery_without_disposition` (unit, generic fake stream + recording acker, both transport paths) and `route_transport_loss_requeues_unacked_message` (docker: real broker redelivery + shared connection health). The declared at-least-once contract is now verified across the route transport-loss path.
- Recorded gap (NOT in scope): the pure `disposition` fn does not parse `Err` variants, and `run_loop` intercepts `ChannelClosed` before `disposition`; only the transport-recognition path is exercised, not a general error-classification routine.

- [x] 2.2

### crates/components/camel-rabbitmq (prefetch/concurrency)

#### Task 2.3: Prefetch and concurrentConsumers

**Files:**
- `crates/components/camel-rabbitmq/src/consumer.rs` (modified)
- `crates/components/camel-rabbitmq/src/connection.rs` (modified — `consumer_channel()` + `current_generation()`)
- `crates/components/camel-rabbitmq/src/config.rs` (modified)
- `crates/components/camel-rabbitmq/src/metadata.rs` (modified)
- `crates/components/camel-rabbitmq/tests/consume_delivery.rs` (modified)

**Steps:**
1. Options `prefetch` (u16, default 10) and `concurrentConsumers` (u32, default 1) in `from_uri` + descriptor + consts; `prefetch == 0` and `concurrentConsumers == 0` fail `from_uri` naming the option.
2. `connection.rs`: `pub async fn consumer_channel(&self) -> Result<(lapin::Channel, u64), CamelError>` (channel + generation) and `pub fn current_generation(&self) -> u64` (2.5 stale-tag guard reads it).
3. Consumer start spawns `concurrentConsumers` engine tasks, each with its OWN channel from `consumer_channel()`, each `basic_qos(prefetch, BasicQosOptions::default())` then its own `basic_consume`.
4. Readiness = every engine marked ready (count-gated: mark_ready fires after the last channel is up).

**Tests:**
- `config_rejects_zero_prefetch`: `from_uri("rabbitmq:default?queue=q&prefetch=0")` → Err naming `prefetch`
- `config_rejects_zero_concurrent_consumers`: same for `concurrentConsumers=0`
- `metadata_parity` extension: P1 ∪ {requeueOnFailure, prefetch, concurrentConsumers}
- `concurrent_consumers_register_on_queue` (docker): `concurrentConsumers=3&prefetch=5`; queue PRELOADED WITH 20 messages; routes blocked (envelopes held); raw lapin passive declare asserts `consumer_count == 3` AND `message_count == 5` (3×5=15 in flight, 5 ready — ignoring `prefetch` would leave 10+ ready, so the option is non-vacuous) (gates "concurrent consumers register on the queue")
- command: `cargo test -p camel-component-rabbitmq --lib && RABBITMQ_ITEST=1 cargo test -p camel-component-rabbitmq --test consume_delivery`
- expected: pass

**Acceptance:**
- Both tiers green; clippy/fmt clean

**Master ledger (task 2.3, 2026-10-09):**
- Connection seam: `RabbitConnectionManager::consumer_channel()` reads the connection `Arc` and its `generation` together under one read lock (no torn pair for the task 2.5 stale-tag guard) and bounds `channel.open` by a private `CHANNEL_OPEN_BOUND` (10 s), following the manager's bounded-wait convention. `current_generation()` is the read accessor; a `try_read` miss returns `0`, which can only ever yield a *stale* verdict (drop the disposition, broker redelivers — at-least-once safe). `publisher_channel` was not resurrected; only this consumer seam returns the pair.
- Consumer: `start()` keeps the bounded connect (`connection_within(CONSUMER_START_BOUND)`), then sets up `concurrentConsumers` engines sequentially, each on its OWN channel from `consumer_channel()`, calling `basic_qos(prefetch)` BEFORE its `basic_consume`. `mark_ready()` fires once, after the last `basic_consume` Ok (count-gated readiness). A failure at engine N closes the channels already opened under ONE aggregate `CONSUMER_STOP_BOUND` (never a per-channel multiplier), calls `mark_failed`, and stores no start state (start-guard semantics preserved, restartable). No shared-connection close and no parent-token cancel on this path.
- Stop/ownership: one child cancel token per engine; `stop()` cancels all children, joins ALL engines under a single aggregate 10 s bound (`futures::future::join_all` + abort-on-timeout), THEN closes the consumer's own channels (also one aggregate bound). It never closes the shared connection. Engine ownership is unchanged from 2.2: `run_loop` still intercepts `CamelError::ChannelClosed` before disposition (no ack/nack), business `Err` still nacks with `requeueOnFailure` (default reject), and each engine closes only its own channel on an unexpected exit.
- Tests (exact, no extra design): `config_rejects_zero_prefetch`, `config_rejects_zero_concurrent_consumers` (config.rs); `metadata_uri_options_parity_p2_partial` extended to P1 ∪ {requeueOnFailure, prefetch, concurrentConsumers} with `prefetch=10`/`concurrentConsumers=1` default asserts (no P3 phantom options); `concurrent_consumers_register_on_queue` (docker: 20 preloaded, `concurrentConsumers=3&prefetch=5`, routes held, bounded passive-declare poll for `consumer_count==3` and `message_count==5`). The engines stay serial (one in-flight route each); lapin buffers the remaining prefetch deliveries, so no extra futures concurrency was added.
- Recorded gap (NOT fixed): the two config zero tests are vacuous in the red phase — the pre-existing fail-closed unknown-param rejection already names `prefetch`/`concurrentConsumers`, so `message.contains(...)` passed before the options existed. The genuine red signals were the metadata parity test (`prefetch` not found) and the docker test's `from_uri` parse failure. A non-vacuous zero test would need a valid-case parse assert, which the task's exact test list does not include; no extra test was invented.
- Evidence: red lib `48 passed; 1 failed` (metadata parity); green lib `49 passed; 0 failed`. Docker: consume_delivery `5 passed`, consumer_readiness `1 passed`, publish_roundtrip `4 passed`; no leftover `rmq-itest-*` containers. clippy `-p camel-component-rabbitmq --all-targets --all-features -D warnings` clean; `cargo fmt --check --all` clean. Ratchets unchanged: lint-cancel-tokens `7 = max 7`, lint-test-sleep `471 = max 471`, lint-unbounded-wait `295 < max 296`; lint-log-redaction/lint-log-levels/lint-unwrap/lint-secrets 0 violations; `schema --check` OK.

**Narrow review fix (r_gpt BLOCK, 2026-10-09) — `join_engines` mixed-completion timeout:**
- Defect: `join_engines` wrapped `futures::future::join_all(handles.iter_mut())` in one timeout, then on timeout aborted and re-`await`ed EVERY handle. `join_all` polls each `&mut JoinHandle`; an engine that completed before the timeout was already polled to `Ready`, so re-awaiting it panicked `JoinHandle polled after completion` — stop could abort before closing own channels / clearing `cancel_token`, violating "all-own engine close + start clear guaranteed after Err joins".
- Fix: `join_engines` now drives an owned `FuturesUnordered<JoinHandle<()>>` under the same single `CONSUMER_STOP_BOUND`. Each engine is removed from the outstanding set the moment it resolves, so the timeout branch aborts/drains ONLY still-outstanding handles via `pending.iter()` + a drain `while pending.next().await.is_some() {}`; no `JoinHandle` is ever re-polled. No orphan handles (owned by the set, drained) and no lost tasks. `stop()`'s post-join `close_owned_channels` + `cancel_token = None` already run unconditionally after the `Err`, so cleanup/restart is guaranteed.
- Regression (exact design, no broader framework): `join_engines_mixed_completion_timeout_does_not_panic` (`#[tokio::test(start_paused = true)]`, existing workspace `tokio` `test-util`): one already-completed engine + one `std::future::pending` engine with a `DropFlag`; asserts the call returns `Err(CamelError::ProcessorError)` containing `did not stop within` and that the stalled engine future was dropped. Virtual time — no real 10 s sleep, no new dependency.
- RED demonstrated against the pre-fix helper: `JoinHandle polled after completion` panic (`target/logs/task2.3-stopfix-red-lib.log`). Post-fix lib `50 passed; 0 failed` (`task2.3-stopfix-green-lib.log`); Docker consume/readiness/publish `5/1/4 passed` (`task2.3-stopfix-docker.log`); fmt/clippy/ratchets/schema clean.
- Tautology removed: `config_rejects_zero_prefetch` / `config_rejects_zero_concurrent_consumers` now also require the real invalid-value diagnostic `must be greater than 0`, so the unknown-option error no longer satisfies them (meaningful red without a pre-impl fake redo).

- [x] 2.3

### crates/components/camel-rabbitmq (headers)

#### Task 2.4: Header and property mapping module

**Files:**
- `crates/components/camel-rabbitmq/src/headers.rs` (new)
- `crates/components/camel-rabbitmq/src/consumer.rs` (modified)
- `crates/components/camel-rabbitmq/src/producer.rs` (modified)
- `crates/components/camel-rabbitmq/tests/publish_roundtrip.rs` (modified)
- `crates/components/camel-rabbitmq/tests/consume_delivery.rs` (modified)

**Steps:**
1. `pub(crate) fn inbound(headers: &FieldTable, props: &BasicProperties, redelivered: bool) -> camel_api::Headers` mapping `contentType`, `contentEncoding`, `priority`, `messageId`, `correlationId`, `replyTo`, `expiration`, `timestamp` from BasicProperties + free-form AMQP headers verbatim + `rabbitmq.redelivered` (bool).
2. `pub(crate) fn outbound(headers: &camel_api::Headers) -> (BasicProperties, FieldTable)` — the single two-way source: reserved names map back onto BasicProperties; everything else rides the FieldTable as LongString. Producer's `build_properties` from 1.4 DELEGATES to this (delete the 1.4 inline FieldTable code).
3. Timestamps: lapin 4.12 `BasicProperties.timestamp` is `Option<Timestamp>` where `Timestamp = LongLongUInt = u64` (AMQP Unix SECONDS, verified in `amq-protocol-types 10.6.3`), NOT chrono-typed. Camel headers carry epoch-millis `i64`; the jms precedent (`camel-jms/src/headers.rs:17-20`) supplies only the units (millis) — jms stores the value as a JSON string, so it is NOT the same representation. Convert inbound seconds→millis with a checked multiply (out-of-range drops the header) and outbound millis→seconds as nonnegative integer floor division. No `chrono` dependency is needed; the original `chrono.workspace = true` note was a type/units mismatch.
4. Docker round-trip test (publish_roundtrip.rs): extend `publish_then_basic_get_round_trip` — after basic_get, run one consume cycle through the consumer and assert `x-custom` survives and `rabbitmq.redelivered` is present (full round-trip gate); consume_delivery.rs `requeue_opt_in_redelivers` gains the `rabbitmq.redelivered=true` header assert.

**Tests:**
- `inbound_maps_redelivered_flag`: redelivered=true → header `rabbitmq.redelivered=true` (gates "redelivered flag maps to header")
- `inbound_maps_reserved_properties`: props with content_type/message_id/correlation_id set → matching headers
- `inbound_maps_timestamp_as_millis`: AMQP `u64` Unix-seconds timestamp → i64 epoch-millis header (checked overflow)
- `outbound_round_trips_free_form`: header `x-custom=abc` → FieldTable carries it; `inbound` restores it (gates "free-form header round trips")
- `header_round_trip_docker` (docker): full producer→queue→consumer round trip with `x-custom` + redelivered asserts
- command: `cargo test -p camel-component-rabbitmq --lib && RABBITMQ_ITEST=1 cargo test -p camel-component-rabbitmq --test publish_roundtrip`
- expected: pass

**Acceptance:**
- Both tiers green; no property mapping duplicated outside headers.rs; clippy/fmt clean

**Master ledger (task 2.4, 2026-10-09):**
- Implementation is the blessed task 2.4 scope; no fresh spec bless claimed. `src/headers.rs` is the single two-way mapping: the eight reserved names (`contentType`, `contentEncoding`, `priority`, `messageId`, `correlationId`, `replyTo`, `expiration`, `timestamp`) map onto `BasicProperties` in both directions, every other header rides the free-form `FieldTable` as `LongString`, and inbound adds `rabbitmq.redelivered` (bool). Reserved precedence holds both ways (free-form reserved names skipped inbound; reserved names excluded from the table outbound), so it cannot be bypassed via free-form. Malformed reserved values (`priority` > u8, negative `timestamp`, >255-byte `ShortString`) drop without panic.
- Protocol discrepancy (plan step 3 corrected): lapin 4.12 `BasicProperties.timestamp` is `Option<u64>` (`amq-protocol-types 10.6.3` `type Timestamp = LongLongUInt = u64`), AMQP Unix SECONDS, NOT a chrono type. `chrono` was NOT added and `Cargo.toml` is unchanged. Inbound converts `u64` seconds → `i64` epoch millis via `i64::try_from(...).checked_mul(1000)` (out-of-range dropped); outbound converts `i64` millis → `u64` seconds as nonnegative integer floor division (`millis / 1000`, negatives dropped). The jms precedent supplies only the millis units; it stores a JSON string, so the representation differs.
- Producer `build_properties` delegates to `headers::outbound` (inline FieldTable mapping removed); URI `contentType` still overrides the header with the header as fallback, and `delivery_mode` is 2/1 by `persistent`. Consumer `build_exchange` maps `headers::inbound` onto the routed exchange.
- Tests: `inbound_maps_redelivered_flag`, `inbound_maps_reserved_properties`, `inbound_maps_timestamp_as_millis`, `outbound_round_trips_free_form` (unit, `src/headers.rs`); `header_round_trip_docker` added and `publish_then_basic_get_round_trip` extended with one consume cycle; `requeue_opt_in_redelivers` asserts `rabbitmq.redelivered=true`.
- Evidence (actual counts, `target/logs/`): red-first stubbed mapping `0 passed; 4 failed` (`task2.4-lib-red.log`); lib `54 passed; 0 failed` (`task2.4-lib-final.log`); docker `publish_roundtrip 5 passed` (`task2.4-docker-publish.log`), `consume_delivery 5 passed` (`task2.4-docker-consume.log`), `consumer_readiness 1 passed` (`task2.4-docker-readiness.log`); ungated notice path `5+5 passed`, exit 0 (`task2.4-ungated.log`); clippy `--all-targets --all-features -D warnings` exit 0 (`task2.4-clippy-postfmt.log`); fresh `cargo fmt --check --all` exit 0 (`task2.4-fmt-final.log`); ratios lint-test-sleep 471 = max 471, lint-cancel-tokens 7 = max 7, lint-unbounded-wait 295 < max 296; lint-unwrap/secrets/log-redaction/log-levels/schema exit 0 (`task2.4-xtask-lints.log`). No leftover `rmq-itest-*` containers. README's stale phase-1 mapping limitation was replaced.
- Note: `target/logs/task2.4-fmt.log` recorded the pre-format failure (exit 1, 51 diff lines) and is preserved; `target/logs/task2.4-fmt-final.log` is the post-format exit-0 capture with CMD/UTC/df/EXIT.

- [x] 2.4

### crates/components/camel-rabbitmq (reconnect safety)

#### Task 2.5: Stale delivery-tag suppression and redelivery after mid-flight stop

**Files:**
- `crates/components/camel-rabbitmq/src/consumer.rs` (modified)
- `crates/components/camel-rabbitmq/tests/common/mod.rs` (modified — `restart()`)
- `crates/components/camel-rabbitmq/tests/reconnect.rs` (new)

**Steps:**
1. Each engine captures generation `g` from `consumer_channel()`; `apply_disposition(d, manager.current_generation() == g, acker)` drops stale tags (debug log).
2. When a delivery stream ends (channel death/broker drop), the engine calls `manager.note_failure()` then waits — cancel-aware — for `ConnStatus::Connected` before taking a new channel+generation and re-consuming.
3. `tests/reconnect.rs` docker tests: (a) `stale_tag_after_broker_restart` — consumer with reconnect initial_delay 50 ms, one held envelope mid-route; `fx.restart()` (fixture helper: `docker restart <id>`); answer the held envelope Ok AFTER reconnect → no protocol error (no channel-close from double-ack), and the message was redelivered by the broker (a fresh envelope for the same payload arrives on the reconnected consumer) (gates "stale tag after broker restart is dropped" + the docker half of "retry policy owns reconnect"); (b) `redelivery_after_mid_flight_stop` — consumer stopped (2.1 stop path) with one held envelope → restart consumer → the same payload is delivered and processed again (gates "redelivery after mid-flight stop"); (c) `generation_increments_after_broker_restart` — capture `manager.current_generation()` before `fx.restart()`, wait for the reconnected consumer to take a fresh channel, then assert `current_generation() == before + 1` (public accessor from 2.3; this is the docker assertion replacing the impossible 1.3 unit fake, since a fake cannot construct `lapin::Connection`).
4. Module doc comment on consumer.rs documents the at-least-once consequence (dropped stale tag ⇒ broker redelivers ⇒ consumers tolerate duplicates).

**Tests:**
- `apply_disposition_stale_generation_drops` (already landed in 2.2 — re-run here as regression)
- `stale_tag_after_broker_restart` (docker, gates scenario)
- `redelivery_after_mid_flight_stop` (docker, gates scenario)
- `generation_increments_after_broker_restart` (docker, `current_generation() == before + 1` across `fx.restart()`; via the public accessor)
- command: `cargo test -p camel-component-rabbitmq --lib && RABBITMQ_ITEST=1 cargo test -p camel-component-rabbitmq --test reconnect`
- expected: pass

**Acceptance:**
- Both tiers green; no lapin channel-close error observed in the docker tests (assert via captured output)

**Worker ledger (task 2.5, 2026-10-09):**
- Implementation: `run_loop` takes the engine's connection generation plus the manager and returns an `EngineExit` (`Cancelled` / `StreamEnded` / `RouteTransportLoss`); a disposition is applied only while `manager.current_generation() == generation`, otherwise the stale tag is dropped with a debug marker (at-least-once). A new `run_engine` wraps `run_loop`: on `StreamEnded` it demotes the connection only when its generation is still current (a manager that already reconnected is never torn down — no extra generation increment), waits cancel-aware for `Connected` (looping `connection_within` under `RECONNECT_WAIT_BOUND`, re-arming a settled manager), then re-consumes on a fresh channel/generation. `ChannelClosed` (`RouteTransportLoss`) still abandons without a disposition, closes only the engine's own channel, and terminates without touching the shared connection. The first channel stays owned by `stop()`; a reconnected channel is engine-owned and released on exit. No manager change was needed: the existing `current_generation()` accessor plus the `open_engine_channel`-failure demotion cover the try-read-`0` conservative case without a permanent stall. No new root cancellation token; lapin auto-recovery stays off.
- Tests (exact): `apply_disposition_stale_generation_drops` re-run (lib); `tests/reconnect.rs` — `stale_tag_after_broker_restart`, `redelivery_after_mid_flight_stop`, `generation_increments_after_broker_restart`. Fixture gains bounded `restart()` (`docker restart <id>`, no `--rm`, own-container only) and a process-wide log capture; the stale-tag test asserts the stale-drop marker was captured (non-vacuous), no `PRECONDITION_FAILED`/`UNKNOWN_DELIVERY_TAG` was logged, the redelivery carries `rabbitmq.redelivered=true`, and a fresh message is consumed on the same reconnected connection.
- RED (real): `task2.5-red-reconnect.log` — `1 passed; 2 failed` (`stale_tag_after_broker_restart`, `generation_increments_after_broker_restart`; the engine terminated instead of reconnecting; `redelivery_after_mid_flight_stop` is the 2.1/2.2 regression and passed). GREEN: lib `54 passed; 0 failed` (`task2.5-lib-green.log`); reconnect `3 passed` (`task2.5-docker-reconnect-green2.log`), stable `3 passed` x2 (`task2.5-docker-reconnect-stability.log`); consume `5 passed`, publish `5 passed`, readiness `1 passed` (`task2.5-docker-*.log`); ungated notice path exit 0 (`task2.5-ungated.log`); no leftover `rmq-itest-*`.
- Gates: clippy `-p camel-component-rabbitmq --all-targets --all-features -D warnings` exit 0 (post-fmt `task2.5-postfmt-clippy-lib.log`); `cargo fmt --check --all` exit 0 (`task2.5-fmt-final.log`); lint-cancel-tokens `7 = max 7`, lint-test-sleep `471 = max 471`, lint-unbounded-wait `295 < max 296`, lint-log-redaction/levels/unwrap/secrets `0`, `schema --check` OK (`task2.5-xtask-lints.log`); extra lints OK except lint-publish-registration's expected Case B (`task2.5-xtask-lints-extra.log`).
- Lifecycle proof: `generation_increments_after_broker_restart` asserts exactly `before + 1` across a broker restart and `stale_tag_after_broker_restart` asserts the pre-restart tag is dropped with the redelivery arriving on the reconnected consumer — one generation bump, no stale ack.
- Gaps: none blocking. The manager's `current_generation()` try-read-miss returning `0` is conservative; the `open_engine_channel`-failure demotion prevents a permanent stall, so no atomic-generation-publication fix was required. The pre-existing slow-old-listener race (an old connection's error listener demoting a newer connection) lives in manager code outside this task's file scope and was not observed across three reconnect runs.

**Review-fix ledger (r_gpt BLOCK, task 2.5, 2026-10-09):**
- Race 1 fixed (late old-generation failure): `RabbitConnectionManager` gains a std `failure_flight` mutex serializing the failure fence (generation check + `Connected -> Disconnected`) against publication (`Connecting -> Connected` + generation store). `note_failure_for_generation(g)` fences inside the manager (no caller-side check-then-act); `note_failure()` stays as the unqualified external-force entry. The error listener is spawned with its connection's published generation and every report is qualified; the consumer `run_engine` stream-death uses the captured engine generation and the channel-re-open failure uses the attempted generation; the producer caches `(channel, generation)` and `mark_channel_dead` qualifies. The deferred clear only clears the slot (generation-guarded); it never flips status or starts a retry. No lock is held across an await: publication takes the state write lock first, then the std flight mutex; the failure path uses sync `try_write` and defers on contention.
- Race 2 fixed (busy-lock generation): `generation: AtomicU64` is the canonical lock-free snapshot; `current_generation()` loads it (`Acquire`) with no `0`-busy fallback. Publication stores it (`Release`) under the state write lock + flight mutex, and `consumer_channel` still reads the `(Arc<Connection>, generation)` pair under the read lock, so the pair stays consistent and the atomic never trails it. `0` is reserved for "no connection published yet".
- Exact amendment tests: `late_old_generation_failure_preserves_new_connection` and `current_generation_remains_authoritative_during_state_write_lock` (manager unit, private cfg(test) `seed_published_state` seam; no fake `lapin::Connection`, no public test hook). The disposition-under-lock test uses a crate-private `RecordingAcker` and the real `apply_disposition`.
- RED/GREEN: `task2.5-reviewfix-red-lib.log` — both new tests FAILED behaviorally (generation 0 under the write lock; successor demoted to Disconnected) against the stub; `task2.5-reviewfix-lib-green.log` lib `56 passed; 0 failed`. Docker `task2.5-reviewfix-docker-reconnect-final.log` reconnect `3 passed` (serialized via a bounded static `tokio::sync::Mutex`, so the capture window is attributable); consume `5`, publish `5`, readiness `1` (`task2.5-reviewfix-docker-*.log`); full ungated exit 0 (`task2.5-reviewfix-full-ungated.log`); no leftover `rmq-itest-*`.
- Capture attribution: `LogCapture::snapshot_len` + `count_containing_since` scope the stale-drop/protocol assertions to the test's own window; the stale-tag test now also asserts `Arc::ptr_eq` on the reconnected connection and an unchanged generation (strong same-connection proof).
- Test decomposition: consumer and connection inline `#[cfg(test)] mod tests` moved to `src/consumer/tests.rs` and `src/connection/tests.rs` (`#[cfg(test)] mod tests;`); `connection.rs` 1564 -> 733 lines, `consumer.rs` 1029 -> 761. `docker_fixture` stays in `connection.rs` (`use super::*` re-exports it).
- Gates: clippy `--all-targets --all-features -D warnings` exit 0 (`task2.5-reviewfix-postfmt-clippy-lib.log`); `cargo fmt --check --all` exit 0 (fresh post-review-fix capture `task2.5-reviewfix-fmt-final.log`, CMD/ENV/UTC/DF/EXIT recorded); lint-cancel-tokens `7 = max 7`, lint-test-sleep `471 = max 471`, lint-unbounded-wait `295 < max 296` (the 3 serialization-lock sites were bounded with `tokio::time::timeout`), log-redaction/levels/unwrap/secrets `0`, schema OK (`task2.5-reviewfix-xtask-lints2.log`); extra lints OK except the expected publish-registration Case B (`task2.5-reviewfix-xtask-lints-extra.log`).
- Review verdict: r_gpt **APPROVE** (all code); residual was only that the earlier fmt log predated the review-fix — closed by the fresh exit-0 `task2.5-reviewfix-fmt-final.log`. No code change was needed; checkbox stays unchecked.
- Safety proof: publication and failure demotion are mutually exclusive, so at any time `status == Connected` implies the canonical atomic equals the stored connection's generation, and a failure reported for any other generation is a no-op — a late old-connection failure cannot flip the successor's status, clear its slot, or start a replacement retry. `current_generation()` is a lock-free `Acquire` load of the value published with `Release` under the write lock, so it is authoritative under any read/write contention.

**Boundary-finding continuation ledger (P2, 2026-10-09):**
- Finding: the `run_engine` `StreamEnded` arm unconditionally called `note_failure_for_generation(engine_generation)`. A *channel-local* termination on a still-healthy shared connection — `basic.cancel` after a queue deletion, or a soft channel close — therefore demoted the live connection, issued generation g+1, and stranded every sibling engine still consuming on generation g: their acks were judged stale and dropped forever (prefetch deadlock). A 404 on the re-open path demoted the healthy connection again on every retry.
- Fix (local-only; no explicit connection replacement): the engine caches the ORIGIN connection its channel was opened on (`consumer_channel` now returns `(channel, generation, Arc<Connection>)` captured atomically under the read lock, so a later reconnected connection is never mistaken for the origin). On `StreamEnded`: a technical debug marker `"RabbitMQ consumer stream ended"` is logged; the superseded channel is closed (no registration leak; siblings and the shared connection untouched); only a DEAD origin is reported through the generation-fenced `note_failure_for_generation`, so `NetworkRetryPolicy` owns reconnection. A healthy origin re-opens on the SAME manager connection/generation without demotion; a soft re-open failure (404 queue absent) is retried under a bounded cancel-aware 100 ms backoff and never invalidates the shared connection. `RouteTransportLoss` still closes only the engine's own channel and terminates.
- Regression test (docker, live): `channel_local_cancel_preserves_sibling_consumption` in `tests/reconnect.rs` (serialized under `RECONNECT_TESTS`). Two queues, two `RabbitConsumer`s on ONE shared manager, each prefetch 1. Delete queue A → await the local-termination marker (`LogCapture::count_containing_since`) and assert the manager generation is unchanged; recreate A and prove it re-subscribes on the same generation; then run THREE messages through sibling B (beyond prefetch) — a generation bump would strand B's acks and stall delivery. Finally assert `Arc::ptr_eq` on the shared connection, an unchanged generation, and zero AMQP protocol errors.
- RED/GREEN (fresh local recovery): `phase2-local-recovery-red.log` — the test FAILED behaviorally at the generation assertion against base `1c49c838` (no marker; manager reconnected), `1 failed`; `phase2-local-recovery-green-reconnect.log` — reconnect `4 passed`; `phase2-local-recovery-lib.log` — lib `56 passed`; `phase2-local-recovery-docker-scope.log` — consume `6`, readiness `1`, publish `5`; `phase2-local-recovery-fmt-clippy.log` / `phase2-local-recovery-fmt.log` — fmt exit 0, clippy `-p camel-component-rabbitmq --all-targets --all-features -D warnings` exit 0; `phase2-local-recovery-lints.log` — cancel-tokens `7 = max 7`, test-sleep `471 = max 471`, unbounded-wait `295 < max 296`, log-redaction/levels/metric-labels/unwrap/secrets `0`, `schema --check` OK. No leftover `rmq-itest-*`.
- Scope: narrow continuation of tasks 2.5/2.6. Base head `1c49c838` remains the valid P2 baseline. This entry records the boundary finding and its fix; it does not re-bless the earlier phase (unchanged-workspace/CLI/bundle gates were not re-run).

**Review amendment (r_gpt REJECT, 2026-10-09):**
- Residual: the consumer `channel.open` failure branch still called the unqualified `note_failure_for_generation(manager.current_generation())` after the await. An OLD attempt that failed after a reconnect had published g+1 therefore re-labelled its failure with g+1; the fence saw a current generation and could not reject the mislabel, so a dead old channel-open tore down the live successor.
- Fix: `consumer_channel` now classifies a `channel.open` error/timeout INSIDE the manager against the `(connection, generation)` captured atomically BEFORE the await, through the private `handle_channel_open_failure(origin_generation, origin_connected)`; it demotes only when the captured origin is actually dead (`!origin_connected`) and never substitutes `current_generation()`. The consumer caller on `Err` only backs off cancel-aware (100 ms) and re-waits — no unqualified/current-generation demotion. An absent connection / `Connecting` manager never reaches the helper (early `disconnected_error`), so it only waits. A channel-open soft error on a healthy origin does not reconnect the shared connection.
- Regression (unit, deterministic): `late_old_channel_open_failure_preserves_new_generation` in `src/connection/tests.rs` — captures origin g1 while `Connected`, publishes successor g2 in the blocked-in-flight window, invokes the production handler with `(g1, dead)`, and asserts generation 2 / status `Connected` / retry driver 0 / slot intact.
- RED/GREEN: `phase2-open-origin-red.log` — the test FAILED behaviorally against the pre-fix handler body (`Connected` -> `Connecting`, successor demoted), test unchanged; `phase2-open-origin-lib.log` — lib `57 passed` (56 + 1); `phase2-open-origin-docker.log` — reconnect `4`, consume `6`, readiness `1`, publish `5`; `phase2-open-origin-fmt-clippy.log` — fmt exit 0, clippy `-p camel-component-rabbitmq --all-targets --all-features -D warnings` exit 0; `phase2-open-origin-lints.log` — cancel-tokens `7 = max 7`, test-sleep `471 = max 471`, unbounded-wait `295 < max 296`, log-redaction/levels/metric-labels `0`, `schema --check` OK. No leftover `rmq-itest-*`. Code grep: no originating-error `note_failure_for_generation(current_generation())` remains.
- Scope: review amendment to the P2 boundary-finding ledger. New post-fix head pending commit; base `1c49c838` unchanged and no large-workspace/CLI/bundle gate was re-run.

**P2 boundary acceptance (2026-10-09):**
- `r_gpt` returned **APPROVE**, with no findings, for `aebbf351...1c49c838` and the reviewed recovery amendments. Both shared-connection recovery blockers are resolved. All tasks 2.1–2.6 and the Phase 2 exit criteria are satisfied.
- Amendment verification: 57 library tests and 16 live broker tests pass; component clippy, workspace formatting, targeted lints, and schema pass. Baseline evidence at `1c49c838` includes workspace build, 16 bundle tests, 19 CLI feature-profile tests, and the boundary gate matrix. Baseline gates were not rerun for these amendments.
- `lint-publish-registration` retains expected Case B for the new unpublished crate. Full workspace clippy/tests, audit, broad documentation build, and remote commit lint remain unrun. This is Phase 2 acceptance, not final change completion.
- Park at the Phase 2 boundary. Phase 3 requires a master ruling.

- [x] 2.5

### crates/components/camel-rabbitmq (consume metrics)

#### Task 2.6: Consume outcome metrics and P2 parity freeze

**Files:**
- `crates/components/camel-rabbitmq/src/consumer.rs` (modified)
- `crates/components/camel-rabbitmq/src/metadata.rs` (modified)
- `crates/components/camel-rabbitmq/tests/consume_delivery.rs` (modified)

**Steps:**
1. Consumer engine emits `camel_component_operations_total{component="rabbitmq", operation="consume", outcome="success"}` when the route returns Ok (post-ack) and `outcome="failure"` when it errors — same RuntimeObservability emission as producer 1.4.
2. Freeze `P2_OPTIONS` = P1 ∪ {requeueOnFailure, prefetch, concurrentConsumers}; parity test asserts descriptor == P1 ∪ P2 exactly.

**Tests:**
- `consume_outcome_counted` (docker, consume_delivery.rs): envelopes answered one Ok + one Err → `RecordingRuntimeObservability` observes `("rabbitmq","consume","success")` once and `("rabbitmq","consume","failure")` once (gates "consume outcome counted")
- `metadata_parity_p2_frozen`: descriptor == P1 ∪ P2 exactly (gates "metadata parity holds at every phase exit" for P2)
- command: `RABBITMQ_ITEST=1 cargo test -p camel-component-rabbitmq --test consume_delivery && cargo test -p camel-component-rabbitmq --lib`
- expected: pass

**Acceptance:**
- Both tiers green; branch releasable: only P1+P2 options surfaced

**Worker ledger (task 2.6, 2026-10-09):**
- Implementation (consumer.rs): a new `ConsumeOutcome` (`Success`/`Failure`) plus `record_consume(rt, outcome)` emit `camel_component_operations_total{component="rabbitmq", operation="consume", outcome}` through the same `RuntimeObservability::component_metrics()` facade as producer 1.4. `run_engine`/`run_loop` now carry the consumer's existing injected `runtime: Arc<dyn RuntimeObservability>` (the `#[allow(dead_code)]` is gone; no production test hook was added). `run_engine` bundles `(channel, generation)` as one arg so the added `runtime` does not trip `clippy::too_many_arguments`.
- Semantics (post-disposition, not ghost route result): `apply_disposition` now returns `Result<DispositionApplied, CamelError>` (`Applied`/`Stale`) instead of `Result<()>`. The outcome is counted only when a disposition was actually written: route `Ok` + ack applied → `success`; route business `Err` + nack applied → `failure`. A stale tag (`Stale`) sends no ack/nack, `ChannelClosed` transport loss returns `EngineExit::RouteTransportLoss` before any disposition, and a failed ack/nack logs `warn` and claims neither outcome — so infrastructure events are never reported as business success/failure (distinct taxonomy). The exact spec case (one Ok route + one Err route) is exactly 1 success + 1 failure.
- Implementation (metadata.rs): P2 is frozen. `config::P2_OPTIONS` stays the P2 delta (`requeueOnFailure`, `prefetch`, `concurrentConsumers`) and is consumed as `P1_OPTIONS ∪ P2_OPTIONS` (the union formulation from the spec step, with no duplicated constant strings); the descriptor is `#[uri_scheme]` unchanged. The old partial test was renamed 1:1 `metadata_uri_options_parity_p2_partial` → `metadata_parity_p2_frozen` (lib count stays 56), still asserting the exact `P1 ∪ P2` name set plus the defaults (`persistent=true`, `requeueOnFailure=false`, `prefetch=10`, `concurrentConsumers=1`) and both capabilities. No P3+ option is surfaced.
- Test (consume_delivery.rs): `consume_outcome_counted` (docker) attaches a real `RecordingRuntimeObservability` to the real `RabbitConsumer` via `RabbitConsumer::new` (the shared `start_consumer` helper gained an `rt` parameter; the five existing callers pass a no-op). It publishes two envelopes, answers one `Ok` and one `Err`, then bounded-polls (`timeout` + `yield_now`, no settling sleeps) until two ops are observed and asserts exactly one `("rabbitmq","consume","success")` and one `("rabbitmq","consume","failure")` with no extra ops.
- Test-infra fix (tests/common/mod.rs): the fixture container name was `rmq-itest-{pid}-{nanos()}`; two concurrent tests can read the same `nanos()` tick, and the collision path's `docker rm -f <name>` then removed the sibling test's broker. Appended a monotonic per-process `fixture_seq()` so names are unique within a process. Test-only; no product surface.
- RED (real behavior): `task2.6-red-consume-outcome.log` — the unmodified engine recorded no ops, so `consume_outcome_counted` failed by bounded-wait elapsed (`0 passed; 1 failed`). GREEN: lib `56 passed; 0 failed` (`task2.6-lib-final.log`, includes renamed `metadata_parity_p2_frozen`); docker consume `6 passed` (`task2.6-docker-consume.log`), readiness `1 passed` (`task2.6-docker-consumer_readiness.log`), publish `5 passed` (`task2.6-docker-publish_roundtrip.log`), reconnect `3 passed` (`task2.6-docker-reconnect.log`); no leftover `rmq-itest-*` containers.
- Gates: clippy `-p camel-component-rabbitmq --all-targets --all-features -D warnings` exit 0 (`task2.6-clippy.log`); fresh `cargo fmt --check --all` exit 0 on the final tree (`task2.6-fmt-final.log`, CMD/ENV/UTC/DF/EXIT); lint-metric-labels `0`, lint-log-redaction `0`, lint-log-levels `0` (strict), lint-unwrap `0`, lint-secrets `0`, `schema --check` OK, lint-test-sleep `471 = max 471`, lint-cancel-tokens `7 = max 7`, lint-unbounded-wait `295 < max 296` (`task2.6-xtask-lints.log`).
- Scope: no new tests beyond the two named ones (the only unit-test touch is the stale-disposition assertion, updated to the new `DispositionApplied::Stale` return); checkbox stays unchecked.

- [x] 2.6

## Phase 3: Producer reliability + topology

### crates/components/camel-rabbitmq (confirms)

#### Task 3.1: confirmTimeout option surfaces the confirm bound

**Files:**
- `crates/components/camel-rabbitmq/src/producer.rs` (modified)
- `crates/components/camel-rabbitmq/src/config.rs` (modified)
- `crates/components/camel-rabbitmq/src/metadata.rs` (modified)
- `crates/components/camel-rabbitmq/tests/publisher_confirms.rs` (new)
- `crates/components/camel-rabbitmq/tests/common/mod.rs` (modified — `pause()`/`unpause()`)

**Steps:**
1. Option `confirmTimeout` (u64 ms, default 5000) in from_uri + descriptor + `P3_OPTIONS` partial const. Producer uses the configured value in place of `DEFAULT_CONFIRM_TIMEOUT` (confirms themselves already run since 1.4).
2. Confirm-nack mapping: a broker nack on the publish confirm fails the exchange (RabbitError lands in 3.5; until then a `CamelError::ProcessorError` carrying exchange+routing key) — failure metric + `note_failure` on channel death.
3. `tests/publisher_confirms.rs`: `confirm_timeout_fails_exchange` — fixture with `confirmTimeout=1000` (ms unit per spec); `fx.pause()` the container (docker pause — connection stalls, confirms never arrive) → publish returns Err within ~2 s; message names the exchange and `confirm` (gates "confirm timeout fails the exchange"); `fx.unpause()` in teardown; `confirm_success_fast_path` — normal publish resolves promptly and the message is basic_get-able.

**Tests:**
- `confirm_error_names_target` (unit): the confirm-error constructor maps `(exchange, routing_key)` into the message
- `confirm_timeout_fails_exchange` (docker, gates scenario)
- `confirm_success_fast_path` (docker)
- command: `cargo test -p camel-component-rabbitmq --lib && RABBITMQ_ITEST=1 cargo test -p camel-component-rabbitmq --test publisher_confirms`
- expected: pass

**Acceptance:**
- Both tiers green; clippy/fmt clean

**Worker ledger (task 3.1, 2026-10-09):**
- Implementation (config.rs): new `confirmTimeout` (u64 ms, default `5000`) parsed from `from_uri` into `RabbitEndpointConfig::confirm_timeout: Duration` (invalid stays an `InvalidUri` naming the parameter). `P3_OPTIONS` partial const = `["confirmTimeout"]` and `is_known_option` unions it; the producer's old `pub(crate) const DEFAULT_CONFIRM_TIMEOUT` is gone, and the default now lives in `config::DEFAULT_CONFIRM_TIMEOUT_MS`.
- Implementation (producer.rs): `publish` awaits the broker verdict under `config.confirm_timeout` instead of the fixed 5 s. A new `confirm_error(target, reason)` constructor names `(exchange, routing_key)`; the Nack arm uses it (`reason = "was nacked by the broker"`) and the timeout arm uses it (`reason = "timed out after <bound> awaiting the broker confirm"`). G-1 honored: no bare `note_failure`; only the existing generation-qualified `mark_channel_dead` (which additionally requires `is_connection_level_error`) runs on real channel/connection error paths. A nack leaves the channel alive; a confirm timeout invalidates the cached channel only when it is still the exact origin (`channel.id()` + generation) this publish used, then best-effort closes it under a bounded `CHANNEL_CLOSE_BOUND` (1 s) — a concurrent healthy replacement survives, and the next publish recreates a fresh confirm channel so a late confirm cannot be mis-associated. The timeout does NOT touch the manager, so the healthy shared connection is never reset.
- Implementation (metadata.rs): descriptor gains `confirmTimeout` (`default = "5000"`). The old `metadata_parity_p2_frozen` is renamed to `metadata_parity_p3_partial` and asserts `P1 ∪ P2 ∪ P3` exactly plus the defaults (`persistent=true`, `requeueOnFailure=false`, `prefetch=10`, `concurrentConsumers=1`, `confirmTimeout=5000`) and both capabilities. It is deliberately NOT the P3 freeze (task 3.5 replaces it with `metadata_parity_p3_frozen`), and no non-3.1 option is surfaced.
- Test infra (tests/common/mod.rs): `RabbitFixture::pause()`/`unpause()` run bounded (`DOCKER_CMD_TIMEOUT`) `docker pause`/`unpause`; a failure is `infra-unavailable`. `Drop` now best-effort unpauses before `docker rm -f` (a frozen container can refuse removal) and stays bounded, so teardown releases the freeze even when a test panicked mid-pause. No product surface.
- Test (tests/publisher_confirms.rs, new): `confirm_timeout_fails_exchange` publishes a warmup message first so the lazily-allocated confirm channel exists BEFORE `fx.pause()` (otherwise the second publish would fail on the disconnected-connection wait, not the confirm bound); then the paused second publish returns Err with elapsed measured, `fx.unpause()` releases the freeze before the asserts, and `Drop` retries best-effort. Message must contain `exchange`, `confirm`, and the routing-key target; elapsed `>= 900 ms` and `< 3 s` (observed ~2 s = 1000 ms confirm bound + 1 s bounded close). `confirm_success_fast_path` resolves promptly and is read back with raw `basic_get`. No public production test hook.
- Test (producer.rs unit): `confirm_error_names_target` maps `("orders.exchange","rk-42")` into the message. No extra functions or tests were added beyond the three named ones (the metadata change is a rename plus a default assert, not a new fn).
- RED (real behavior): `task3.1-red-lib.log` — `confirm_error_names_target` failed (57 passed; 1 failed) against the non-naming stub; `task3.1-red-publisher_confirms.log` — both docker tests failed (0 passed; 2 failed) with `InvalidUri("unknown query parameter 'confirmTimeout'")`.
- GREEN: lib `58 passed; 0 failed` (`task3.1-lib-final.log`); full RabbitMQ docker tier `18 passed; 0 failed` (`task3.1-docker-full.log`): publish_roundtrip 5, consume_delivery 6, consumer_readiness 1, reconnect 4, publisher_confirms 2 (current 16 + 2 new). No leftover `rmq-itest-*` containers.
- Gates: clippy `-p camel-component-rabbitmq --all-targets --all-features -D warnings` exit 0 (`task3.1-clippy.log`); fresh `cargo fmt --check --all` exit 0 on the final tree (`task3.1-fmt-final.log`); xtask lints all OK (`task3.1-xtask-lints.log`) — lint-metric-labels 0, lint-log-redaction 0, lint-log-levels 0 (strict), lint-unwrap 0, lint-secrets 0, `schema --check` OK, lint-test-sleep `471 = max 471`, lint-cancel-tokens `7 = max 7`, lint-unbounded-wait `295 < max 296`. Disk stayed at 66 % on `/home/shared`.
- **Review fix (r_gpt, 2026-10-09): cache lock held across network + cache-identity race.** The first cut used `tokio::sync::Mutex<Option<(Channel, u64)>>` and held the guard across `connection_within` + an UNBOUNDED `create_channel`/`confirm_select`; the timeout path then did `*slot.lock().await = None`. On a stalled broker a fresh creation wedged the cache lock, so a second pending confirm timeout blocked forever on invalidation, and an old timeout could remove a newer concurrent creator's healthy channel. Fixed: `ChannelCache = std::sync::Mutex<Option<(Channel, u64)>>` (tiny synchronous snapshot/conditional-update sections; a `std` guard is never held across `.await`), `ensure_channel` clones the connected channel out / releases the lock before any network, prepares a fresh channel under ONE bounded `PRODUCER_OPEN_BOUND` (10 s) deadline with the lock free, and publishes into the cache only if still vacant (a concurrent winner's channel is used; the loser is closed bounded). `invalidate_cached`/`mark_channel_dead` now take the captured `channel.id()` + generation and clear ONLY that exact origin, so a concurrent replacement survives (generation qualification preserved, P2 behavior unchanged). The manager seam stays the atomic `connection_within` `(conn, generation)` pair (no `.current_generation()` read after the await).
- **Concurrency regression (tests/publisher_confirms.rs):** the one added test `concurrent_confirm_timeouts_complete_within_bound` warms a producer + queue, pauses the broker, then staggers three cloned publishes with a bounded `tokio::time::interval(600 ms)` ticker (no settling sleep, no ratchet increase): A at ~0 ms, B at ~600 ms (A's confirm pending), C at ~1200 ms (after A's timeout invalidated the cache while B's old confirm is still pending, so C must prepare a fresh channel on the paused broker). A and B must each return `Ok(Err(..))` within 1 s confirm + 1 s cleanup; C is asserted still stalled (`!is_finished()`) when they complete — proof the fresh-creation path never held the cache lock — then aborted. Each `JoinHandle` is awaited at the CALLSITE under an explicit `tokio::time::timeout` (see the wait-site fix bullet), so the join wait itself is bounded, not just the inner publish. No private-cache inspection and no production test hook were needed.
  - RED (real behavior, buggy producer temporarily restored): `task3.1-concurrencyfix-red.log` — `concurrent_confirm_timeouts_complete_within_bound` failed `0 passed; 1 failed`, B blocked on the cache lock and returned `Err(Elapsed(()))` at the 6 s outer bound.
  - GREEN: `task3.1-concurrencyfix-lib.log` lib `58 passed; 0 failed`; `task3.1-concurrencyfix-publisher_confirms-final.log` `3 passed; 0 failed`; `task3.1-concurrencyfix-docker-full.log` full RabbitMQ tier `19 passed; 0 failed` (publish_roundtrip 5, consume_delivery 6, consumer_readiness 1, reconnect 4, publisher_confirms 3). No leftover `rmq-itest-*`.
  - Gates: `task3.1-concurrencyfix-clippy-final.log` clippy exit 0; `task3.1-concurrencyfix-fmt-final.log` fresh `cargo fmt --check --all` exit 0; `task3.1-concurrencyfix-xtask-lints-final.log` all lints OK (metric-labels 0, redaction 0, levels 0 strict, unwrap 0, secrets 0, schema OK, test-sleep `471 = max`, cancel-tokens `7 = max`, unbounded-wait `295 < max 296`).
- **Wait-site fix (r_gpt, 2026-10-09): false `allow-test-wait` exemptions removed.** The concurrency regression initially carried three `// allow-test-wait:` markers on the `a.await`/`b.await`/`c.await` joins; review flagged these as false exemptions — ADR-0069 §13.2 requires the wait at the CALLSITE to be bounded, and the inner `CALL_BOUND` does not bound the join wait. All three markers are removed. `a` and `b` are now each awaited via `tokio::time::timeout(JOIN_BOUND = 8 s, ..)` (CALL_BOUND 6 s + close allowance); `c.abort()` is followed by `tokio::time::timeout(REAP_BOUND = 2 s, c)` asserted to yield `JoinError::is_cancelled()` (abort completion is prompt, not synchronous). The inner publish keeps its own `CALL_BOUND` 6 s timeout. Test-only change; no production code touched, no new test. Fresh evidence: `task3.1-waitfix-publisher_confirms.log` `3 passed; 0 failed`; `task3.1-waitfix-clippy.log` clippy exit 0; `task3.1-waitfix-fmt-final.log` fresh `cargo fmt --check --all` exit 0; `task3.1-waitfix-lints.log` test-sleep `471 = max`, cancel-tokens `7 = max`, unbounded-wait `295 < max 296` (no exemption markers). The full 19-test Docker tier and the lib unit suite were not re-run: production code and the lib are unchanged by this test-only edit.
- Scope: only the five task files changed; no P4 or later behavior, no broad workspace gate. Checkbox stays unchecked.

- [x] 3.1

### crates/components/camel-rabbitmq (mandatory)

#### Task 3.2: Mandatory flag and basic.return mapping

**Files:**
- `crates/components/camel-rabbitmq/src/producer.rs` (modified)
- `crates/components/camel-rabbitmq/src/config.rs` (modified)
- `crates/components/camel-rabbitmq/src/metadata.rs` (modified)
- `crates/components/camel-rabbitmq/tests/publisher_confirms.rs` (modified)

**Steps:**
1. Option `mandatory` (bool, default false) in from_uri + descriptor + const.
2. When `mandatory=true`, publish with `BasicPublishOptions { mandatory: true, ..Default::default() }` and route basic.return into the publish's error path (lapin 4 returns API — worker uses the exact lapin surface): a returned message fails the exchange with Err naming exchange + routing key.
3. Docker test `unroutable_mandatory_fails` (gates scenario): publish mandatory to a routing key with no binding → exchange Err naming exchange and key.

**Tests:**
- `unroutable_mandatory_fails` (docker, gates scenario)
- `mandatory_off_drops_silently` (docker): `mandatory=false` same publish → Ok (broker drops) — documents the default
- command: `RABBITMQ_ITEST=1 cargo test -p camel-component-rabbitmq --test publisher_confirms`
- expected: pass

**Acceptance:**
- Docker tier green; metadata parity includes `mandatory`

**Worker ledger (task 3.2, 2026-10-09):**
- Implementation (config.rs): new `mandatory` (bool, default `false`, `config::DEFAULT_MANDATORY`) parsed from `from_uri` into `RabbitEndpointConfig::mandatory`; an invalid value stays an `InvalidUri` naming the parameter. `P3_OPTIONS` partial const gains `mandatory` (`["confirmTimeout", "mandatory"]`) and `is_known_option` unions it, so the option is accepted end-to-end without surfacing any non-3.2 option.
- Implementation (producer.rs): the publish sends `BasicPublishOptions { mandatory: config.mandatory, .. }`. The broker verdict is mapped by a pure `map_confirmation(target, lapin::Confirmation)` over the EXACT lapin 4 surface: `Ack(None) | NotRequested → Ok`; `Ack(Some(returned)) | Nack(Some(returned)) → Err(unroutable_error(..))`; `Nack(None) → Err(confirm_error(.., "was nacked by the broker"))` (the 3.1 nack naming is unchanged; the consolidated `RabbitError` taxonomy is deferred to 3.5, so all three stay `CamelError::ProcessorError`). `unroutable_error` names `(exchange, routing_key)` and the broker reply, and includes the token `unroutable` so a return is distinguishable from a 404 channel error on a missing exchange. (The first cut wrongly claimed lapin correlates the return to the exact publish; corrected by the return-fix subsection below.)
- Health/cache lifecycle: the return path (`Ack(Some)`/`Nack(Some)`) does NOT call `mark_channel_dead`, `invalidate_cached`, or `manager.note_failure_for_generation`, and never resets the shared connection; on the mandatory path the per-call dedicated channel is closed bounded (`CHANNEL_CLOSE_BOUND`) after the verdict. Only the confirm-timeout path of the cached (`mandatory=false`) channel invalidates the exact origin. The failed exchange is still counted: `call` maps the `Err` to `PublishOutcome::Failure` (`record_publish` → publish/error metrics).
- Implementation (metadata.rs): descriptor gains `mandatory` (`default = "false"`); `metadata_parity_p3_partial` keeps asserting `P1 ∪ P2 ∪ P3` exactly and adds the `mandatory` default/then-not-required assert. No new test function.
- Tests (tests/publisher_confirms.rs): `unroutable_mandatory_fails` and `mandatory_off_drops_silently`, plus a local `declare_unbound_exchange(url, name)` helper using raw lapin `exchange_declare` (Direct, auto_delete). Both use a DECLARED named exchange with NO bindings, so the broker returns the message (`basic.return`) instead of a 404 closing the channel; the mandatory test asserts the error names the exchange + routing key AND contains `unroutable` (proves the return path, not the missing-exchange 404). The default test asserts `Ok` (broker drops).
- Test (src/producer.rs unit, docker-gated): `concurrent_mandatory_returns_are_isolated` reuses the integration fixture via `connection::docker_fixture` (that alias in `connection.rs` is now `pub(crate)`; no fixture logic duplicated) and the private `acquire_channel` seam. It opens two mandatory leases at once, asserts DISTINCT channel ids plus the same shared connection `Arc`/generation, then publishes raw on each (A to a bound routing key, B to an unbound one) and confirms both through the production `map_confirmation`: A → `Ok`, B → `Err(unroutable)`, and only A's payload is routed. Without `RABBITMQ_ITEST=1` it takes the fixture's skip notice; the invariant is never silently asserted without a broker.
- Test-design gap (resolved): `Ack(Some)`/`Nack(Some)` cannot be constructed in a unit test (`BasicReturnMessage`/`Delivery` have no public constructor; the `Acker` field is private), so the mapper's `Some` arms are exercised against the real broker; `Nack(None)` has no deterministic real-broker trigger. The gated isolation unit test closes the gap the two external tests could not prove non-vacuously.
- RED (real behavior): `task3.2-red-publisher_confirms.log` — `unroutable_mandatory_fails` failed with `InvalidUri("unknown query parameter 'mandatory'")` (4 passed; 1 failed). `mandatory_off_drops_silently` already passed pre-change because `mandatory` defaults to false and `BasicPublishOptions::default()` is non-mandatory: it is a default/regression guard, not a new behavioral RED.
- GREEN: lib `58 passed; 0 failed` (`task3.2-lib-final.log`); publisher_confirms `5 passed; 0 failed` (`task3.2-publisher_confirms-final.log`); full RabbitMQ docker tier `21 passed; 0 failed` (`task3.2-docker-full.log`): publish_roundtrip 5, consume_delivery 6, consumer_readiness 1, reconnect 4, publisher_confirms 5 (component 19 prior + 2 new). No leftover `rmq-itest-*` containers.
- Gates: `cargo fmt --check --all` exit 0 and clippy `-p camel-component-rabbitmq --all-targets --all-features -D warnings` exit 0 (`task3.2-clippy-fmt.log`); targeted xtask lints/schema all OK (`task3.2-xtask-lints.log`) — lint-metric-labels 0, lint-log-redaction 0, lint-log-levels 0 (strict), lint-unwrap 0, lint-secrets 0, `schema --check` OK, lint-test-sleep `471 = max 471`, lint-cancel-tokens `7 = max 7`, lint-unbounded-wait `295 < max 296`. Disk stayed at 67 % on `/home/shared`.
- **Return-fix (r_gpt BLOCK, 2026-10-09): lapin's returned message is NOT per-publish.** The first cut claimed the returned message rides the exact publish's confirmation. That is false: lapin's `ReturnedMessages` is a FIFO popped once per completed confirmation (`acknowledgement.rs::complete_pending` → `get_waiting_message`), and pending acks are a `HashMap` walked unsorted. Two concurrent `mandatory=true` publishes on one shared confirm channel can therefore pair one publish's return with another publish's confirm (a returned message read as a clean ack → silently lost). Fix: `acquire_channel(mandatory=true, ..)` creates a FRESH confirm channel per call on the shared connection (via the factored `prepare_channel`, bounded `PRODUCER_OPEN_BOUND` 10 s, no cache lock held) with exactly one outstanding publish — never cached or reused, so a broker return can only pair with this call. `mandatory=false` keeps the task 3.1 cached concurrency untouched. The dedicated channel is closed bounded (`CHANNEL_CLOSE_BOUND` 1 s) after success/return/nack/timeout; on future cancellation the `ChannelLease` drops and lapin's own `ChannelCloser::drop` (`channel_closer.rs`) sends `CloseChannel` when the id is non-zero and still connected — verified in source, no task spawned by us, and a never-reused channel cannot poison a successor. The shared connection/generation is never reset by a mandatory acquire. `NotRequested` stays `Ok` (impossible once `confirm_select` ran; matcher unchanged).
- Return-fix RED (real behavior): `task3.2-returnfix-red.log` — the pre-fix `acquire_channel` (shared cache) made both mandatory leases return channel id `1`, so `assert_ne!` failed (0 passed; 1 failed; 58 filtered).
- Return-fix GREEN: `task3.2-returnfix-lib-gated.log` lib `59 passed; 0 failed` with `concurrent_mandatory_returns_are_isolated` running live against the broker (not skipped; ungated `task3.2-returnfix-lib-ungated.log` `59 passed` with the new test on its skip path); `task3.2-returnfix-publisher_confirms.log` `5 passed`; `task3.2-returnfix-publish_roundtrip.log` `5 passed`; `task3.2-returnfix-docker-full.log` full tier `80 passed` (lib 59 + integration 21: publish_roundtrip 5, consume_delivery 6, consumer_readiness 1, reconnect 4, publisher_confirms 5). No leftover `rmq-itest-*`.
- Return-fix gates: `task3.2-returnfix-clippy-fmt.log` fmt exit 0 + clippy exit 0; `task3.2-returnfix-xtask-lints.log` metric-labels 0, log-redaction 0, log-levels 0 (strict), unwrap 0, secrets 0, schema OK, test-sleep `471 = max`, cancel-tokens `7 = max`, unbounded-wait `295 < max 296`. Disk at 67 %.
- Scope: `config.rs`, `metadata.rs`, `producer.rs`, `tests/publisher_confirms.rs`, plus `connection.rs` (fixture alias `pub(crate)`) and this ledger. No P4/later behavior, no broad workspace gate. Checkbox stays unchecked.

- [x] 3.2

### crates/components/camel-rabbitmq (passive check)

#### Task 3.3: Passive queue-exists check at consumer start

**Files:**
- `crates/components/camel-rabbitmq/src/consumer.rs` (modified)
- `crates/components/camel-rabbitmq/tests/topology.rs` (new)

**Steps:**
1. After connect + channel creation and BEFORE `mark_ready`: `channel.queue_declare(queue, QueueDeclareOptions { passive: true, ..Default::default() }, FieldTable::default())` — a 404 channel error fails consumer start (`ctx.mark_failed` + start Err) naming the missing queue (fail fast).
2. The passive check runs on a dedicated short-lived channel created for the check and dropped after — a 404 closes only that probe channel and the consume channel stays clean (comment documents this).
3. `tests/topology.rs`: `missing_queue_fails_fast` (gates scenario) — no queue declared, consumer start → Err containing the queue name + `mark_failed` (startup signal stays not-ready); `existing_queue_proceeds` — queue pre-declared → start reaches ready.

**Tests:**
- `missing_queue_fails_fast` (docker, gates scenario)
- `existing_queue_proceeds` (docker)
- `passive_check_error_names_queue` (unit): error-mapping seam asserts the queue name present
- command: `cargo test -p camel-component-rabbitmq --lib && RABBITMQ_ITEST=1 cargo test -p camel-component-rabbitmq --test topology`
- expected: pass

**Acceptance:**
- Both tiers green; consumer start never consumes from a missing queue

**Worker ledger (task 3.3, 2026-10-09):**
- Implementation (consumer.rs): `passive_queue_check(manager, queue)` runs ONCE per `start()`, after `connection_within(CONSUMER_START_BOUND)` succeeds and BEFORE any engine channel is created. It opens a dedicated short-lived probe channel through the manager's bounded helper (`RabbitConnectionManager::consumer_channel`, `channel.open` capped by the manager's own bound), then runs `queue_declare(queue, QueueDeclareOptions { passive: true, ..Default::default() }, FieldTable::default())` under a local `PASSIVE_CHECK_BOUND` (10 s). `autoDeclare` is NOT implemented here — task 3.4 owns active declare, so every start is passive.
- Implementation (consumer.rs): `passive_check_error(queue, detail)` is the task's error-mapping seam — a `CamelError::ProcessorError` naming the queue EXACTLY and carrying only the broker AMQP protocol text (or a local timeout note), never the broker URL or credentials. Both the broker-404 arm and the local-timeout arm map through it; a `tracing::warn!` logs the explicit `queue` (and the safe `broker_error` text) so diagnosis never has to render a URL or secret. `close_probe_channel(channel)` closes the probe bounded (`PROBE_CLOSE_BOUND`, 1 s) on BOTH the Ok and Err paths; if the wait does not resolve the channel drops and lapin's `ChannelCloser::drop` completes/aborts the close (no task spawned, channel never reused). A probe 404 is a soft AMQP error: the connection-scoped `spawn_error_listener` classifies it channel-local and keeps watching, so the manager is never demoted and the shared connection/generation is preserved for siblings.
- `start()` wiring: the probe failure calls `ctx.mark_failed(error.to_string())` and returns the same `Err` fail-fast, so the Explicit startup signal resolves FAILED and the runtime surfaces a precise startup error instead of silently retrying; no consume registration ever happens for a missing queue. Start still connects first (not-ready while retrying); only after connect does the probe govern.
- Test (src/consumer/tests.rs unit): `passive_check_error_names_queue` asserts the seam's message contains the exact queue name and carries no `amqp://` URL. Test (tests/topology.rs, new): `missing_queue_fails_fast` (gates "missing queue fails fast") starts against an undeclared queue → `start()` Err names the queue AND does NOT contain `basic_consume` (proves the failure came from the passive probe, never a consume registration); the `StartupSignal` receiver resolves `Err` (FAILED, never Ready). It then captures the manager's live `(Arc<Connection>, generation)` BEFORE the failure and asserts `Arc::ptr_eq` + equal generation AFTER, and starts a sibling consumer on a real queue of the SAME manager, publishes, and consumes one message — the natural error-isolation proof (`spawn_error_listener` soft classification) lives inside this one test, no extra design. `existing_queue_proceeds` pre-declares the queue → start reaches ready and stops cleanly. Both topology tests serialize on one `TOPOLOGY_TESTS` mutex.
- RED (real behavior): `task3.3-red-unit.log` — `cargo test --lib` failed to compile with `error[E0425]: cannot find function 'passive_check_error'` (the new seam did not exist). `task3.3-red-topology.log` — against the pre-change consumer, `missing_queue_fails_fast` FAILED (`1 passed; 1 failed`): the pre-change path failed through `basic_consume on queue ...` (the assertion forbids that token), while `existing_queue_proceeds` passed (regression guard, no behavioral RED of its own).
- GREEN: lib gated `60 passed; 0 failed` with `passive_check_error_names_queue` (`task3.3-lib-gated.log`, 59 baseline + 1 new). `--test topology` `2 passed; 0 failed` (`task3.3-topology.log`). Full RabbitMQ docker tier `83 passed; 0 failed` (`task3.3-docker-full.log`): lib 60 + integration 23 (publish_roundtrip 5, consume_delivery 6, consumer_readiness 1, reconnect 4, publisher_confirms 5, topology 2 — 21 prior + 2 new). No leftover `rmq-itest-*` containers. The prior `consumer_readiness` delayed-connect gate conforms unchanged: the queue is declared before the held connection is released, so the passive check finds it available before `basic.consume`.
- Reconnect/queue-delete behavior unchanged: the passive check runs only at `start()`, never in the engine's `register_consumer` re-open loop, so a local queue deletion after start still surfaces as a channel-local 404 on `basic.consume` retried under the bounded reopen backoff (task 3.4 owns any reopen-time passive handling). No P2 sibling regression (reconnect 4/4 green).
- Gates: `cargo fmt --check --all` exit 0 (`task3.3-fmt.log`); clippy `-p camel-component-rabbitmq --all-targets --all-features -D warnings` exit 0 (`task3.3-clippy.log`); targeted xtask lints/schema all OK (`task3.3-xtask-lints.log`) — lint-metric-labels 0, lint-log-redaction 0, lint-log-levels 0 (strict), lint-unwrap 0, lint-secrets 0, `schema --check` OK, lint-cancel-tokens `7 = max 7`, lint-test-sleep `471 = max 471`, lint-unbounded-wait `295 < max 296` (all ratchets unchanged). Producer return/generation paths untouched. No broad workspace gate re-run.
- **Diagnostic-wording fix (r_gpt APPROVE-with-must-resolve, 2026-10-09):** `passive_check_error` no longer hard-codes "does not exist" — a permission error or a stalled/timed-out RPC is not a missing queue. The message is now neutral: `rabbitmq consumer passive queue check failed for queue '<queue>': <detail>`, preserving the exact queue name and the verbatim broker detail, so only the broker's own `404 NOT_FOUND` text proves absence (task 3.5 will map an actual 404 to `RabbitError::MissingQueue`). The existing `passive_check_error_names_queue` unit test is now table-driven over both a `404 NOT_FOUND` detail and a `timed out after 10s` detail (no new test fn), asserting the queue name and the detail both survive and no broker URL leaks. Fresh targeted evidence (`task3.3-diagnosticfix-*`): lib gated `60 passed` with `passive_check_error_names_queue` ok (`task3.3-diagnosticfix-lib.log`), `--test topology` `2 passed` (`task3.3-diagnosticfix-topology.log`), fmt exit 0 (`task3.3-diagnosticfix-fmt.log`), clippy exit 0 (`task3.3-diagnosticfix-clippy.log`); each log records its UTC timestamp, CMD, `df`, and EXIT. No leftover `rmq-itest-*`; disk 67 %. The full unchanged Docker tier was NOT re-run (fresh full-tier reruns = 0); behavior is unchanged.
- Scope: `consumer.rs`, `consumer/tests.rs`, `tests/topology.rs` (new), this ledger. No autoDeclare or other 3.4 behavior, no P4/later, no merge/commit. Checkbox stays unchecked.

- [x] 3.3

### crates/components/camel-rabbitmq (auto-declare)

#### Task 3.4: Opt-in autoDeclare with x-args and 406 conflict handling

**Files:**
- `crates/components/camel-rabbitmq/src/topology.rs` (new)
- `crates/components/camel-rabbitmq/src/consumer.rs` (modified)
- `crates/components/camel-rabbitmq/src/config.rs` (modified)
- `crates/components/camel-rabbitmq/src/metadata.rs` (modified)
- `crates/components/camel-rabbitmq/tests/topology.rs` (modified)

**Steps:**
1. Options: `autoDeclare` (bool, default false), `exchangeType` (string, default "direct"), `durableQueue` (bool, default true), `queueArguments` (string, JSON object of string→string, e.g. `queueArguments={"x-dead-letter-exchange":"dlx"}` — no existing component parses map params from URIs, so the JSON-object encoding is this change's decision; parse via `serde_json::from_str::<HashMap<String,String>>`, reject non-object/non-string values naming the option; each value maps to a long-str FieldTable entry).
2. `src/topology.rs`: `pub(crate) async fn declare_topology(channel, config) -> Result<(), CamelError>` — active declare: when exchange != "" — `exchange_declare(exchange, kind from exchangeType, durable, …)`, `queue_declare(queue, durable=durableQueue, arguments=queueArguments as FieldTable)`, `queue_bind(queue, exchange, routing_key)`. When exchange == "" (default exchange) SKIP exchange_declare AND queue_bind (the default exchange cannot be declared or bound — broker answers 403). A 404/405/406 channel error maps to Err carrying the broker's message text (fail route start via mark_failed).
3. Consumer start: `autoDeclare=true` → `declare_topology`; `false` → passive check (3.3). Documented divergence comment (Camel spring-rabbitmq consumer default is autoDeclare=true).
4. Docker tests: `autodeclare_creates_topology` (gates scenario — NAMED exchange endpoint, queue absent + autoDeclare=true + defaults → durable queue exists (passive check passes), bound, consumption starts); `conflicting_declare_fails_start` (gates scenario — pre-create queue non-durable, autoDeclare durableQueue=true → start Err carrying `PRECONDITION` from the broker); `autodeclare_honors_exchange_type` (`exchangeType=fanout` declared as fanout → raw `exchange_declare(kind=Direct)` on the same name gets 406 — proves the option is read); `autodeclare_honors_durable_false` (`durableQueue=false` → raw `queue_declare(durable=true)` on the queue gets 406 PRECONDITION_FAILED); `queue_arguments_passthrough` (`queueArguments={"x-dead-letter-exchange":"dlx"}` + autoDeclare → the 2.2 DLX routing test flow works with the arg-created queue).

**Tests:**
- `autodeclare_creates_topology` (docker, gates scenario)
- `conflicting_declare_fails_start` (docker, gates scenario)
- `autodeclare_honors_exchange_type` (docker, option non-vacuous)
- `autodeclare_honors_durable_false` (docker, option non-vacuous)
- `queue_arguments_passthrough` (docker)
- command: `RABBITMQ_ITEST=1 cargo test -p camel-component-rabbitmq --test topology`
- expected: pass

**Acceptance:**
- Docker tier green; parity includes the four new options; `metadata_parity_p3` = P1 ∪ P2 ∪ {confirmTimeout, mandatory, autoDeclare, exchangeType, durableQueue, queueArguments}

**Worker ledger (task 3.4, 2026-10-09):**
- Files: `src/topology.rs` (new), `src/consumer.rs` (modified), `src/consumer/tests.rs` (modified), `src/config.rs` (modified), `src/metadata.rs` (modified), `src/lib.rs` (modified), `tests/topology.rs` (modified), this ledger. Production build adds NO new workspace root: `serde_json` was already a crate dependency.
- Implementation (config.rs): `P3_OPTIONS` now `["confirmTimeout", "mandatory", "autoDeclare", "exchangeType", "durableQueue", "queueArguments"]` (task 3.5 still freezes). New `RabbitEndpointConfig` fields + parsing: `auto_declare` (bool, `DEFAULT_AUTO_DECLARE=false`), `exchange_type` (String, `DEFAULT_EXCHANGE_TYPE="direct"`), `durable_queue` (bool, `DEFAULT_DURABLE_QUEUE=true`), `queue_arguments`: `serde_json::from_str::<HashMap<String, String>>` — a non-object (e.g. `[1,2]`, `"plain"`) or non-string (`{"x":1}`) value is an `InvalidUri` naming `queueArguments` and the JSON-object contract (strongly typed, no runtime `Value`). Each value maps to an `AMQPValue::LongString` `FieldTable` entry in topology.
- Implementation (topology.rs, new): `topology_check(manager, config)` opens ONE dedicated short-lived probe channel via the manager's bounded `consumer_channel` and dispatches ONCE per start, BEFORE any engine channel: `autoDeclare=false` → `passive_queue_check` (3.3 semantics preserved verbatim, `passive_check_error` seam moved intact); `autoDeclare=true` → `declare_topology(channel, config)`. Active declare order: `exchange_declare(exchange, kind from exchangeType, durable=true)` (named endpoints only), `queue_declare(queue, durable=durableQueue, x-args)` then `queue_bind(queue, exchange, routingKey)`. The default exchange (empty path) SKIPS both exchange declare and bind (the reserved exchange answers 403) but still declares the queue. `exchange_kind` maps direct/fanout/topic/headers exactly; any other string is passed through as `ExchangeKind::Custom`, so a broker-rejected type fails route start (fail closed). Each RPC is bounded by `PASSIVE_CHECK_BOUND` (10 s); the probe close is `PROBE_CLOSE_BOUND` (1 s). `CamelSpringRabbit` divergence documented in config/metadata: its consumer defaults `autoDeclare=true`, this component requires explicit opt-in because an active declare mutates broker topology.
- Error/health: a 404/405/406 soft error closes ONLY the probe channel (connection-scoped listener classifies it channel-local), so the shared connection/generation is never demoted and siblings keep consuming. The broker text is carried verbatim in `declare_error`/`passive_check_error`, so a conflicting declare surfaces the broker's own `PRECONDITION_FAILED`; start calls `ctx.mark_failed` before returning Err (never Ready). `consumer.rs` start now calls `topology_check` once and then the existing per-engine channel/consume loop count-gates ready across all engines.
- Reconnect scope (normative read): the spec scopes autoDeclare to consumer START ("Consumer start SHALL ... With autoDeclare=true ... start SHALL actively declare"). The engine's reconnect/`register_consumer` re-open path is unchanged (basic_consume; a local 404 is retried under the bounded reopen backoff and never invalidates the shared connection). No runtime redeclare was added; that would be P4 or later.
- Tests (tests/topology.rs, +5 fns; existing 2 passive tests unchanged → 7): `autodeclare_creates_topology` (NAMED exchange + absent queue + defaults → durable queue proven by a conflicting non-durable redeclare PRECONDITION, binding proven by publish→consume; plus an additional default-exchange phase proving the skip branch reaches ready and consumes); `conflicting_declare_fails_start` (pre-created non-durable queue + durableQueue=true → start Err and startup signal both carry `PRECONDITION`, shared `Arc`/generation unchanged, sibling consumer on the same manager still consumes); `autodeclare_honors_exchange_type` (`exchangeType=fanout`; raw `exchange_declare(Direct)` → 406); `autodeclare_honors_durable_false` (`durableQueue=false`; raw `queue_declare(durable=true)` → 406); `queue_arguments_passthrough` (`queueArguments={"x-dead-letter-exchange":"<dlx>"}`; failed route rejects to the DLX and the payload lands on the DLQ — the 2.2 flow with the arg-created queue). New raw observers (`raw_channel`/`raw_declare_exchange`/`raw_declare_queue`) and `publish_to_exchange`/`answer_err`/`basic_get`/`wait_for_message` helpers are bounded; no production test hook. Unit: `endpoint_config_parses_default_exchange` now asserts the four defaults; new `endpoint_config_rejects_non_string_queue_arguments` (table-driven `{"x":1}`/`[1,2]`/`"plain"`); `metadata_parity_p3_partial` asserts the four new descriptor defaults (folded into the existing fns; no extra metadata fn).
- Probe isolation: `missing_queue_fails_fast` and `conflicting_declare_fails_start` both assert `Arc::ptr_eq` + equal generation across the soft probe failure and a sibling consume, so the probe channel is provably isolated. `queue_arguments` stays strongly typed (`HashMap<String,String>` → `FieldTable` LongString at declare time); no `serde_json::Value` in the config surface.
- RED (real behavior): `task3.4-red-lib.log` — `cargo test --lib` failed to compile, `E0609` x4 (the 4 new config fields did not exist). `task3.4-red-topology.log` — against the pre-change consumer, `2 passed; 5 failed`: the 5 new tests panicked at their `from_uri(...).expect(...)` because `autoDeclare` was an unknown query parameter; the 2 existing passive tests passed unchanged.
- GREEN: `task3.4-green-lib.log` gated lib `61 passed; 0 failed` (60 baseline + 1 new config unit; the live `concurrent_mandatory_returns_are_isolated` ran, RABBITMQ_ITEST=1). `task3.4-green-topology.log` `--test topology` `7 passed; 0 failed`. `task3.4-full-tier.log` full RabbitMQ tier EXIT 0: lib 61 + integration 28 (publish_roundtrip 5, consume_delivery 6, consumer_readiness 1, reconnect 4, publisher_confirms 5, topology 7). No leftover `rmq-itest-*` containers; disk 67 % on `/home/shared`.
- Gates: `task3.4-fmt.log` `cargo fmt --check --all` exit 0 (after `cargo fmt --all`); `task3.4-clippy.log` `cargo clippy -p camel-component-rabbitmq --all-targets --all-features -D warnings` exit 0. `task3.4-xtask-lints.log`: lint-metric-labels 0, lint-log-redaction 0, lint-log-levels 0 (strict), lint-unwrap 0, lint-secrets 0, lint-cancel-tokens `7 = max 7`, lint-test-sleep `471 = max 471`, lint-unbounded-wait `295 < max 296`, `schema --check` OK (all schemas and TS types match). Producer return/generation paths untouched; no broad workspace gate re-run.
- **Wait-site fix (r_gpt APPROVE-with-minor, 2026-10-09): whole helper publish bounded.** `publish` and `publish_to_exchange` in `tests/topology.rs` awaited `basic_publish(..)` UNBOUNDED before the confirm timeout, so a stalled initial RPC could park the test (ADR-0069 §13.2 callsite bound). Both helpers now wrap the ENTIRE `basic_publish` + confirm sequence in ONE `timeout(RPC_BOUND, async { .. })` at the call site. Test-only edit, two helpers, no new functions and no production change. Fresh targeted evidence (all with UTC / ENV / CMD / `df` / EXIT, `/home/shared/rust-camel-worktrees/349-rabbitmq/target` build dir): `task3.4-waitfix-fmt.log` fmt exit 0; `task3.4-waitfix-clippy.log` `clippy --all-targets --all-features -D warnings` exit 0; `task3.4-waitfix-topology.log` `--test topology` `7 passed; 0 failed`; `task3.4-waitfix-lints.log` lint-unbounded-wait `295 < max 296`. The lib `61` / integration `28` counts are the unchanged original-tier numbers; the wait-fix was NOT re-run across the full tier (fresh full-tier reruns = 0).
- **Log retention (2026-10-09, mission requirement):** during execution the 13 `task3.4-*` evidence logs were written to the shell working directory `/home/shared/tmp/`; they were then MOVED into the worktree evidence dir `/home/shared/rust-camel-worktrees/349-rabbitmq/target/logs/` (parent verified to exist first; only the exact known filenames moved — no glob/find). SHA-256 verified byte-identical before and after for all 13 (CMD/ENV/UTC/df/EXIT untouched). Source now holds zero leftovers; destination holds exactly 13. Retained full paths: `…/target/logs/task3.4-{red-lib,red-topology,green-lib,green-topology,full-tier,compile-lib,fmt,clippy,xtask-lints,waitfix-fmt,waitfix-clippy,waitfix-topology,waitfix-lints}.log`.
- Scope: only the seven task files plus this ledger; no P4/later, no commit/merge/push. Checkbox stays unchecked.

- [x] 3.4

### crates/components/camel-rabbitmq (P3 freeze)

#### Task 3.5: P3 parity freeze and error taxonomy consolidation

**Files:**
- `crates/components/camel-rabbitmq/src/metadata.rs` (modified)
- `crates/components/camel-rabbitmq/src/error.rs` (new)

**Steps:**
1. `src/error.rs`: `#[non_exhaustive] pub enum RabbitError { Disconnected, ConfirmTimeout { exchange, routing_key }, ConfirmNacked { exchange, routing_key }, Unroutable { exchange, routing_key }, MissingQueue (String), DeclareConflict (String), PublishFailed (String) }` consolidating the error constructors scattered across 1.3–3.4 (each maps into `CamelError::ProcessorError`/component error per the component-error-semantics mapping used by kafka); update call sites. P4's `ReplyTimeout` variant is ADDED in 4.1 (`#[non_exhaustive]` permits extension).
2. Freeze `P3_OPTIONS`; parity test = P1 ∪ P2 ∪ P3 exactly.
3. Unit test exercising every variant's Display text naming the required tokens (exchange/routing key/queue name/PRECONDITION) — table-driven `error_display_names_targets`.

**Tests:**
- `metadata_parity_p3_frozen`: descriptor == P1 ∪ P2 ∪ P3
- `error_display_names_targets`: each variant's message contains its required token
- command: `cargo test -p camel-component-rabbitmq --lib`
- expected: pass

**Acceptance:**
- AGENTS.md QUALITY GATES green on the crate (fmt, clippy workspace set — the crate has no native deps so it joins no exclusion list)
- All P3 gating scenarios green

**Worker ledger (task 3.5, 2026-10-09):**
- Files: `src/error.rs` (new), `src/lib.rs` (modified), `src/metadata.rs` (modified), `src/config.rs` (doc only), `src/connection.rs` (modified), `src/producer.rs` (modified), `src/consumer.rs` (modified), `src/topology.rs` (modified), `src/consumer/tests.rs` (modified), this ledger. No new dependency: `thiserror` was already a crate dependency.
- Implementation (error.rs, new): `#[non_exhaustive] pub enum RabbitError` with exactly the seven planned variants (`Disconnected`, `ConfirmTimeout{exchange,routing_key}`, `ConfirmNacked{…}`, `Unroutable{…}`, `MissingQueue(String)`, `DeclareConflict(String)`, `PublishFailed(String)`); `Display` + `std::error::Error` via the existing `thiserror`; `impl From<RabbitError> for CamelError` → plain `CamelError::ProcessorError(error.to_string())`. Kafka does NOT use `ProcessorErrorWithSource`, so no source chain was added and no dependency was pulled; every variant converts the same way and none panics. Exported as `pub mod error;` + `pub use error::RabbitError;`. P4 `ReplyTimeout` NOT added (the enum stays at 7).
- Typed classification: `RabbitError::for_queue_error(queue, &lapin::Error)` reads the AMQP reply code via `ErrorKind::ProtocolError(amqp).get_id()` — only `404` → `MissingQueue(queue)`, every other code/timeout/permission is the neutral `PublishFailed` that still names the queue (no false absence, no message-text heuristics). `RabbitError::for_declare_error(op, target, &lapin::Error)` maps `405`/`406` → `DeclareConflict` carrying the broker's verbatim text (so `PRECONDITION` and queue/exchange names survive); other codes are neutral.
- Call-site consolidation: `connection.rs` `disconnected_error` → `Disconnected`; connect failure/cancel and `consumer_channel` open failure/timeout → `PublishFailed` (redacted URL preserved). `producer.rs` confirm timeout → `ConfirmTimeout`; `Nack(None)` → `ConfirmNacked`; `Ack(Some)/Nack(Some)` → `Unroutable`; generic publish/channel-prep/body failures → `PublishFailed`. `consumer.rs` ack/nack/qos/consume/engine-panic/stop → `PublishFailed`. `topology.rs` passive/active → the typed constructors above; the string-formatting `passive_check_error`/`declare_error` constructors were removed. Unchanged by design: `ConfigInvalidUri` stays `CamelError::InvalidUri`; `ConsumerContext` `ChannelClosed` stays a direct `CamelError::ChannelClosed` (route-transport classifier and `DispositionApplied` metric semantics untouched, G2); broker-resolution `Config` errors untouched; poison-recovery (`PoisonError::into_inner`) and lifecycle checks untouched. Connection-level failure notes still run BEFORE the error is mapped, generation-qualified.
- Metadata freeze: `P3_OPTIONS` frozen (the same six: `confirmTimeout`, `mandatory`, `autoDeclare`, `exchangeType`, `durableQueue`, `queueArguments`); `metadata_parity_p3_partial` renamed to `metadata_parity_p3_frozen`, asserting the descriptor equals `P1 ∪ P2 ∪ P3` exactly with no P4 option. New unit test `error_display_names_targets` is a table over all seven variants (tokens: `disconnected`, `confirm`+exchange+routing key, `nacked`+target, `unroutable`+target, queue name, `PRECONDITION`+target, generic detail) plus typed 404/405/406 classification and the `ProcessorError` conversion.
- Tests: RED not captured as a separate artifact — the enum and its test landed together, so the guidance's "compile-missing error then GREEN" step was not run as a distinct red. GREEN: `task3.5-lib-ungated.log` `--lib` `62 passed; 0 failed`; `task3.5-full-tier.log` `RABBITMQ_ITEST=1 cargo test -p camel-component-rabbitmq` EXIT 0 — lib `62` (includes the live `concurrent_mandatory_returns_are_isolated`) + integration `28` across six files (consume_delivery 6, consumer_readiness 1, publish_roundtrip 5, publisher_confirms 5, reconnect 4, topology 7). No leftover `rmq-itest-*` containers; disk 67 % on `/home/shared`.
- Gates: `task3.5-fmt.log` `cargo fmt --all` then `--check` exit 0; `task3.5-clippy.log` `cargo clippy -p camel-component-rabbitmq --all-targets --all-features -- -D warnings` exit 0; `task3.5-workspace-clippy.log` the AGENTS.md canonical workspace clippy (`--workspace --all-features` excluding `camel-cli`, `camel-component-kafka`, `security-keycloak`, `security-wasm-policy`) exit 0 (0 errors); `task3.5-xtask-lints.log` — 15 of the 16 registry lints pass (`lint-unwrap`, `lint-secrets`, `lint-single-source`, `lint-non-exhaustive`, `lint-log-levels`, `lint-log-redaction`, `lint-cancel-tokens` `7 = max 7`, `lint-test-sleep`, `lint-unbounded-wait`, `lint-ignore`, `lint-publish-cycles`, `lint-component-deps`, `lint-gate-forwarding`, `lint-context-citations`, `lint-metric-labels`) and `schema --check` exit 0; `lint-publish-registration` FAILED with the expected Case B — `camel-component-rabbitmq` has never been published, so the owner must do the manual first publish and register trustpub (not a code defect). `lint-commits` NOT run (remote); `cargo audit`, `doc-build`, and full workspace tests not run (unchanged broads; `doc-build` does not even include this crate).
- Log retention: all six `task3.5-*` evidence logs were written directly to the worktree evidence dir `/home/shared/rust-camel-worktrees/349-rabbitmq/target/logs/` (never `/tmp` or `/home/shared/tmp`); no move was needed. The one interleaved `task3.5-full-tier.log` produced when an orphaned first run overlapped a re-run was deleted and the clean `task3.5-full-tier-verify.log` renamed to `task3.5-full-tier.log`; the retained file is the clean single-run EXIT 0 artifact. Exact paths: `…/target/logs/task3.5-{fmt,clippy,workspace-clippy,xtask-lints,lib-ungated,full-tier}.log`.
- Scope: only the task files plus this ledger; no commit/merge/push/rebase; no P4/later; checkbox stays unchecked.

- [x] 3.5

**P3 boundary acceptance (2026-10-09):**
- `r_gpt` returned **APPROVE**, with no findings, for the complete `36f9d6de...c99d1154` phase diff. Tasks 3.1–3.5 and the Phase 3 exit criteria are satisfied.
- Retained current-code verification: 62 library tests, including one live broker test, and 28 integration tests pass. Component and canonical workspace-set clippy, formatting, 15 local lints, and schema pass. Fresh boundary checks pass 16 bundle tests, 19 CLI feature-profile tests, and strict OpenSpec validation; 17 of 24 tasks are complete. Unchanged broad gates were not rerun.
- `lint-publish-registration` remains expected Case B for the unpublished crate: owner first publication and trustpub registration are pending. Full workspace tests, audit, full documentation build, remote commit lint, and separate CLI/Kafka clippy gates remain unrun. Task 3.5 has no captured RED run; its passing tests do not establish a TDD cycle.
- Startup timing is bounded per operation: initial connection acquisition has 60 seconds, topology RPCs have 10 seconds each, and probe cleanup has one second. The runtime readiness backstop has 90 seconds; the 60-second constant is not an aggregate consumer-start deadline.
- Park at the Phase 3 boundary. Phase 4 requires a master ruling.

## Phase 4: Request/reply

### crates/components/camel-rabbitmq (correlation)

#### Task 4.1: Correlation table with bounded replyTimeout and InOut producer half

**Files:**
- `crates/components/camel-rabbitmq/src/reply.rs` (new)
- `crates/components/camel-rabbitmq/src/error.rs` (modified — ReplyTimeout variant)
- `crates/components/camel-rabbitmq/src/producer.rs` (modified)
- `crates/components/camel-rabbitmq/src/config.rs` (modified)
- `crates/components/camel-rabbitmq/src/metadata.rs` (modified)

**Steps:**
1. Option: `replyTimeout` (u64 ms, default 30000) in from_uri + descriptor. InOut detection reads `exchange.pattern == camel_api::ExchangePattern::InOut` (`camel-api/src/exchange.rs:73`) — camel-jms parses exchangePattern from the URI, which this change must NOT copy.
2. Add `RabbitError::ReplyTimeout { timeout_ms: u64 }` to the taxonomy (extends the 3.5 enum; `#[non_exhaustive]` permits it) and extend `error_display_names_targets` with its Display containing `reply` and the ms value.
3. `src/reply.rs`: `pub(crate) struct CorrelationTable { inner: DashMap<String, oneshot::Sender<ReplyPayload>> }` with `register(correlation_id) -> oneshot::Receiver`, `resolve(correlation_id, payload) -> bool` (false = late/dropped), `remove(correlation_id)`; uuid v4 correlation ids. The unit seam is `pub(crate) async fn await_reply(table: &CorrelationTable, id: &str, rx: oneshot::Receiver<ReplyPayload>, timeout: Duration) -> Result<ReplyPayload, RabbitError>` — timeout removes the entry and errs `ReplyTimeout`.
4. InOut publish path: reply channel = dedicated channel on the SAME connection; `basic_consume("amq.rabbitmq.reply-to", …, no_ack=true per direct-reply-to rules)` started BEFORE the first InOut publish; publish with `reply_to = "amq.rabbitmq.reply-to"`, `correlation_id = <uuid>`; `await_reply` bounded by `replyTimeout`; requests still use confirms from 3.1 (allowed); replies are consumed no-ack.

**Tests:**
- `correlation_register_resolve_roundtrip` (unit): register → resolve true → receiver got payload
- `correlation_resolve_after_remove_is_late` (unit): remove then resolve → false
- `reply_timeout_fails_within_bound` (unit): `await_reply` with 50 ms timeout, no resolver → Err `ReplyTimeout` elapsed < 1 s; table entry removed (`table.len() == 0` — the accessor is pub(crate), the unit test lives in-crate)
- command: `cargo test -p camel-component-rabbitmq --lib`
- expected: pass

**Acceptance:**
- lib tests green; DashMap/oneshot from existing workspace deps

**Worker ledger (task 4.1, 2026-10-09):**
- Files: `src/reply.rs` (new), `src/error.rs`, `src/config.rs`, `src/metadata.rs`, `src/producer.rs` + `src/producer/tests.rs` (tests decomposed out-of-line, P2 pattern; producer production 760 lines), `src/connection.rs` (one visibility change), `src/lib.rs`, `Cargo.toml`, `Cargo.lock`, this ledger. `dashmap`/`uuid` were already workspace deps (`Cargo.toml:175,233`); they are now crate deps (the only lockfile delta is this crate's dependency list).
- Config/metadata: `P4_OPTIONS = ["replyTimeout"]`, `DEFAULT_REPLY_TIMEOUT_MS = 30000`, `RabbitEndpointConfig.reply_timeout: Duration` parsed from `replyTimeout` (non-negative u64 ms), descriptor `_reply_timeout` (`default = "30000"`). InOut detection reads `exchange.pattern == camel_api::ExchangePattern::InOut`; the URI `exchangePattern` parameter is NOT added (JMS parity not copied).
- Error: `RabbitError::ReplyTimeout { timeout_ms: u64 }` added to the `#[non_exhaustive]` taxonomy; `error_display_names_targets` gains one row asserting Display contains `reply` and `2000` (existing table, no new fn).
- `reply.rs`: `CorrelationTable { inner: DashMap<String, oneshot::Sender<ReplyPayload>> }` with `register` / `resolve` (remove-then-send, so at-most-once) / `remove` / `len` (`#[cfg(test)]`) / `clear`; UUID v4 ids generated at the producer. `await_reply` keeps the exact 4.1 signature and uses a `RemoveOnDrop` guard so a caller-cancelled future removes its entry.
- Loop ownership (G-5): the reply loop is generic over `Stream<Item = (Option<String>, ReplyPayload)>` (thin lapin `filter_map` adapter builds `headers::inbound` + `Bytes`, mapping the delivery's actual `redelivered` flag) and captures ONLY `Arc<CorrelationTable>` + `Arc<AtomicBool>` + a channel clone + `Option<CancellationToken>` — never `Arc<ReplyState>`, so there is no reference cycle. `ReplyState::new` follows G-1 order: `prepare_channel` (open + confirm_select) -> `basic_consume("amq.rabbitmq.reply-to", no_ack)` -> spawn loop. `Drop = cancel() + handle.abort() + table.clear()` (mqtt `DriverHandle` shape, no blocking join); the channel closes via lapin's `ChannelCloser` when the last clone/state drops.
- Cancellation / retirement (G-3/G-6/G-7): `publish_in_out` calls the `admit` seam, which acquires the bounded mandatory lane FIRST and only then registers the correlation (registration is still before `basic_publish`, so an early reply is not lost). A caller cancelled while waiting for the lane has registered nothing, so no entry can leak; registration, the retirement re-check, and the caller arming `ConfirmGuard` are contiguous with no `.await` in between. A lane timeout registers nothing (returns `ConfirmTimeout`); a state retired before registration removes the entry and returns `Disconnected` without a reply wait. Once the lane is won, `ConfirmGuard` is armed from `basic_publish` through the clean confirm verdict: a caller cancellation (future dropped), a channel-level publish error, or a confirm timeout before the verdict drops the guard -> `retire_sync` (retired flag, drain table, cancel + abort loop), so a stale `basic.return` cannot pair with a successor's confirm. A clean verdict (`Ack`/`Nack`) marks the guard complete: an `Unroutable`/`ConfirmNacked` result removes only this request's entry and leaves the shared state healthy. A plain `ReplyTimeout` does NOT retire the state (late replies are dropped by `resolve == false`), preserving the 4.4 late-reply follow-up. A drained receiver maps to `Disconnected`, never `ReplyTimeout`.
- Generation details: `ReplyState::new` obtains `(connection, generation)` from `connection_within`; the generation is deliberately NOT stored (no dead field) because slot reuse identity is the `Arc` plus `is_usable() = !retired && !loop_abort.is_finished() && channel.status().connected()`. A reconnect closes the channel and ends the loop, so `is_usable()` is false and the next InOut displaces the stale state; the first-use race replaces the slot only when the present state is unusable and retires the fresh loser bounded. The producer holds the state lazily in `Arc<std::sync::Mutex<Option<Arc<ReplyState>>>>` (`ReplySlot`), mirroring `ChannelCache` (build outside the lock, never hold the guard across an `.await`).
- InOut path (`producer.rs`): `publish_in_out` publishes on the shared reply channel with `reply_to = amq.rabbitmq.reply-to` and `correlation_id = uuid v4`, confirms under `confirmTimeout`, maps the verdict through `map_confirmation`, then `await_reply` bounded by `replyTimeout`; on success `exchange.output = Some(reply body + inbound headers)` and `pattern` stays InOut. The InOnly path (3.1 cached channel / 3.2 mandatory leases) is unchanged. Broker RPC reply tests are 4.4's (absent here); the 4.1 production path is compile/clippy verified and every spawned task is owned (the reply loop is abort-controlled by its `ReplyState`). 4.2 still owns the loop/Drop unit tests; 4.3 owns the consumer reply publisher.
- RED: `target/logs/task4.1-red-lib.log` — `61 passed; 4 failed` (metadata_parity_p4_partial + the three `reply::` tests against `unimplemented!` stubs).
- GREEN: `target/logs/task4.1-green-lib.log` — `65 passed; 0 failed` (62 baseline + 3 new). Docker-gated lib `target/logs/task4.1-docker-lib.log` — `65 passed` (the `RABBITMQ_ITEST=1` gate ran `concurrent_mandatory_returns_are_isolated`). Integration `target/logs/task4.1-docker-tests.log` — consume_delivery 6, consumer_readiness 1, publish_roundtrip 5, publisher_confirms 5, reconnect 4, topology 7 = 28 prior integration, all green, EXIT 0; no leftover `rmq-itest-*` containers.
- Gates: `target/logs/task4.1-fmt.log` fmt exit 0; `target/logs/task4.1-clippy.log` `cargo clippy -p camel-component-rabbitmq --all-targets --all-features -- -D warnings` exit 0; `target/logs/task4.1-xtask-lints.log` — metric-labels 0, log-redaction 0, log-levels 0 (strict), unwrap 0, secrets 0, non-exhaustive 0, cancel-tokens `7 = max 7`, test-sleep `471 = max 471`, unbounded-wait `295 < max 296`, `schema --check` OK (all ratchets unchanged). Disk stayed at 67 % on `/home/shared`. Component-level clippy only: the shared seam (`connection.rs`) is a crate-private visibility change and no public API crosses a crate boundary.
- Scope: no commit/merge/rebase/push; checkbox stays unchecked. No P5/4.2/4.3/4.4 behavior.
- **Review fix (r_glm, 2026-10-09): lane-before-register leak.** IMPORTANT finding: the original code registered the correlation BEFORE awaiting the mandatory lane, so a caller cancelled while waiting for the lane leaked an entry (no `ConfirmGuard`/`RemoveOnDrop` was armed yet). Fixed by extracting the production `admit(table, retired, lane, id, bound)` seam in `reply.rs` (used by `publish_in_out`) that acquires the bounded lane FIRST, then registers; lane timeout registers nothing, and a retired-before-registration state removes its entry and returns `Disconnected` immediately (MINOR finding). Registration, the retired re-check, and the `ConfirmGuard` arm now have no `.await` gap. `producer.rs` tests moved out-of-line to `src/producer/tests.rs` (P2 pattern; production 760 lines). `ReplyPayload.headers` doc aligned to the actual `delivery.redelivered` flag (no functional change).
- Review-fix test (deterministic, no broker): `mandatory_admission_cancel_leaves_table_empty` (reply unit) holds the lane, polls `admit` once (parks), drops the future, asserts `table.len() == 0`; then frees the lane and asserts the table is reusable, then retires the state and asserts `Admission::Retired` + `len == 0` (pins the MINOR fix). Behavioral RED (pre-fix ordering temporarily restored): `target/logs/task4.1-admissionfix-red.log` — `0 passed; 1 failed` (`left: 1, right: 0`, the leaked entry). GREEN: `target/logs/task4.1-admissionfix-lib.log` — `66 passed; 0 failed` (`RABBITMQ_ITEST=1`, 65 + the one new test). Fresh gates: `target/logs/task4.1-admissionfix-fmt.log` fmt exit 0; `target/logs/task4.1-admissionfix-clippy.log` clippy exit 0; `target/logs/task4.1-admissionfix-xtask-lints.log` ratchets unchanged (cancel 7=max 7, test-sleep 471=max 471, unbounded-wait 295<max 296) + all others 0 + `schema --check` OK. No allow markers added; no metadata APIs invented. Prior 28 Docker not re-run: the request path changed only in the InOut admission (InOnly unchanged), and `clippy --all-targets` recompiled every integration target.

- [x] 4.1

### crates/components/camel-rabbitmq (late replies)

#### Task 4.2: Late-reply drop and Drop-based cleanup

**Files:**
- `crates/components/camel-rabbitmq/src/reply.rs` (modified)
- `crates/components/camel-rabbitmq/src/producer.rs` (modified)

**Steps:**
1. Reply-consumer loop: on delivery, `table.resolve(correlation_id, payload)`; resolve=false → debug log + drop.
2. `pub(crate) struct ReplyState { table: CorrelationTable, loop_cancel: CancellationToken, loop_handle: JoinHandle<()> }` held by the producer as `Arc<ReplyState>`; `impl Drop for ReplyState` cancels the reply loop, joins it, and drains the table (dropped senders close receivers) — the mqtt `DriverHandle` Drop shape (`crates/components/camel-mqtt/src/producer.rs:71`); there is no producer stop hook in the Endpoint trait, so Drop owns cleanup.
3. Timeout path removes the entry at timeout fire (inside `await_reply`), not at resolve attempt.

**Tests:**
- `late_reply_dropped_and_table_empty` (unit): register, `await_reply` times out, then resolve → false AND `table.len() == 0` (gates "late reply is dropped")
- `reply_state_drop_drains_table` (unit): 3 pending entries → drop the ReplyState → all receivers closed, len 0, loop handle finished
- command: `cargo test -p camel-component-rabbitmq --lib`
- expected: pass

**Acceptance:**
- No sender/receiver leak (unit-asserted); clippy/fmt clean

**Worker ledger (task 4.2, 2026-10-09):**
- Files: `src/reply.rs` (core/session split + 3 tests), `src/producer.rs` (slot/guard/InOut accesses retargeted), this ledger. No other source touched.
- Construction seam (minimal, type composition): `ReplyState` is now the transport-free logical core (`table` + `publish_lane` + `retired` + `loop_cancel` + `loop_abort`) with a private `ReplyState::spawn<S: Stream<..>>(stream, Option<CancellationToken>)`; the lapin channel moved into a new `ReplySession { state, channel }` owned by the producer (`ReplySlot = Mutex<Option<Arc<ReplySession>>>`). `ReplyState`'s `Drop`/`retire_sync` are unchanged (cancel child token + `loop_abort.abort()` + `table.clear()`, mqtt shape, no blocking join); `ReplySession` exposes `channel()/table()/retired()/publish_lane()/is_usable()/retire_sync()/retire_bounded()`. No `channel: Option<..>` branch, no public hook, no new pub/trait abstraction, no mock framework. The reply loop is transport-free (captures only `Arc<CorrelationTable>` + `Arc<AtomicBool>` + `Option<CancellationToken>`, never `Self`); the channel closes via lapin's `ChannelCloser` on the last clone, and `retire_bounded` still closes it bounded. Timer-fire removal (4.2 step 3) and the `Disconnected` mapping for drained receivers were already correct from 4.1 and are unchanged.
- Tests (exact names): `late_reply_dropped_and_table_empty` (register → `await_reply` 50 ms `start_paused` → `ReplyTimeout`, then `resolve == false` and `len == 0`); `reply_state_drop_drains_table` (transport-free `ReplyState::spawn(futures::stream::pending(), None)`, 3 registered, `drop`, all 3 receivers `Err` under `timeout(1s)`, `len == 0`, loop `abort.is_finished()` under a `timeout(1s)`/`yield_now` reap — no sleeps, no marker false-exemption); `await_reply_cancel_removes_entry` (normative G-7 cleanup regression: register, poll `await_reply` once `Pending`, `drop(Box::pin(..))`, `len == 0` and `resolve == false`). 4.1 lib had 66 tests; 66 + 3 = 69.
- RED (genuine compile RED, not faked): `target/logs/task4.2-red-lib.log` — EXIT 101, `E0599 no associated function named spawn` at `reply.rs` `ReplyState::spawn` (the transport-free construction seam did not exist; because the cleanup behavior was already correct from 4.1, no behavioral mutation was fabricated).
- GREEN: `target/logs/task4.2-green-lib.log` — 69 passed; 0 failed. Activated `target/logs/task4.2-docker-lib.log` — 69 passed (`RABBITMQ_ITEST=1` ran `concurrent_mandatory_returns_are_isolated`).
- Integration regression (prod loop structure changed): `target/logs/task4.2-docker-tests.log` — prior InOnly integration **28 across all 6 bins** (consume_delivery 6, consumer_readiness 1, publish_roundtrip 5, publisher_confirms 5, reconnect 4, topology 7 = 28; a tail-read shows only the final bin's topology 7), all green, EXIT 0; no leftover `rmq-*` containers.
- Gates: `target/logs/task4.2-fmt-final.log` (full UTC/ENV/CMD/df/EXIT metadata; `cargo fmt --all --check` exit 0 with empty stdout; the earlier bare-redirect `target/logs/task4.2-fmt.log` is preserved at 0 bytes); `target/logs/task4.2-clippy.log` `cargo clippy -p camel-component-rabbitmq --all-targets --all-features -- -D warnings` exit 0; `target/logs/task4.2-xtask-lints.log` — metric-labels 0, log-redaction 0, log-levels 0 (strict), unwrap 0, secrets 0, non-exhaustive 0, cancel-tokens `7 = max 7`, test-sleep `471 = max 471`, unbounded-wait `295 < max 296`, `schema --check` OK (all ratchets unchanged).
- Scope: no commit/merge/rebase/push; checkbox stays unchecked. No P5/4.3/4.4 behavior; no RPC integration.

- [x] 4.2

### crates/components/camel-rabbitmq (consumer reply)

#### Task 4.3: Consumer-side reply publishing

**Files:**
- `crates/components/camel-rabbitmq/src/consumer.rs` (modified)
- `crates/components/camel-rabbitmq/src/headers.rs` (modified)

**Steps:**
1. Inbound mapping (2.4) already surfaces `replyTo` and `correlationId`. Extract `#[async_trait] pub(crate) trait ReplyPublisher { async fn publish_reply(&self, reply_to: &str, correlation_id: &str, body: Vec<u8>, props: BasicProperties) -> Result<(), CamelError>; }` implemented for `lapin::Channel` (unit fakes implement it); plus `pub(crate) fn maybe_reply(route_result: &Result<Exchange, CamelError>, exchange_headers: &camel_api::Headers, out_body: Vec<u8>, publisher: &dyn ReplyPublisher) -> Option<ReplyIntent>` deciding whether a reply is warranted (Ok + replyTo + correlationId present).
2. After the route completes Ok AND `maybe_reply` yields an intent, the consumer publishes the OUT body/headers to `replyTo` with `correlationId` (properties via headers.rs outbound, delivery mode 1 — replies are transient), on a channel from the manager, under the bounded disconnected guard.
3. Route failure → NO reply publish (disposition path unchanged; `maybe_reply` returns None on Err).
4. Replies publish WITHOUT confirm wait (direct-reply-to limitation).

**Tests:**
- `reply_sent_after_route_success` (unit): fake ReplyPublisher recorder; route Ok + replyTo/correlationId headers → exactly one `publish_reply` with the correlation id and out body
- `no_reply_on_route_failure` (unit): route Err → zero publishes (gates "failed route sends no reply")
- `no_reply_without_headers` (unit): Ok but no replyTo → zero publishes
- command: `cargo test -p camel-component-rabbitmq --lib`
- expected: pass

**Acceptance:**
- lib tests green

**Worker ledger (task 4.3, 2026-10-09):**
- Files: `src/consumer/reply.rs` (new, 211 lines — cohesive module: `ReplyPublisher` trait + `impl for lapin::Channel`, `ReplyIntent`, pure `maybe_reply`, `send_reply`, `send_reply_on_managed_channel`), `src/consumer.rs` (964 lines; module decl + reply block, generic `disposition<T>`), `src/consumer/tests.rs` (3 new tests + `RecordingReplyPublisher`), `src/producer.rs` (`body_to_bytes` widened to `pub(crate)` so both directions use ONE converter), this ledger. No new module API, no test hook.
- Signature split (G-10): `maybe_reply(route_result: &Result<Exchange, CamelError>, original_headers: &Headers) -> Option<ReplyIntent>` is pure; `async fn send_reply(intent, publisher: &dyn ReplyPublisher)` does the one `basic_publish`. The plan's unused `publisher` param on the pure fn is gone (no `unused_variables`). r_glm preparation fix: the old `reply_body` caller-side pre-materialized the body for ALL messages before the header check; that is folded in so the body is materialized ONLY after the route is `Ok` AND both original reply headers are present (no allocation for an InOnly route without reply headers), the reply OUT-or-IN message is selected exactly ONCE, body and headers come from that same message, and the shared `body_to_bytes` is preserved. A body the converter cannot materialize yields no intent (same behavior, no new error policy). run_loop is now ONE `maybe_reply` call.
- Transport: `impl ReplyPublisher for lapin::Channel` publishes on the DEFAULT exchange (`""`), routing key = `replyTo`, `mandatory=false`, `delivery_mode=1` (transient), correlation from the ORIGINAL request. It awaits the no-confirm `BasicPublishFuture` (resolves `NotRequested`; no broker ack wait) so the bounded close cannot race the frame. Production acquires a plain channel from the manager via `consumer_channel()` (NO `confirm_select` — outside the cached producer confirm path; bounded by the existing `CHANNEL_OPEN_BOUND`), captures the origin BEFORE the open await and lets the manager demote only a dead origin; `CHANNEL_CLOSE_BOUND` closes that reply channel bounded after the publish. G-10 names "the engine's own consume channel"; this implementation uses a SEPARATE plain manager channel because a REPLIER may publish on any channel/connection (only the requester's direct reply-to channel is rule-bound, 4.1), so a reply publish failure can never reset the healthy shared connection.
- Header policy (original vs OUT): `replyTo`/`correlationId` are read from the ORIGINAL request headers snapshotted BEFORE `ctx.send_and_wait` (a route can never redirect a reply or its UUID). The reply message is the route OUT message when present, else the IN message — selected exactly ONCE — and the reply body AND headers both come from that same message (the shared `body_to_bytes` converter). `correlationId` is enforced AFTER `headers::outbound` (route-set value cannot override).
- Failure policy (G-10, verbatim from `/home/shared/rust-camel-worktrees/349-rabbitmq/target/logs/p4-plan-rebless-selfgrill.md:56`): "A reply publish failure logs `warn` and leaves the disposition unchanged (direct reply-to replies are not fault-tolerant per the RabbitMQ caveat)." Implementation trace: route `Ok` + a reply attempt fails → `rt.metrics().increment_errors(route_id, "b-prime:rabbitmq:reply-publish")` + `tracing::warn!` (no credentials), then FALLS THROUGH to the normal route disposition — `disposition(&Ok(route), requeue)=Disposition::Ack`; the post-ack consume `Success` is recorded iff the ack actually applied. No `EngineExit` variant, no reconsume, no disposition change. Business route `Err` → NO reply + normal Nack (`requeueOnFailure` unchanged); Ok with missing `replyTo`/`correlationId` → NO reply + normal Ack; `ChannelClosed` → NO reply/no ack (existing early exit). If the connection is actually dead, the Ack naturally fails and the existing stale-generation/redelivery infrastructure policy applies. The send is bounded (`REPLY_SEND_BOUND` 10 s) and cancel-aware (`tokio::select!` on the engine token), so stop still joins promptly. The four bless artifacts (`p4-plan-rebless-{prev-bless.json,hash.log,selfgrill.md,tasks.diff}`) exist in this WT's `target/logs`.
- Correction: the first version of this ledger implemented a "leave unacked / close own consume channel" strategy. That was the CONDUCTOR's own earlier suggestion, NOT r_glm guidance (the reviewer had not been called) and had no master/expert bless; it is superseded and reverted to the fresh BLESS e_opus G-10 above. No architecture ruling, accepted-risk record, or invented reviewer directive remains.
- Tests: `reply_sent_after_route_success` (Ok + original `replyTo`/`correlationId`, actual Exchange with OUT body `b"out"` ≠ IN, OUT `correlationId=route-override` → exactly 1 `publish_reply`, recorded `("amq.rabbitmq.reply-to.g1","c1",b"out")`, `delivery_mode==1`, props correlation `c1` wins; plus the G-10 failure branch in the SAME test — fake publisher returns Err → helper `send_reply` returns Err while `disposition(&Ok(route))==Ack`); `no_reply_on_route_failure` (route `Err` + both headers → `None`, 0 publishes); `no_reply_without_headers` (Ok + only `correlationId` → `None`, 0 publishes). All 3 call the 2-arg pure `maybe_reply`; the expected body is the real OUT body, not mirrored. The fake calls the real production `maybe_reply`/`send_reply`; no test-mirrored logic, no type-level fake channel, no production hook, no new test function. The earlier tautological `"b-prime:…".split(':').count()` assertion was removed (lint-metric-labels already pins the label grammar).
- RED (genuine behavioral): `target/logs/task4.3-red.log` — `0 passed; 1 failed` (`reply_sent_after_route_success`, `left: Some("route-override") right: Some("c1")`) with the correlation enforcement temporarily removed; restored immediately.
- GREEN (latest `preparationfix` artifact): `target/logs/task4.3-preparationfix-lib.log` — `72 passed; 0 failed` (69 baseline + 3 new); `target/logs/task4.3-guidancefix-docker-lib.log` — `72 passed` with `RABBITMQ_ITEST=1` (`concurrent_mandatory_returns_are_isolated`), no leftover containers (the preparation fix is a pure decision-path refactor and the docker-gated lib test is reply-agnostic). No unchanged-28 integration rerun: no existing integration binary exercises the reply RPC path (no prior live RPC; 4.4 owns the live request/reply tier). The earlier before-fix logs (`task4.3-green-lib.log`, `task4.3-docker-lib.log`, `task4.3-docker-tests.log`, `task4.3-clippy.log`, `task4.3-xtask-lints.log`, `task4.3-fmt-final.log`, `task4.3-guidancefix-*`) are preserved as honest history.
- Gates (latest `preparationfix` artifact): `target/logs/task4.3-preparationfix-fmt.log` fmt exit 0; `target/logs/task4.3-preparationfix-clippy.log` `cargo clippy -p camel-component-rabbitmq --all-targets --all-features -- -D warnings` exit 0; `target/logs/task4.3-preparationfix-xtask-lints.log` — log-redaction 0, log-levels 0 (strict), metric-labels 0 (pins the `b-prime:rabbitmq:reply-publish` grammar), non-exhaustive 0, cancel-tokens `7 = max 7`, test-sleep `471 = max 471`, unbounded-wait `295 < max 296`, `schema --check` OK (all ratchets unchanged).
- Scope: no commit/merge/rebase/push; checkbox stays unchecked. No P5/4.4 behavior.

- [x] 4.3

### crates/components/camel-rabbitmq (req/rep e2e)

#### Task 4.4: Request/reply docker integration suite

**Files:**
- `crates/components/camel-rabbitmq/tests/request_reply.rs` (new)
- `crates/components/camel-rabbitmq/src/metadata.rs` (modified)

**Steps:**
1. `tests/request_reply.rs` with the fixture: (a) `reply_round_trip` — replier route `from rabbitmq:default?queue=rpc` echoing `Hello ` + body; requester sends InOut → resolves with the echoed body (gates "reply round trip"); (b) `reply_timeout_docker` — no replier, `replyTimeout=2000` → Err `ReplyTimeout` (Display contains `reply`) within 3 s; (c) `late_reply_dropped_docker` — replier delayed past `replyTimeout=1000` via a held envelope; requester errs; the delayed reply arrives late; then a FOLLOW-UP InOut on the same producer with a fresh replier resolves with ITS OWN body — the late reply was dropped, not misrouted (no pub(crate) table probe from an integration test); (d) `failed_route_sends_no_reply_docker` — replier envelope answered Err → requester times out (never resolves with a body).
2. Freeze `P4_OPTIONS` = P1 ∪ P2 ∪ P3 ∪ {replyTimeout}; parity test final.

**Tests:**
- `reply_round_trip`, `reply_timeout_docker`, `late_reply_dropped_docker`, `failed_route_sends_no_reply_docker` (docker; gate the four P4 scenarios)
- `metadata_parity_p4_frozen` (unit)
- command: `RABBITMQ_ITEST=1 cargo test -p camel-component-rabbitmq --test request_reply && cargo test -p camel-component-rabbitmq --lib`
- expected: pass

**Acceptance:**
- Docker tier green; parity frozen; no reply misrouting

**Worker ledger (task 4.4, 2026-10-09):**
- Files: `crates/components/camel-rabbitmq/tests/request_reply.rs` (new; exactly four live `#[tokio::test]` functions: `reply_round_trip`, `reply_timeout_docker`, `late_reply_dropped_docker`, `failed_route_sends_no_reply_docker`), `crates/components/camel-rabbitmq/src/metadata.rs` (parity test renamed `metadata_parity_p4_partial` → `metadata_parity_p4_frozen`; the expected set is still derived from `P1_OPTIONS ∪ P2_OPTIONS ∪ P3_OPTIONS ∪ P4_OPTIONS`, not restated). No production test hook and no `pub(crate)` correlation-table seam: the tests observe only the public component surface plus the shared recording tracing sink the tier already installs.
- Fixture: one `require_fixture()` per test (own `rabbitmq:3.13-alpine` container) with unique per-process queue names, consistent with the existing tier. With `RABBITMQ_ITEST` unset the explicit notice is printed and each test returns 0.
- Live scenarios. (1) `reply_round_trip` is a table over `mandatory=false|true` inside the ONE function (no extra test names): a real `RabbitConsumer` replier on `rabbitmq:default?queue=…` answers the `ConsumerContext` envelope `Ok` with an OUT `Hello <body>` plus an `x-which` header; the requester's `Exchange::new_in_out` resolves with the echoed body, the `x-which` OUT header, and the ORIGINAL request `correlationId` (captured at the replier). (2) `reply_timeout_docker`: declared queue, no replier, `replyTimeout=2000` → `Err` whose Display contains `reply`, elapsed `< 3 s`. (3) `late_reply_dropped_docker`: the first envelope is HELD until the requester errors (`replyTimeout=1000`), then released with an obviously different `OLD-REPLY` body; a real `LogCapture` barrier on `"rabbitmq late reply dropped"` (bounded `timeout` + `yield_now`, no settling sleep) proves the late reply was published and dropped before the follow-up; the SAME producer's follow-up resolves with `Hello new` + `x-which=new` and its OWN correlation id (≠ the first), so the late reply did not misroute. (4) `failed_route_sends_no_reply_docker`: the replier answers `Err` → no reply, and the requester (`replyTimeout=1500`) errors within `< 3 s` without a body. Every RPC call site is wrapped in a 10 s `tokio::time::timeout`; spawned replier tasks are `AbortOnDrop` with a 2 s reap.
- Evidence (this WT's `target/logs`; each log carries UTC/CMD/df/EXIT): `task4.4-compile.log` exit 0; `task4.4-ungated.log` `4 passed` (notice path, exit 0); `task4.4-live.log` `4 passed` in 21.64 s with `--nocapture` (`RABBITMQ_ITEST=1 --test-threads=1`; the earlier `task4.4-live-run1.log` `4 passed` in 21.99 s is retained); `task4.4-lib.log` `72 passed` (`metadata_parity_p4_frozen` ok); `task4.4-lib-docker.log` `72 passed` (activated lib); `task4.4-integration-all.log` `32 passed` across the seven bins on the final revision (6+1+5+5+4+4+7 = 28 prior + 4 new), no leftover `rmq-itest-*`; `task4.4-clippy.log` exit 0 (`--all-targets --all-features -D warnings`); `task4.4-fmt-clippy.log` fmt-apply/fmt-check/clippy all exit 0 on the final revision; `task4.4-fmt.log` exit 0; `task4.4-xtask-lints.log` log-redaction 0, log-levels 0 (strict), metric-labels 0, non-exhaustive 0, cancel-tokens `7 = max 7`, test-sleep `471 = max 471`, unbounded-wait `295 < max 296`, `schema --check` OK. Disk held at 73 % (guard 78 %); no broad workspace gate re-run (no shared seam changed).
- Measured live RPC proof (`task4.4-live.log`, `--nocapture`): `reply_timeout_docker` elapsed `2.012643748s` (< 3 s) error `… reply timed out after 2000 ms …`; `failed_route_sends_no_reply_docker` elapsed `1.506048256s` (< 3 s) error `… reply timed out after 1500 ms …`; `late_reply_dropped_docker` first request elapsed `1.006761385s`, late-drop barrier `352.308618ms`, `first_corr=8ca42ca3-…` vs `fresh_corr=8a9d7e3b-…` (distinct UUIDs), follow-up body `Hello new` — the late reply was dropped, not misrouted.
- No RED claim: the 4.1–4.3 production paths already exist, so the new live tier is expected green; no behavioral mutation was fabricated to manufacture a red, and the test file did not exist before so there was no prior test to fail.
- Metadata clarification: the conductor asked whether an existing metadata capability could advertise exchange patterns. `ComponentCapabilities` has only the four role/streaming booleans; it has no exchange-pattern field or builder. This is not a spec mismatch or an open requirement. Runtime `ExchangePattern::InOut` behavior and frozen URI-option parity satisfy the plan. No shared API change or RabbitMQ `exchangePattern` URI option is needed. `RabbitError` stays at eight variants, including `ReplyTimeout`.
- Test-shape clarification: the late-reply scenario keeps the same live replier for the follow-up rather than starting a fresh consumer. It receives a fresh envelope and distinct correlation ID after the late-drop barrier, then returns the follow-up's own body and header. This preserves the planned fresh-response proof while keeping the original replier alive to inject its late response; the requester producer is unchanged.
- Policy unchanged: the 4.3 G-10 reply-publish-failure policy (b-prime warn, disposition unchanged) is untouched; the failed-route scenario exercises "no reply at all", never a reply-publish failure.
- Scope: only the two task files plus this ledger; no commit/merge/rebase/push; no P5. Checkbox stays unchecked.

- [x] 4.4

**P4 boundary acceptance (2026-10-09):**
- `r_glm` returned **APPROVE** for the complete `bc5f3326...e1c93bc7` diff. Tasks 4.1–4.4 and the Phase 4 exit criteria are satisfied; no unresolved P4 implementation finding remains.
- Retained current-code verification: 72 library tests, including one live broker test, and 32 integration tests pass. Live RPC tests cover mandatory on/off, bounded timeout, late-reply rejection with a fresh correlation on the same producer, and failed-route no-reply behavior. Component clippy and formatting pass. Fresh boundary checks pass 16 bundle tests, 19 CLI feature-profile tests, 15 local lints, schema, and strict OpenSpec validation; 21 of 24 tasks are complete.
- `lint-publish-registration` remains expected Case B for the unpublished crate. Owner first publication and trustpub registration are pending; no globally-green gate claim is made. Full workspace tests/build, audit, full documentation build, remote commit lint, and fresh broad workspace clippy remain unrun. Prior P3 workspace-clippy evidence is not represented as a fresh P4 run.
- P5 documentation follow-up, owned by task 5.1 and `rc-hpk35`: if the shared body converter cannot materialize an OUT body, reply preparation returns no intent and the requester times out without a new consumer-side signal. Record this limitation in the README/CONTEXT divergences table. The reviewer explicitly requests no P4 behavior change; this note is not an accepted-risk ruling.
- Park at the Phase 4 boundary. Phase 5, landing, and publication require a master ruling. The existing `rc-2bofp` spec/ruling reconciliation remains pending before completion/archive.

**Pre-P5 spec reconciliation ledger (aws-lc scope, 2026-10-09):**
- Provenance: this records the master ruling of 2026-10-08 (task 1.3 ledger, "Master ruling 1"). It is not a new decision. The master session is the human's conductor session (`.opencode/fleet/README.md:17-19`). Evidence: `rc-hpk35` comments `ca84cb11` (escalation: whole-component graph vs lapin-only constraint) and `143ac1ee` ("master aws-lc ruling ... applied"). The P1 park report `.opencode/fleet/inbox/processed-349-rabbitmq-parked-p1.json` records task 1.3: "master authorized component-owned aws-lc scope (rc-2bofp)". `processed-349-rabbitmq-parked-diskpause.json` records: "RabbitMQ-selected lapin features introduce no new aws-lc edge". The master's P5 order restates the ruling. `rc-2bofp` records that the earlier "ring, never aws-lc" mission guidance was component-scoped, never a workspace ruling.
- Normative change: the spec requirement "Connection management" and the scenario now named "component-selected TLS stack adds no aws-lc edge" replace the whole-graph `cargo tree -i aws-lc-rs` gate. The new check: lapin is selected with exactly `tokio`/`rustls--ring`/`rustls-native-certs`, and no feature in the lapin chain selects the rustls `aws_lc_rs`/`aws-lc-rs`/`default` features. aws-lc-rs that shared dependencies already make reachable stays outside the gate and is tracked in `rc-2bofp`. `design.md` (approach paragraph and the P1 exit scenario name) and the proposal risk budget were aligned. TLS behavior is unchanged: a custom `tls` section is still rejected at boot (`rc-l8ohw`), and `amqps://` with system roots still works. The scenario reuses the two `cargo tree -e features` commands from the accepted task 1.3 acceptance, re-run 2026-10-09 with the expected result (`target/logs/p5-spec-plan-rebless-cargo-tree.log`). No new fixture or gate.
- The contradiction is resolved, not accepted as a risk. Phase 5 task blocks 5.1-5.3 are unchanged. The 5.1 README "TLS (ring) note" must describe the component's ring selection and must not claim that the raw dependency tree has no aws-lc.
- Blessing: `experts/e_opus` ran a fresh spec blessing and a fresh plan blessing of these artifacts. The verdicts and hashes are in the worktree-root `.bless.json`, which keeps the full history.
- Editorial amendment (r_glm APPROVE, two minor findings, fixed before the final re-bless). (1) The scenario THEN no longer says that each printed aws-lc-rs path "enters only through shared dependencies". Cargo feature unification also prints paths through lapin's ring/std features. The scenario now states that no component-selected enabling edge turns on aws-lc, and that the enabling features come from shared dependencies outside the gate (`rc-2bofp`). (2) The design approach paragraph now names exactly the three lapin features `tokio`, `rustls--ring`, `rustls-native-certs`. No change to the gate or the scope.

## Phase 5: Docs + hardening

### docs + repo docs

#### Task 5.1: README, component CONTEXT.md, CONTEXT-MAP entry

**Files:**
- `crates/components/camel-rabbitmq/README.md` (modified — full)
- `crates/components/camel-rabbitmq/CONTEXT.md` (new)
- `CONTEXT-MAP.md` (modified)

**Steps:**
1. README (camel-jms README as shape): scheme, Camel.toml brokers block, URI options table (phase-final set with defaults — must equal the frozen P4 parity set exactly), delivery semantics (ack-after-route, reject-no-requeue default, `requeueOnFailure` warning), auto-declare divergence callout (incl. default-exchange skip), request/reply example, docker test tier invocation (`RABBITMQ_ITEST=1`), at-least-once + duplicate note, TLS (ring) note.
2. Component CONTEXT.md (shape of camel-kafka/camel-jms CONTEXT.md): purpose, key design decisions (generation counter, no auto-recover, confirms-always-on, direct reply-to), seams (RabbitConnectionManager, CorrelationTable, DeliveryAcker, ReplyPublisher), divergences vs kafka/mqtt crash-redelivery.
3. CONTEXT-MAP.md: extend the Components section's example bullet list (line ~11) with a one-line rabbitmq entry in the style of the kafka/mqtt/jms bullets, linking the new CONTEXT.md. There is no separate per-component index in CONTEXT-MAP.

**Tests:**
- `cargo xtask lint-context-citations` exits 0
- README options table names == frozen P4 parity set (reviewer assert; cite parity test)
- command: `cargo xtask lint-context-citations`
- expected: pass

**Acceptance:**
- Gate green; README table matches the parity set exactly

- [x] 5.1

**Task 5.1 ledger (2026-10-09):**
- Files: `crates/components/camel-rabbitmq/README.md` (full rewrite), `crates/components/camel-rabbitmq/CONTEXT.md` (new), `CONTEXT-MAP.md` (one Components bullet linking the new CONTEXT.md).
- README URI-option table: 15 rows, the frozen P4 parity set (`P1_OPTIONS ∪ P2_OPTIONS ∪ P3_OPTIONS ∪ P4_OPTIONS`). Names and defaults were checked against `crates/components/camel-rabbitmq/src/config.rs` and the `RabbitMqMetadataDescriptor` in `crates/components/camel-rabbitmq/src/metadata.rs` (`metadata_parity_p4_frozen`). No new option, no pattern URI flag, no TLS URI parameter.
- Test: `cargo xtask lint-context-citations` exits 0 (worktree target, `TMPDIR=/home/shared/tmp`, `CARGO_BUILD_JOBS=4`, df guard 73%). Log: `target/logs/task5.1-lint-context-citations.log`.
- TLS note (G-1): describes the component's lapin `rustls--ring` + `rustls-native-certs` selection and the boot rejection of a custom `tls` section (rc-l8ohw). It does not claim the raw dependency tree has no aws-lc; shared workspace reachability stays tracked in rc-2bofp.
- P4 binding note (G-2): the README and CONTEXT divergences tables record that an unmaterializable OUT body yields no reply intent and a requester timeout, with no new consumer-side signal.
- Checkbox stays unchecked; no commit, merge, rebase, push, or bd close. P5.2 and P5.3 remain open.

#### Task 5.2: mdBook guide page and runnable example

**Files:**
- `docs/src/components/rabbitmq.md` (new — mirrors `docs/src/components/jms.md` / `kafka.md`)
- `docs/src/SUMMARY.md` (modified — entry under the Messaging brokers section, ~lines 68-71)
- `docs/src/components/brokers.md` (modified — add rabbitmq to the broker components overview if that page enumerates them)
- `examples/rabbitmq-example/` (new — following `examples/jms-example` layout: Cargo.toml, README, route file; workspace example member registration matching jms-example)

**Steps:**
1. Guide page: broker config, producer route, consumer route with DLX, request/reply snippet, docker fixture instructions, divergence notes (mirror `docs/src/components/jms.md` structure).
2. Example crate: timer → `rabbitmq:default?queue=demo` producer demo + consumer route echoing to log, broker URL via env (docker fixture), registered exactly as `examples/jms-example` is.

**Tests:**
- `cargo build -p rabbitmq-example` exits 0
- mdBook gate: `mdbook build docs && mdbook test docs` exits 0 (the docs CI gate, `.github/workflows/docs.yml:67-68`)
- command: `cargo build -p rabbitmq-example && mdbook build docs && mdbook test docs`
- expected: pass

**Acceptance:**
- Builds green; example follows jms-example structure; SUMMARY lists the page

- [x] 5.2

**Task 5.2 ledger (2026-10-09):**
- Files: `docs/src/components/rabbitmq.md` (new), `docs/src/SUMMARY.md` (RabbitMQ entry under Messaging brokers), `docs/src/components/brokers.md` (RabbitMQ bullet), `examples/rabbitmq-example/{Cargo.toml,README.md,src/main.rs}` (new), `Cargo.lock` (+`rabbitmq-example` package entry). No root `Cargo.toml` change: the `examples/*` member glob registers the crate.
- Guide shape mirrors `docs/src/components/jms.md`/`kafka.md`: schemes/scope note, example, URI table (the 15-row frozen P4 parity set), broker config, consumer, producer, delivery semantics (ack-after-route, reject-no-requeue default, `requeueOnFailure` hot-loop warning, DLX via `queueArguments`), auto-declare divergence (default `false` vs Camel `spring-rabbitmq` `true`), programmatic request/reply (`replyTimeout`), headers, TLS (ring), Docker fixture, limitations. No cross-language bridge (native `lapin`).
- Snippets use the existing `rust,ignore` `{{#include ...:anchor}}` convention for route anchors and the programmatic request/reply block; `bash`/`yaml`/`text`/`toml` blocks carry non-Rust content. The example crate itself is compiled by `cargo build -p rabbitmq-example`.
- Example runtime config: consumer `rabbitmq:default?queue=demo&autoDeclare=true` → `log:info?showHeaders=true`; producer `timer:tick?period=1000` → `set_body` JSON → `rabbitmq:default?queue=demo` (default exchange, routing key falls back to `demo`). Broker URL from `RABBITMQ_URL`, default `amqp://rmq:rmq@127.0.0.1:5672/%2f`.
- Docker fixture (loopback only): `rabbitmq:3.13-alpine`, `-p 127.0.0.1:5672:5672`, `-e RABBITMQ_DEFAULT_USER=rmq`, `-e RABBITMQ_DEFAULT_PASS=rmq`. Explicit ephemeral demo credentials, not a production secret; `RABBITMQ_URL` overrides.
- TLS note (rc-l8ohw/rc-2bofp): describes the component's `rustls--ring` + `rustls-native-certs` selection and the boot rejection of a custom `tls` section; does not claim the raw dependency tree has no `aws-lc-rs`.
- Tests: `cargo build -p rabbitmq-example` exits 0 (`target/logs/task5.2-build.log`); `mdbook build docs` exits 0 (`target/logs/task5.2-mdbook-build.log`); `mdbook test docs` exits 0 (`target/logs/task5.2-mdbook-test.log`, RabbitMQ chapter tested at line 70). Extra: `cargo fmt -p rabbitmq-example -- --check` exits 0 (`target/logs/task5.2-fmt.log`, df 77% before/after).
- Checkbox stays unchecked; no commit, merge, rebase, push, or bd close. P5.3 remains open.

#### Task 5.3: Full gate sweep and hardening

**Files:**
- any file gate findings require (modified)

**Steps:**
1. Run the full AGENTS.md QUALITY GATES list in the worktree (minus `lint-commits`) plus conductor additions: `cargo build --workspace`, `cargo test --workspace --lib`, `cargo test -p camel-core --test hexagonal_architecture_boundaries_test`, and `cargo test -p camel-component-rabbitmq --no-fail-fast` covering every `tests/*.rs` binary WITH `RABBITMQ_ITEST=1` (docker tier) and once WITHOUT (notice path).
2. Run the xtask lints: lint-unwrap, lint-secrets, lint-single-source, lint-non-exhaustive, lint-log-levels, lint-log-redaction, lint-cancel-tokens, lint-test-sleep, lint-unbounded-wait, lint-ignore, lint-publish-cycles, lint-publish-registration, lint-component-deps, lint-gate-forwarding, lint-context-citations, lint-metric-labels, schema --check, doc-build (`RUSTDOCFLAGS="-D warnings" cargo doc -p camel-api -p camel-core -p camel-builder -p camel-dsl -p camel-endpoint --no-deps`), `cargo audit` (new dep tree — lapin/rustls advisories), clippy workspace set from AGENTS.md, feature_profiles, `cargo fmt --check --all`. EXCEPTION: `cargo xtask lint-publish-registration` MUST exit non-zero while `camel-component-rabbitmq` is `new-unpublished` — record exactly one Case B and its owner action (manual classic-token first publish with `CARGO_REGISTRY_TOKEN`, then register trustpub). Do NOT mark the crate `registered` falsely and do NOT reclassify the Case B as a pre-existing exemption or N/A.
3. Fix every finding except the expected `new-unpublished` Case B; re-run until green. Record exit codes in the park file.

**Tests:**
- every gate command exits 0, EXCEPT `cargo xtask lint-publish-registration` which exits non-zero with exactly one expected Case B for `camel-component-rabbitmq`; its owner action (manual first publish, then trustpub registration) is recorded. Never a false `registered` assertion; never a pre-existing-exemption classification.
- command: per-gate commands as listed in AGENTS.md `## QUALITY GATES`
- expected: all green except the single expected `new-unpublished` Case B

**Acceptance:**
- All gates green except the expected `lint-publish-registration` Case B, whose owner action is recorded; gate-coverage self-check enumerates each gate explicitly

**Final verification and mission acceptance (2026-10-09):**
- `r_glm` returned **APPROVE** for task 5.3, the P5 phase, and the full holistic review of `aebbf351..ea31646c`. No unresolved finding remains. Later tracked changes record acceptance only; implementation, documentation, and example sources are unchanged from the reviewed and tested state.
- The exhaustive 34-gate matrix is retained at `target/logs/task5.3-verification-matrix.json`: 32 pass, one expected Case B, and one explicit CI-owned remote skip. Workspace library tests pass 10,633 tests with two ignored; the architecture boundary suite passes 33. RabbitMQ passes 72 library tests and 32 integration tests with `RABBITMQ_ITEST=1`, and its separate unset-environment notice run passes without a live-test claim. CLI feature profiles pass 19 tests.
- All four prescribed clippy commands, workspace build/formatting, documentation build, schema, local lints, example build, and mdBook build/test pass. `cargo audit` reports zero vulnerabilities and six unmaintained informational warnings under the unchanged repository policy: `atomic-polyfill`, `backoff`, `bincode`, `instant`, `paste`, and `rustls-pemfile`.
- `lint-publish-registration` exits 1 with exactly one expected `new-unpublished` Case B. The owner must perform the first publication with a classic `CARGO_REGISTRY_TOKEN`, then register trustpub. This is not a pre-existing exemption or N/A. Remote `lint-commits` was explicitly excluded by the plan/conductor and was not executed; its retained skip log records a null exit, not a passing command.
- Operational incidents require master acknowledgement in the landing report. The gate helper initially invoked workspace build from the main checkout with the worktree target; that run is invalid and was discarded. The authoritative rerun uses the feature worktree, with zero main-source path references. Disk usage transiently reached 81% during the discarded run; main target stayed cold. An earlier expert wrote a 31 KB bd scratch file under forbidden `/tmp` and deleted it. These disclosures do not claim that main Cargo never ran or that the disk threshold was never crossed.
- All 24 tasks are complete. Final mission park hands off the commits, red/green evidence, full matrix, incident disclosures, and publication prerequisites. The master owns landing, publication, and issue closure; the agent performs none of them.

- [x] 5.3
