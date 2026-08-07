# Tasks: audit-fix-http-clippy-gate

## camel-component-http

### Task 1.1: Add 3 missing `#[allow(clippy::await_holding_lock)]` to test functions

**Files:**
- `crates/components/camel-http/src/static_endpoint.rs` (modified)
- `crates/components/camel-http/src/lib.rs` (modified)

**Steps:**
1. In `crates/components/camel-http/src/static_endpoint.rs`, add `#[allow(clippy::await_holding_lock)]` immediately above the `#[tokio::test]` attribute on the function `test_static_consumer_emits_mark_ready_after_register`. Match the indentation and placement of the 2 existing `#[allow(clippy::await_holding_lock)]` annotations in the same file (e.g. lines 434, 499).
2. In `crates/components/camel-http/src/lib.rs`, add `#[allow(clippy::await_holding_lock)]` immediately above the `#[tokio::test]` attribute on the function `test_unregister_last_http_route_keeps_server_alive`. Match the indentation of the 15 existing annotations in the same file.
3. In `crates/components/camel-http/src/lib.rs`, add `#[allow(clippy::await_holding_lock)]` immediately above the `#[tokio::test]` attribute on the function `test_http_consumer_emits_mark_ready_after_bind`. Match the indentation of the 15 existing annotations in the same file.

**Tests:** (executable spec — name, setup, action, assert)
- `clippy-gate-passes`: after all 3 annotations are added → run `cargo clippy -p camel-component-http --all-targets -- -D warnings` → exits 0 (previously exited non-zero with 3 `clippy::await_holding_lock` warnings).

**Acceptance:**
- `cargo check -p camel-component-http --all-targets` exits 0.
- `cargo test -p camel-component-http` passes (no behavioral change, all existing tests green).
- `cargo clippy -p camel-component-http --all-targets -- -D warnings` exits 0.
- `cargo clippy --workspace --all-features --exclude camel-cli --exclude camel-component-kafka --exclude security-keycloak --exclude security-wasm-policy -- -D warnings` exits 0 (workspace gate from AGENTS.md).
- Total `#[allow(clippy::await_holding_lock)]` count in `static_endpoint.rs` = 3 (was 2).
- Total `#[allow(clippy::await_holding_lock)]` count in `lib.rs` = 17 (was 15).
- `cargo fmt --check --all` exits 0 (no formatting change — annotation placement matches existing).

- [x] 1.1
