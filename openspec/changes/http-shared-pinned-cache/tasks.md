# Tasks: http-shared-pinned-cache

## camel-component-http

### Task 1.1: Hoist PinnedClientCache from endpoint to component

**Files:**
- `crates/components/camel-http/src/lib.rs` (modified)

**Steps:**
1. Add field `pinned_cache: std::sync::Arc<PinnedClientCache>` to
   `pub struct HttpComponent` (around lib.rs:1801) and to
   `pub struct HttpsComponent` (around lib.rs:1921).
2. Initialize the field in every constructor of both structs
   (`HttpComponent::new`, any `with_config`-style constructor,
   `HttpsComponent::new` and its variants, and the `Default` impls that
   funnel through `new`) with
   `std::sync::Arc::new(PinnedClientCache::new(PINNED_CLIENT_TTL, PINNED_CLIENT_MAX_ENTRIES))`.
3. In `impl Component for HttpComponent::create_endpoint`
   (around lib.rs:1893) and `impl Component for HttpsComponent::create_endpoint`
   (around lib.rs:1963), delete the local `pinned_cache` binding created by
   the `PinnedClientCache::new` call in each body (lib.rs:1902-1905 and
   lib.rs:1972-1975) and pass
   `pinned_cache: std::sync::Arc::clone(&self.pinned_cache)`
   into the `HttpEndpoint` struct literal instead.
4. Run `rg -n "HttpComponent \\{|HttpsComponent \\{" crates/components/camel-http/src`
   and fix any internal struct-literal construction of the two components
   (tests construct them through `new()`, so hits are expected only if a
   literal exists).

**Tests:** (existing suites are the regression net; new behavior tests land
in Task 1.2)
- `producers_share_endpoint_cache` and
  `producer_repeated_hostname_requests_build_one_client`: exist in the
  current suite (lib.rs:8470, lib.rs:8502). Verify they still pass
  unchanged: helper `endpoint_with_shared_cache` (lib.rs:8454) injects a
  cache, so Task 1.1 must not alter that path. Command:
  `cargo test -p camel-component-http --lib pinned cache` → the pinned and
  cache suites pass; or explicitly
  `cargo test -p camel-component-http --lib producers_share_endpoint_cache
  producer_repeated_hostname_requests_build_one_client`.
- `test_http_endpoint_creates_producer`: exists (around lib.rs:2830).
  Command: `cargo test -p camel-component-http --lib
  test_http_endpoint_creates_producer` → passes.

**Acceptance:**
- `cargo build -p camel-component-http` exits 0.
- `rg -n "PinnedClientCache::new" crates/components/camel-http/src/lib.rs`
  shows hits only in constructors and test code, none inside either
  `create_endpoint` body.
- `cargo test -p camel-component-http --lib` passes.
- `cargo fmt --check --all` and
  `cargo clippy -p camel-component-http -- -D warnings` exit 0.

- [x] 1.1

### Task 1.2: Cross-endpoint and dynamic-resolution sharing tests

**Files:**
- `crates/components/camel-http/src/lib.rs` (modified, test module)

**Steps:**
1. Add the three test functions below to the existing `mod tests` in
   `lib.rs`, next to the existing shared-cache tests. They access the
   private `pinned_cache` field of the concrete component structs directly
   (in-crate test module).
2. Reuse the existing local-responder helper `spawn_multi_accept_200`
   (lib.rs:8425) whose `base_url` is the `http://localhost:{port}` form,
   exactly like `producers_share_endpoint_cache` (lib.rs:8470) does. The
   `localhost` hostname is required: IP-literal URIs such as 127.0.0.1
   bypass the pinned cache (`ip_literal_request_never_enters_cache`,
   lib.rs:8534) and would make every build-count assertion fail.
3. Give every URI used by tests (a) and (b) distinct paths AND distinct
   endpoint-level query parameters, so the tests also exercise the
   config-identity scenario: client config comes from the component's
   `HttpConfig`, never from URI parameters.

**Tests:** (write these exact tests first; they must fail after Task 1.1 is
reverted and pass after Task 1.1)
- `test_component_endpoints_share_pinned_cache`:
  setup: one `HttpComponent::new()`, `base_url` from
  `spawn_multi_accept_200()`, `build_count()` baseline read from
  `component.pinned_cache`.
  action: `create_endpoint` twice with distinct URIs
  (`{base_url}/a?allowInternal=true&k=a` and
  `{base_url}/b?allowInternal=true&k=b`), `create_producer` from each
  endpoint, send one request through each producer.
  assert: `component.pinned_cache.build_count() - baseline == 1`.
  command: `cargo test -p camel-component-http --lib
  test_component_endpoints_share_pinned_cache`.
  expected: fails to compile pre-change (no `pinned_cache` field on the
  component), passes after Task 1.1.
- `test_dynamic_resolution_sequence_hits_shared_cache`:
  setup: one `HttpComponent::new()`, `base_url` from
  `spawn_multi_accept_200()`, baseline `build_count()`.
  action: loop three times over distinct URIs
  (`{base_url}/r{i}?allowInternal=true&k={i}`); each iteration calls
  `create_endpoint` then `create_producer` then sends one request, the
  resolution shape of recipientList/routingSlip/dynamicRouter.
  assert: `component.pinned_cache.build_count() - baseline == 1`.
  command: `cargo test -p camel-component-http --lib
  test_dynamic_resolution_sequence_hits_shared_cache`.
  expected: fails to compile pre-change (no `pinned_cache` field on the
  component), passes after Task 1.1.
- `test_https_component_owns_distinct_cache`:
  setup: `HttpComponent::new()` and `HttpsComponent::new()` side by side.
  action: `Arc::ptr_eq(&http.pinned_cache, &https.pinned_cache)`; then
  `create_endpoint` on each component
  (`http://localhost:1/?allowInternal=true` for the http component,
  `https://localhost:1/?allowInternal=true` for the https component, with
  `NoOpComponentContext`).
  assert: `ptr_eq` returns false; both `create_endpoint` calls return
  `Ok`; both components' caches report `build_count() == 0` after endpoint
  creation (no request sent, no build). The wiring of each endpoint to its
  component's cache is enforced by the Task 1.1 rg acceptance gate.
  command: `cargo test -p camel-component-http --lib
  test_https_component_owns_distinct_cache`.
  expected: fails to compile pre-change (no field), passes after Task 1.1.

**Acceptance:**
- The three named tests exist with exactly those names and pass.
- `cargo test -p camel-component-http --lib` passes in full.
- `cargo fmt --check --all` and
  `cargo clippy -p camel-component-http -- -D warnings` exit 0.

- [x] 1.2

### Task 1.3: Operator note on shared-cache idle-connection footprint

**Files:**
- `crates/components/camel-http/CONTEXT.md` (modified)

**Steps:**
1. Read `crates/components/camel-http/CONTEXT.md`. The file has no
   client-reuse section today; the pinned-client work is documented in
   `### Outbound SSRF and TLS defaults`. Append the new note at the end of
   that section.
2. Append one short paragraph there stating: the pinned-client cache is
   shared across all endpoints of one component instance; it retains at
   most `PINNED_CLIENT_MAX_ENTRIES` (64) clients, each for a
   `PINNED_CLIENT_TTL` (60 s) window; each cached client can hold up to
   `pool_max_idle_per_host` (default 100) idle connections per distinct
   host until `pool_idle_timeout` (default 90 s) closes them; workloads
   touching many distinct hosts should size these two knobs accordingly.
3. Keep canonical English and the file's existing citation style.

**Tests:** (documentation task — lint gates are the test)
- `cargo xtask lint-context-citations` exits 0 after the edit.
- `rg -n "pool_max_idle_per_host" crates/components/camel-http/CONTEXT.md`
  returns at least one hit.

**Acceptance:**
- The paragraph exists at the end of `### Outbound SSRF and TLS defaults`,
  names both config fields (`pool_max_idle_per_host`,
  `pool_idle_timeout`) and both cache constants
  (`PINNED_CLIENT_MAX_ENTRIES`, `PINNED_CLIENT_TTL`), and states the 90 s
  default idle timeout.
- `cargo xtask lint-context-citations` exits 0.

- [x] 1.3
