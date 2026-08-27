# Tasks: http-component-shared-client

## camel-component-http

### Task 1.1: Hoist shared unpinned client to component

**Files:**
- `crates/components/camel-http/src/lib.rs` (modified)
- `crates/components/camel-http/CONTEXT.md` (modified)

**Steps:**
1. Add a `#[cfg(test)] thread_local!` counter near `pub(crate) fn
   build_client` (lib.rs:1806):
   `thread_local! { static BUILD_CLIENT_CALLS: std::cell::Cell<u64> =
   const { std::cell::Cell::new(0) }; }`. Increment it at the top of
   `build_client` under `#[cfg(test)]` via
   `.with(|c| c.set(c.get() + 1))`. Add a `#[cfg(test)] pub(crate) fn
   build_client_call_count() -> u64` reader (`.with(|c| c.get())`).
   Thread-local is mandatory: sibling tests construct components on
   parallel threads, and a process-global counter would make the delta
   assertions flaky. The three new tests must be plain sync `#[test]`s.
   No production-visible surface.
2. Add field `client: reqwest::Client` to `pub struct HttpComponent`
   (~lib.rs:1801) and `pub struct HttpsComponent` (~lib.rs:1921).
3. Initialize the field in every constructor of both structs (`new`,
   `with_config`; `with_optional_config` and `Default` funnel through
   these — verify, do not duplicate) with
   `build_client(&self.config, None)`.
4. In both `create_endpoint` bodies (http ~lib.rs:1908, https
   ~lib.rs:1977), delete the `let client = build_client(&self.config,
   None);` line (lib.rs:1914, lib.rs:1993) and pass
   `client: self.client.clone()` into the `HttpEndpoint` struct literal.
5. `rg -n "build_client\(" crates/components/camel-http/src/lib.rs` —
   after the change the only production call sites are the component
   constructors and the pinned-cache-miss closure (~lib.rs:2226); none in
   either `create_endpoint` body.
6. Add one sentence to the operator note in `crates/components/camel-http/CONTEXT.md`
   (end of `### Outbound SSRF and TLS defaults`): the shared unpinned
   client also holds up to `pool_max_idle_per_host` idle connections for
   its hosts.

**Tests:** (write these exact tests first, in the existing `mod tests`
next to the pinned-cache sharing tests; no requests are sent — counters
stay deterministic because `create_producer` clones and never builds, and
the pinned closure fires only inside producer `call`)
- `test_component_constructor_builds_one_unpinned_client`:
  setup: read `build_client_call_count()` baseline.
  action: `HttpComponent::new()`; then `HttpsComponent::new()`.
  assert: each constructor call increased the counter by exactly one
  (baseline+1 after the first, baseline+2 after the second).
  command: `cargo test -p camel-component-http --lib
  test_component_constructor_builds_one_unpinned_client`.
  expected: fails pre-change (constructors build zero, create_endpoint
  builds per call — total delta 0, not 2).
- `test_component_endpoints_share_unpinned_client`:
  setup: `HttpComponent::new()`, counter baseline after construction.
  action: `create_endpoint` for `http://localhost:1/a?allowInternal=true`
  and `http://localhost:1/b?allowInternal=true` with
  `NoOpComponentContext`.
  assert: counter delta == 0 across both endpoint creations.
  command: `cargo test -p camel-component-http --lib
  test_component_endpoints_share_unpinned_client`.
  expected: fails pre-change (delta == 2).
- `test_dynamic_resolution_adds_no_unpinned_client_builds`:
  setup: `HttpComponent::new()`, baseline after construction.
  action: loop three times over
  `http://localhost:1/r{i}?allowInternal=true&k={i}` calling
  `create_endpoint` then `create_producer` (no request sent).
  assert: counter delta == 0 across the whole loop.
  command: `cargo test -p camel-component-http --lib
  test_dynamic_resolution_adds_no_unpinned_client_builds`.
  expected: fails pre-change (delta == 3).

**Acceptance:**
- The three named tests exist with exactly those names and pass.
- `rg -n "build_client\(" crates/components/camel-http/src/lib.rs` shows
  no call inside either `create_endpoint` body; production call sites are
  component constructors plus the pinned-cache-miss closure only.
- `cargo test -p camel-component-http --lib` passes in full.
- `cargo fmt --check --all` and
  `cargo clippy -p camel-component-http -- -D warnings` exit 0.
- `cargo xtask lint-context-citations` exits 0; CONTEXT.md sentence names
  `pool_max_idle_per_host`.
- Scenario 4 (config identity) is verified structurally — single
  derivation from `self.config`, rg-gate on call sites — and by the
  existing IP-literal unpinned-client test.

- [x] 1.1
