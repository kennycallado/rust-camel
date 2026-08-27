# Tasks: http-pinned-client-cache

## crates/components/camel-http

### Task 1.1: PinnedClientCache module

**Files:**
- `crates/components/camel-http/src/client_cache.rs` (new)
- `crates/components/camel-http/src/lib.rs` (modified — add `mod client_cache;` next to the existing `mod ssrf;` declaration)
- `crates/components/camel-http/Cargo.toml` (modified — add `moka.workspace = true` to `[dependencies]`)

**Steps:**
1. In `Cargo.toml`, add `moka.workspace = true` (workspace already defines `moka = { version = "0.12", features = ["future"] }`).
2. Create `src/client_cache.rs` with `pub(crate) struct PinnedClientCache` wrapping a private `moka::future::Cache<PinnedKey, reqwest::Client>` field named `cache`, plus a private `build_counter: std::sync::atomic::AtomicU64` field.
3. Define `pub(crate) const PINNED_CLIENT_TTL: std::time::Duration = Duration::from_secs(60);` and `pub(crate) const PINNED_CLIENT_MAX_ENTRIES: u64 = 64;` with a doc comment stating they bound retained pools, TLS-material staleness, and pinned address sets (see design.md §Bounds).
4. Define the private key type `PinnedKey(String, Vec<std::net::SocketAddr>)` deriving `Clone, Eq, PartialEq, std::hash::Hash`, constructed by a private `fn pinned_key(host: &str, addrs: &[std::net::SocketAddr]) -> PinnedKey` that canonicalizes the address set mutably on an owned copy — `let mut canonical = addrs.to_vec(); canonical.sort_unstable(); canonical.dedup();` — before building the key (`sort_unstable` because `SocketAddr`'s `Ord` is total and stability is irrelevant).
5. Implement `pub(crate) fn new(ttl: std::time::Duration, max_entries: u64) -> Self` building the cache via `moka::future::Cache::builder().time_to_live(ttl).max_capacity(max_entries).build()`.
6. Implement `pub(crate) async fn get_or_build(&self, host: &str, addrs: &[std::net::SocketAddr], build: impl FnOnce() -> reqwest::Client) -> reqwest::Client`: build the canonical `pinned_key(host, addrs)`, then `self.cache.get_with(key, async { self.build_counter.fetch_add(1, Ordering::Relaxed); build() }).await`. The `fetch_add` sits inside the init future so only actual builds are counted.
7. Implement `#[cfg(test)] pub(crate) fn build_count(&self) -> u64` returning `self.build_counter.load(Ordering::Relaxed)`. The `#[cfg(test)]` gate keeps `clippy -D warnings` clean in non-test builds; the 1.1 unit tests below assert through `build_count()` so the accessor is exercised by tests here; its non-test-build liveness is covered by Task 1.2's clippy gate once the wiring makes the module's symbols live.
8. Add a `#[cfg(test)] mod tests` in `client_cache.rs` with the unit tests listed below. Use `PinnedClientCache::new(Duration::from_millis(50), 64)` for TTL-sensitive tests, await `cache.run_pending_tasks()` before any `entry_count()` assertion, and assert `entry_count()` with `<=` (moka's count is approximate).

**Tests:** (executable spec — name, setup, action, assert, command, expected)
- `hit_within_ttl_builds_once`: setup — cache with 50 ms TTL; action — two sequential `get_or_build("api.example.com", &[addr_a], &builder)` calls (builder returns `reqwest::Client::new()`); assert — `cache.build_count() == 1`; command — `cargo test -p camel-component-http --lib client_cache`; expected — fails before implementation exists (module absent), passes after.
- `ttl_expiry_rebuilds`: setup — cache with 50 ms TTL, a builder closure returning `reqwest::Client::new()`; action — `get_or_build`, `tokio::time::sleep(Duration::from_millis(80))`, `get_or_build` with the same key; assert — `cache.build_count() == 2` (the expired entry is logically dead and a fresh client is built); command — same as above; expected — passes after implementation.
- `changed_addr_set_builds_new_entry`: setup — two `tokio::net::TcpListener`s on DISTINCT loopback IPs sharing one port: bind listener A on `127.0.0.2:0` (yields port `p`), then bind listener B on `127.0.0.3:p` (if taken, rebind A on a fresh port and retry — practically collision-free on loopback). Each listener runs an accept-loop that records received connections into its own flag and writes `HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok`. Using the same port on different IPs makes the routing assertion independent of whether reqwest takes the port from the URL or from the pinned addresses. A cache; action — `cache.get_or_build("localhost", &[addr_a], || crate::build_client(&HttpConfig::default(), Some(("localhost", &[addr_a]))))` with `addr_a = 127.0.0.2:p`, then the same pattern with `addr_b = 127.0.0.3:p` (each closure is built at its call site and captures its own addr set — `get_or_build`'s zero-arg-closure signature from step 6 is unchanged), then send `client_b.get("http://localhost:{p}/x").send()`; assert — `cache.build_count() == 2`, after `run_pending_tasks().await` `entry_count() == 2`, the request returned 200, ONLY listener B (`127.0.0.3`) recorded a connection (A recorded none — the changed-resolution client connects only to the new validated set); command — `cargo test -p camel-component-http --lib changed_addr_set_builds_new_entry`; expected — passes after implementation.
- `addr_order_normalized`: setup — cache, a builder closure returning `reqwest::Client::new()`; action — `get_or_build("api.example.com", &[addr_a, addr_b], builder)` then `get_or_build("api.example.com", &[addr_b, addr_a, addr_a], builder)` (second call adds a duplicate to exercise dedup); assert — `cache.build_count() == 1` (sorted+deduplicated keys are equal); command — same; expected — passes after implementation.
- `concurrent_same_key_builds_once`: setup — cache, a builder closure returning `reqwest::Client::new()`; action — spawn 8 `tokio::spawn` tasks all calling `get_or_build` with the same key, join all; assert — `cache.build_count() == 1` (moka `get_with` single-flight); command — same; expected — passes after implementation.
- `capacity_bounds_entries`: setup — cache with `max_entries = 2`, a builder closure returning `reqwest::Client::new()`; action — three `get_or_build` calls with three distinct keys, then `run_pending_tasks().await`; assert — `cache.build_count() == 3` and `entry_count() <= 2`; command — same; expected — passes after implementation.

**Acceptance:**
- `cargo fmt --check --all` exits 0.
- `cargo test -p camel-component-http --lib client_cache` passes (6 tests).
- `cargo xtask lint-unwrap` exits 0 (no new `unwrap()`).
- No clippy gate at this task: the module's `pub(crate)` symbols are production-unused until Task 1.2 wires them, so a `--all-targets` clippy here would fail on `dead_code` by construction. Task 1.2's clippy acceptance covers this module once the wiring makes the symbols live.

- [x] 1.1

### Task 1.2: Producer path wiring (structural)

**Files:**
- `crates/components/camel-http/src/lib.rs` (modified)

**Steps:**
1. Add `pub(crate) use client_cache::{PinnedClientCache, PINNED_CLIENT_MAX_ENTRIES, PINNED_CLIENT_TTL};` (or `use` at module level as existing style dictates) next to the existing `ssrf` usage.
2. Add field `pinned_cache: std::sync::Arc<PinnedClientCache>` to `struct HttpEndpoint` (~line 1983).
3. In `HttpComponent::create_endpoint` (~1899) and `HttpsComponent::create_endpoint` (~1964), after the existing `let client = build_client(&self.config, None);`, add `let pinned_cache = std::sync::Arc::new(PinnedClientCache::new(PINNED_CLIENT_TTL, PINNED_CLIENT_MAX_ENTRIES));` and pass `pinned_cache` into both `HttpEndpoint` struct literals.
4. Add field `pinned_cache: std::sync::Arc<PinnedClientCache>` to `struct HttpProducer` (~2042, stays `#[derive(Clone)]` — `Arc` clones).
5. In `HttpEndpoint::create_producer` (~2018), pass `pinned_cache: std::sync::Arc::clone(&self.pinned_cache)` into the `HttpProducer` literal.
6. In `Service<Exchange> for HttpProducer::call` (~2157): bind `let shared_client = self.client.clone();` (the endpoint's unpinned client, kept for the literal path and for the redirect loop's literal hops), add `let pinned_cache = std::sync::Arc::clone(&self.pinned_cache);`, and shadow `let client = if let Some((ref host, ref addrs)) = resolved { pinned_cache.get_or_build(host.as_str(), addrs, || build_client(&http_config, Some((host.as_str(), addrs)))).await } else { shared_client.clone() };` replacing the per-request `build_client` branch (~2186-2190). Update the branch's comment: the pinned client is now cached per validated `(host, addrs)` — per-request SSRF validation and DNS pinning are unchanged.
7. Update the three `HttpEndpoint` struct literals in the test module (~lines 7679, 7697, 7726) to add `pinned_cache: std::sync::Arc::new(PinnedClientCache::new(PINNED_CLIENT_TTL, PINNED_CLIENT_MAX_ENTRIES))`.

**Tests:** (regression only — behavioral tests live in Task 1.3)
- Existing suites must stay green after the wiring: command — `cargo test -p camel-component-http --lib`; expected — all existing tests pass unchanged (no producer-path behavior assertions yet; the `#[cfg(test)] build_count` accessor from Task 1.1 stays exercised by Task 1.1's own tests, so no dead-code lint fires at this gate).

**Acceptance:**
- `cargo fmt --check --all` exits 0.
- `cargo clippy -p camel-component-http --all-targets --all-features -- -D warnings` exits 0.
- `cargo test -p camel-component-http --lib` passes (existing suites unchanged).
- `cargo xtask lint-unwrap` exits 0.

- [x] 1.2

### Task 1.3: Producer-path behavioral tests

**Files:**
- `crates/components/camel-http/src/lib.rs` (modified — test module only)

**Steps:**
1. Add a multi-accept local responder helper `spawn_multi_accept_200()` to the test module: binds `tokio::net::TcpListener` on `127.0.0.1:0`, accepts connections in a loop (unlike the single-accept `start_host_capturing_destination()` at ~7994), writes `HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok` per connection, returns `(base_url, JoinHandle)`.
2. Add the three tests below to the existing `#[cfg(test)]` module. Build endpoints by constructing the `HttpEndpoint` struct literal directly (the ~7679 fixture pattern — same-crate tests reach private fields, unlike `create_endpoint`'s `Box<dyn Endpoint>`), each literal sharing one test-owned `Arc<PinnedClientCache::new(PINNED_CLIENT_TTL, PINNED_CLIENT_MAX_ENTRIES)>` so counts are assertable. Use `tower::ServiceExt::oneshot` and `test_producer_ctx()` as existing tests do. All destination URLs use the `localhost` hostname (a domain name for `resolve_initial_url_for_ssrf`) or the `127.0.0.1` literal, with `allowInternal=true` in the endpoint config so the resolver permits loopback.

**Tests:** (executable spec)
- `producers_share_endpoint_cache`: setup — one `HttpEndpoint` struct literal (base URL `http://localhost:{dest_port}/`, `allowInternal=true` config) whose `pinned_cache` field is one shared `Arc<PinnedClientCache>`, `spawn_multi_accept_200()` on `dest_port`, two producers from `endpoint.create_producer(rt(), &ctx)`; action — each producer sends one `oneshot` exchange whose resolved URL is `http://localhost:{dest_port}/`; assert — both exchanges complete OK and the shared cache's `build_count() == 1` (both producers hit the same cache entry); command — `cargo test -p camel-component-http --lib producers_share_endpoint_cache`; expected — fails if sharing is broken (a second build would make the count 2).
- `producer_repeated_hostname_requests_build_one_client`: setup — one `HttpEndpoint` struct literal (base URL `http://localhost:{dest_port}/`, `allowInternal=true`), `spawn_multi_accept_200()`, one producer, the endpoint's cache kept in a test-owned `Arc` clone; action — two sequential `oneshot` requests to `http://localhost:{dest_port}/`; assert — both OK and `cache.build_count() == 1`; command — `cargo test -p camel-component-http --lib producer_repeated_hostname_requests_build_one_client`; expected — before the Task 1.2 wiring the producer bypasses the cache entirely and the count would be 0 (test fails with 0 != 1); after, 1.
- `ip_literal_request_never_enters_cache`: setup — one `HttpEndpoint` struct literal with base URL `http://127.0.0.1:{dest_port}/ping` and `allowInternal=true`, `spawn_multi_accept_200()` on that port, one producer, the endpoint's cache kept in a test-owned `Arc` clone; action — one `oneshot` request; assert — exchange OK and `cache.build_count() == 0` (literal URL uses the shared client, never enters the cache); command — `cargo test -p camel-component-http --lib ip_literal_request_never_enters_cache`; expected — passes after implementation.

**Acceptance:**
- `cargo fmt --check --all` exits 0.
- `cargo clippy -p camel-component-http --all-targets --all-features -- -D warnings` exits 0.
- `cargo test -p camel-component-http --lib` passes (3 new tests green, existing suites unchanged).
- `cargo xtask lint-unwrap` exits 0.

- [x] 1.3

### Task 1.4: Redirect path wiring

**Files:**
- `crates/components/camel-http/src/ssrf.rs` (modified)
- `crates/components/camel-http/src/lib.rs` (modified — the single caller at ~line 2331)

**Steps:**
1. In `send_with_ssrf_safe_redirects` (~259), add two parameters after `initial_client`: `shared_client: &reqwest::Client` and `pinned_cache: &crate::client_cache::PinnedClientCache`. The existing `#[allow(clippy::too_many_arguments)]` stays.
2. In the redirect hop handling (~370-376), replace `current_client = build_client(http_config, Some((redirect_host, resolved_slice)));` with: if `redirect_host.parse::<std::net::IpAddr>().is_ok()` (IP-literal target) set `current_client = shared_client.clone();` (unpinned, never enters the cache); otherwise `current_client = pinned_cache.get_or_build(redirect_host, &resolved_addrs, || build_client(http_config, Some((redirect_host, &resolved_addrs)))).await;`. The `validate_redirect_target_for_ssrf` call above it is unchanged — per-hop validation stays.
3. Update the doc comment of `send_with_ssrf_safe_redirects` (step 5 of the numbered list): hops build per-hop clients with `resolve_to_addrs` **from the endpoint's pinned-client cache** (hostname targets) or reuse the shared unpinned client (IP-literal targets).
4. Update the caller in `lib.rs` (~2331): pass `&client` as `initial_client` — this is the SHADOWED pinned-or-shared binding from Task 1.2 step 6 (unchanged by Task 1.3), so a hostname initial request keeps its DNS-pinned client — and pass `&shared_client` (the pre-shadowing unpinned binding) as `shared_client`, plus `&pinned_cache`, all before `&http_config`.
5. Add the redirect tests below to the `#[cfg(test)]` module in `ssrf.rs`. Add two small local-server helpers there, modeled on the listener pattern in `lib.rs` tests (~7994): `spawn_302_responder(location: String)` — binds `tokio::net::TcpListener` on `127.0.0.1:0`, accepts connections in a loop, writes a minimal `HTTP/1.1 302 Found\r\nLocation: {location}\r\nContent-Length: 0\r\n\r\n` response; `spawn_200_responder()` — same but `HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok`. Both return `(base_url, JoinHandle)`.

**Tests:** (executable spec)
- `redirect_hostname_target_reuses_cached_client`: setup — `spawn_302_responder(format!("http://localhost:{}/hop", port2))` on port1 and `spawn_200_responder()` on port2, a fresh `PinnedClientCache::new(PINNED_CLIENT_TTL, PINNED_CLIENT_MAX_ENTRIES)`, clients built with `build_client(&HttpConfig::default(), None)` (redirect `Policy::none()` — a bare `reqwest::Client::new()` would auto-follow the 302 before the manual loop sees it) serving as both initial and shared client, and an `HttpEndpointConfig` from `HttpEndpointConfig::from_uri("http://localhost/?allowInternal=true").unwrap()`; action — call `send_with_ssrf_safe_redirects(&shared, &shared, &cache, &http_config, &endpoint_config, reqwest::Method::GET, "http://localhost:{port1}/start", vec![], None, 3, None).await` twice sequentially; assert — first call returns status 200 and `cache.build_count() == 1` (only the `localhost:port2` redirect hop enters the cache — the initial request uses `initial_client` untouched); second call returns 200 and `cache.build_count()` is still 1 (the hop entry is reused from cache); command — `cargo test -p camel-component-http --lib redirect_hostname_target_reuses_cached_client`; expected — fails before the change (per-hop `build_client` has no cache; with the new signature absent it does not compile), passes after.
- `redirect_ip_literal_target_bypasses_cache`: setup — `spawn_302_responder(format!("http://127.0.0.1:{}/hop", port2))` on port1, `spawn_200_responder()` on port2, a fresh cache, an initial/shared client built with `build_client(&HttpConfig::default(), None)`, the same `HttpConfig::default()` and allow-internal `HttpEndpointConfig` as above; action — one `send_with_ssrf_safe_redirects` call with initial URL `http://127.0.0.1:{port1}/start` (literal, so the initial request uses the shared client); assert — response status 200 and `cache.build_count() == 0` (neither the literal initial request nor the literal hop entered the cache); command — `cargo test -p camel-component-http --lib redirect_ip_literal_target_bypasses_cache`; expected — passes after implementation.
- Existing SSRF tests unchanged: `cargo test -p camel-component-http --lib ssrf` — the full existing `ssrf` module suite (validation, blocked-IP, TOCTOU pinning tests) passes without modification.

**Acceptance:**
- `cargo fmt --check --all` exits 0.
- `cargo clippy -p camel-component-http --all-targets --all-features -- -D warnings` exits 0.
- `cargo test -p camel-component-http --lib` passes (2 new redirect tests + all existing suites).
- `cargo xtask lint-unwrap` and `cargo xtask lint-ignore` exit 0.
- The full workspace gate set from `AGENTS.md ## QUALITY GATES` runs at STAGE 4 close-out; no config/schema surface changed, so `schema-check` is unaffected.

- [x] 1.4
