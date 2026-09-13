# Tasks: jmsmutants

## camel-component-jms

### Task 1.1: Pin reconnect defaults and adjudicate equivalent fields

**Files:**
- `crates/components/camel-jms/src/config.rs` (modified)

**Steps:**
1. Add a unit test in the existing configuration test module that calls the existing `jms_reconnect_default()` helper and asserts `max_attempts == 0`, `initial_delay == Duration::from_secs(5)`, `jitter_factor == 0.0`, `max_delay == Duration::from_secs(30)`, and `multiplier == 2.0`.
2. Run the focused test before the test change is present to establish the baseline, then run it after adding the assertions and inspect the result against the reconnect policy source.
3. Record in the task result that deletion of `multiplier` and `max_delay` is equivalent because `NetworkRetryPolicy::default()` supplies the same values; do not change production code to manufacture a non-equivalent value.

**Tests:**
- `jms_reconnect_default_pins_all_overridden_fields`: arrange the existing JMS config test module; act by calling `jms_reconnect_default()`; assert the five exact values above; command `cargo test -p camel-component-jms --lib jms_reconnect_default_pins_all_overridden_fields`; expected result is one passing test.

**Acceptance:**
- The focused test asserts all five fields and passes.
- `initial_delay`, `jitter_factor`, and `max_attempts` deletion mutants are killed by the assertions; `multiplier` and `max_delay` deletion mutants have written equivalent-mutant rationale tied to `NetworkRetryPolicy::default()`.
- `cargo fmt --check` and `cargo clippy -p camel-component-jms --all-targets -- -D warnings` exit 0.

- [x] 1.1

### Task 1.2: Add adversarial endpoint URI validation tests

**Files:**
- `crates/components/camel-jms/src/config.rs` (modified)

**Steps:**
1. Add unit tests using `JmsEndpointConfig::from_uri` for unsupported schemes, empty `queue:` and `topic:` names, non-empty `jms:<name>` shorthand, and priority query values 9 and 10.
2. Assert the empty-name error contains the destination-format message and does not contain the `jms:` ambiguity message, so the shorthand guard cannot be replaced with an unconditional match arm.
3. Assert priority 9 parses successfully and priority 10 returns an error, directly pinning the strict `p > 9` boundary.
4. Run the focused tests and the full JMS library test suite.

**Tests:**
- `from_uri_rejects_unsupported_scheme`: arrange `http:queue:orders`; act with `JmsEndpointConfig::from_uri`; assert an error containing `expected scheme`; command `cargo test -p camel-component-jms --lib from_uri_rejects_unsupported_scheme`; expected result is one passing test.
- `from_uri_rejects_empty_destination_name_with_format_error`: arrange `jms:queue:`, `jms:topic:`, and bare `jms:`; act with `from_uri`; assert errors containing `destination must be` and not containing `ambiguous`; command `cargo test -p camel-component-jms --lib from_uri_rejects_empty_destination_name_with_format_error`; expected result is one passing test.
- `from_uri_rejects_ambiguous_jms_shorthand`: arrange `jms:orders`; act with `from_uri`; assert the full error text is `URI 'jms:orders' is ambiguous — use 'jms:queue:orders' or 'jms:topic:orders'`; command `cargo test -p camel-component-jms --lib from_uri_rejects_ambiguous_jms_shorthand`; expected result is one passing test.
- `from_uri_enforces_priority_boundary`: arrange `jms:queue:orders?priority=9` and `jms:queue:orders?priority=10`; act with `from_uri`; assert the first returns `Ok` with `priority == Some(9)` and the second returns an error; command `cargo test -p camel-component-jms --lib from_uri_enforces_priority_boundary`; expected result is one passing test.

**Acceptance:**
- Tests distinguish unsupported scheme, empty destination, and ambiguous shorthand errors.
- Priority 9 succeeds and priority 10 fails.
- The three `from_uri` survivor mutants are killed by these assertions.
- `cargo test -p camel-component-jms --lib` exits 0.

- [x] 1.2

### Task 1.3: Pin the bridge cache directory default

**Files:**
- `crates/components/camel-jms/src/config.rs` (modified)

**Steps:**
1. Add a unit test in the existing configuration test module that evaluates `default_bridge_cache_dir()` and the concrete `camel_bridge::download::default_cache_dir()` helper under the normal test environment; this deliberately pins the cross-crate cache subdirectory contract.
2. Assert the two paths are equal and the result is not `PathBuf::default()`, killing replacement with an empty default path.
3. Run the focused test and the full JMS library test suite without starting a broker.

**Tests:**
- `default_bridge_cache_dir_matches_bridge_download_default`: arrange the normal process environment; act by evaluating both existing cache-directory helpers; assert equality and inequality with `PathBuf::default()`; command `cargo test -p camel-component-jms --lib default_bridge_cache_dir_matches_bridge_download_default`; expected result is one passing test.

**Acceptance:**
- The test pins the concrete delegated cache path and rejects an empty `PathBuf`.
- The `default_bridge_cache_dir` replacement mutant is killed.
- `cargo test -p camel-component-jms --lib` exits 0.

- [x] 1.3
