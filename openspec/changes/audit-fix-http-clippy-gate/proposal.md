# Proposal: audit-fix-http-clippy-gate

## Why

`cargo clippy -p camel-component-http --all-targets -- -D warnings` fails with 3 `clippy::await_holding_lock` violations in test functions. The workspace clippy gate (AGENTS.md `## QUALITY GATES`) does NOT exclude `camel-component-http` from the workspace-wide clippy run, so this failure blocks CI and blocks every subsequent audit-fix change from passing quality gates.

17 existing test functions in the same crate already carry `#[allow(clippy::await_holding_lock)]` because the tests intentionally hold a lock guard across an `.await` point. The 3 missing annotations are a gap, not a design change.

**bd:** rc-4vx8 (P1, freeze-blocker).

## What Changes

- Add `#[allow(clippy::await_holding_lock)]` to 3 test functions in `camel-component-http`:
  1. `static_endpoint.rs` — `test_static_consumer_emits_mark_ready_after_register`
  2. `lib.rs` — `test_unregister_last_http_route_keeps_server_alive`
  3. `lib.rs` — `test_http_consumer_emits_mark_ready_after_bind`

**Explicitly excluded:** no production code changes, no behavioral change, no new tests — this is a mechanical annotation mirror.

## Acceptance criteria

- `cargo clippy -p camel-component-http --all-targets -- -D warnings` passes.
- 3 `#[allow]` annotations added, matching the exact pattern of the 17 existing ones.
- Workspace clippy gate green.

## Risk budget

Zero risk. The change adds compiler annotations to test-only code, mirroring a pattern already used 17 times in the same files. No production behavior is affected.
