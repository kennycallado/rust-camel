# Tasks: seda-flake-fix

## camel-test

### Task 1.1: Convert the three flaky seda tests to event-driven settling

**Files:**
- `crates/camel-test/tests/seda_test.rs` (modified)

**Steps:**
1. `test_seda_connects_two_routes`: move the mock endpoint handle acquisition
   (`let endpoint = h.mock().get_endpoint("result").unwrap();`) to immediately
   after `h.start().await;`. Replace
   `tokio::time::sleep(std::time::Duration::from_millis(500)).await;` with
   `endpoint.await_exchanges(3, std::time::Duration::from_secs(10)).await;`.
   Keep `h.stop().await;` then `endpoint.assert_exchange_count(3).await;`
   unchanged.
2. `test_seda_concurrent_load`: same restructuring — endpoint handle after
   `h.start()`, replace
   `tokio::time::sleep(std::time::Duration::from_millis(1000)).await;` with
   `endpoint.await_exchanges(50, std::time::Duration::from_secs(30)).await;`
   before `h.stop()`, keep `assert_exchange_count(50)` after stop.
3. `test_seda_inout_integration`: same restructuring on endpoint
   `inout-result` — replace the 500ms sleep with
   `endpoint.await_exchanges(3, std::time::Duration::from_secs(10)).await;`
   before `h.stop()`, keep `assert_exchange_count(3)` after stop.
4. Route definitions, `repeatCount`s, endpoint names, exchange patterns, and
   final assertions stay byte-identical. The three already-settled tests
   (`test_seda_fanout_integration`, `seda_single_consumer_survives_context_restart`,
   `seda_single_consumer_survives_suspend_resume`) are NOT touched.
5. Run `cargo fmt` on the file and
   `cargo clippy -p camel-test --all-targets -- -D warnings`.

**Tests:** (executable spec — the converted tests are the deliverable)
- `test_seda_connects_two_routes`: setup = timer period=50 repeatCount=3 →
  seda:bridge → mock:result, harness started → action = await 3 exchanges with
  10s backstop, then stop → assert = exactly 3 exchanges. Command:
  `cargo test -p camel-test --test seda_test test_seda_connects_two_routes -- --exact`.
  Expected: green deterministically (previously flaked when 500ms window lost
  the last tick).
- `test_seda_concurrent_load`: setup = timer period=10 repeatCount=50 →
  seda:load concurrentConsumers=4 → mock:result → action = await 50 exchanges
  with 30s backstop, then stop → assert = exactly 50. Command:
  `cargo test -p camel-test --test seda_test test_seda_concurrent_load -- --exact`.
- `test_seda_inout_integration`: setup = timer repeatCount=3 → seda InOut →
  mock:inout-result → action = await 3 exchanges with 10s backstop, then stop →
  assert = exactly 3. Command:
  `cargo test -p camel-test --test seda_test test_seda_inout_integration -- --exact`.
- Full-binary stability: `cargo test -p camel-test --test seda_test` in 10
  consecutive runs — expected 10/10 green (main showed ~4/8 failure rate).

**Acceptance:**
- `rg -n "from_millis\(500\)|from_millis\(1000\)" crates/camel-test/tests/seda_test.rs`
  returns no matches inside the three converted tests (fixed windows removed
  from their settling paths).
- `cargo test -p camel-test --test seda_test` exits 0, 10/10 consecutive runs.
- `cargo check -p camel-test` exits 0.
- `cargo fmt --check` and `cargo clippy -p camel-test --all-targets -- -D warnings`
  exit 0.

- [x] 1.1
