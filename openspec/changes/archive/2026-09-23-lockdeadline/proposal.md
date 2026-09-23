# Proposal: lockdeadline — deadline-bounded global test-lock acquisition

## Why

bd rc-88old: 97 test-body sites acquire a process-global test
mutex with `.lock().await` and no deadline. These locks serialize
tests that share global state (the camel-ws `ServerRegistry`, static
HTTP ports, Redis sentinel topology). When one holder deadlocks or
leaks the guard, every queued test on the same mutex parks forever —
the exact rc-y24l camel-ws hang class that wedged a whole 4/4
libtest binary. The unbounded acquisition converts any single stuck
test into a binary-wide stall.

## What changes

- Add ONE shared helper `acquire_deadline(lock, what, deadline)` to
  `camel_component_api::test_support` (behind the existing
  `test-support` Cargo feature). A synchronous `#[track_caller]`
  outer function captures the acquisition site, then returns a
  future that wraps `lock.lock()` in `tokio::time::timeout`; on
  timeout it panics naming the lock, the site, and the deadline.
- Mechanically convert all 97 sites across 7 files / 4 crates:
  - `crates/components/camel-ws/src/lib.rs` — REGISTRY_TEST_LOCK ×44
  - `crates/camel-dsl/tests/rest_negotiation_e2e.rs` — SERVER_MUTEX ×15
  - `crates/camel-dsl/tests/rest_stream_contract_e2e.rs` — SERVER_MUTEX ×8
  - `crates/camel-test/tests/http_static_test.rs` — TEST_MUTEX ×18
  - `crates/camel-test/tests/redis_repositories_test.rs` — SENTINEL_TOPOLOGY_LOCK ×6
  - `crates/camel-test/tests/redis_sentinel_test.rs` — TOPOLOGY_LOCK ×3
  - `crates/components/camel-component-wasm/tests/source_bind_gate.rs` — ACK_TEST_LOCK ×3
- One uniform deadline `TEST_LOCK_DEADLINE = 900 s`, derived above
  every family's worst-case healthy serialization queue (largest
  chain 602 s, ~1.5× margin); a timeout therefore means a stalled
  holder, not load.
- `scripts/xtask/ratchet-unbounded-wait.max` and
  `scripts/xtask/src/lint_unbounded_wait.rs` are NOT touched —
  mission 238 (drainscope) owns them. The lint finding count only
  decreases.

## Acceptance criteria

- `rg '<LOCK>\.lock\(\)\.await'` for all six lock names returns zero
  test-body sites on this branch.
- Helper unit tests pass: uncontended fast path, and stalled-holder
  panic that names lock + deadline.
- Affected crates compile and their locally-runnable test suites pass;
  infra-dependent suites (redis, `#[ignore]` wasm gates) are
  compile-verified and deferred to CI.
- fmt, clippy `-D warnings` on affected crates, and the 16 xtask
  lints pass.

## Risk budget

Low: test-only surface, mechanical conversion, no production code
path changes. Main risk is a false-positive timeout on a healthy but
deeply serialized suite — neutralized by the uniform 900 s deadline
(1.5× above the largest worst-case chain, 602 s); heavily loaded CI
still completes its serialization chains inside the deadline.

## Affected crates

`camel-component-api` (helper + tests), `camel-ws`, `camel-dsl`,
`camel-test`, `camel-component-wasm` (site conversions only).

## Issue

bd rc-88old (P2, in_progress).
