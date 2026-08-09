# Tasks: audit-fix-availability-dos

## camel-component-grpc

### Task G1: Add BackoffState to gRPC accept-loop

**Files:**
- `crates/components/camel-component-grpc/src/server.rs` (modified)

**Steps:**
1. Add `use camel_api::backoff::{BackoffConfig, BackoffState};` to the imports at the top of `server.rs`. Also add `use std::time::Duration;` if not already present.
2. Extract the backoff config to a testable function: add `fn accept_backoff_config() -> BackoffConfig { BackoffConfig { initial_delay: Duration::from_millis(10), multiplier: 2.0, max_delay: Duration::from_secs(5) } }` as a private fn near `run_grpc_server` (NOT inline in the loop body). This enables testing the config values without integration timing.
3. In `run_grpc_server` (line 330), after the `route_id` derivation (line 344) and before the `loop` (line 346), construct: `let mut backoff = BackoffState::new(accept_backoff_config());`.
4. In the accept-loop error branch (lines 349-356), BEFORE the `continue` at line 355, insert: `let delay = backoff.next_delay();` then `tokio::time::sleep(delay).await;`.
5. In the accept-loop success branch (after line 348 where the `Ok(s)` is matched, before `tokio::spawn` at line 364), insert: `backoff.reset();`.

**Tests:** (executable spec — name, setup, action, assert)
- `test_accept_backoff_config_values`: Assert that `accept_backoff_config()` returns `BackoffConfig { initial_delay: Duration::from_millis(10), multiplier: 2.0, max_delay: Duration::from_secs(5) }`. Place the test in the existing `mod tests` block (server.rs:752); `accept_backoff_config` is in scope via `use super::*`. This verifies the correct config is wired. Integration timing of the accept-loop cannot be deterministically tested without restructuring `run_grpc_server` to inject the backoff — the config test proves the right values are used, and `BackoffState` behavior (increase-on-error, cap, reset-on-success) is already tested in `camel-api/src/backoff.rs:78+`.

**Acceptance:**
- `CARGO_TARGET_DIR=/home/kenny/.cache/rust-camel-target-audit-fix-availability-dos cargo test -p camel-component-grpc --lib` — all existing + new tests pass
- `CARGO_TARGET_DIR=/home/kenny/.cache/rust-camel-target-audit-fix-availability-dos cargo clippy -p camel-component-grpc --all-targets -- -D warnings` — clean
- `BackoffState` used in `run_grpc_server` error path with `next_delay()` + `sleep`
- `backoff.reset()` called on successful accept
- `accept_backoff_config()` function exists and is testable

- [x] G1

## camel-component-llm

### Task L1: Add max_header_json_bytes config + header size check

**Files:**
- `crates/components/camel-component-llm/src/config.rs` (modified — add field)
- `crates/components/camel-component-llm/src/producer.rs` (modified — add field, builder, check helper)
- `crates/components/camel-component-llm/src/endpoint.rs` (modified — thread config)

**Steps:**
1. In `config.rs`, add `fn default_max_header_json_bytes() -> usize { 65_536 }` near `default_max_prompt_bytes` (line 9). Add `#[serde(default = "default_max_header_json_bytes")] pub max_header_json_bytes: usize,` to `LlmGlobalConfig` (after `max_prompt_bytes` at line 34). Update `Default` impl to include `max_header_json_bytes: default_max_header_json_bytes()`.
2. In `producer.rs`, add `max_header_json_bytes: usize` field to `LlmProducer` struct (after `max_prompt_bytes`). Initialize it in `LlmProducer::new` from `crate::config::default_max_header_json_bytes()` (NOT a hardcoded literal — single source of truth). Add builder method: `pub fn with_max_header_json_bytes(mut self, max: usize) -> Self { self.max_header_json_bytes = max; self }`.
3. In `endpoint.rs`, after the `LlmProducer::new(config, provider, max_prompt_bytes, route_id)` call and before `.build()`, add `.with_max_header_json_bytes(global_config.max_header_json_bytes)` to the builder chain. Follow the `.with_*()` builder precedent (e.g. `.with_timeout()`, `.with_pricing()`).
4. In `producer.rs`, add a private helper near `build_chat_request`: `fn check_header_json_size(name: &str, value: &serde_json::Value, max: usize) -> Result<(), LlmError> { let size = value.to_string().len(); if size > max { return Err(LlmError::InvalidRequest(format!("{name} header exceeds max_header_json_bytes ({size} > {max})"))); } Ok(()) }`.
5. In `build_chat_request` (line 139), call `check_header_json_size("CamelLlmMessages", msgs_val, self.max_header_json_bytes)?;` BEFORE the `serde_json::from_value` at line 171. Call `check_header_json_size("CamelLlmTools", v, self.max_header_json_bytes)?;` inside the `.map()` closure BEFORE `serde_json::from_value` at line 191. Call `check_header_json_size("CamelLlmToolChoice", v, self.max_header_json_bytes)?;` inside the `.map()` closure BEFORE `serde_json::from_value` at line 209. The helper is a free function, not a method — call it without `self.`.

**Tests:** (executable spec — name, setup, action, assert)
- `test_oversized_messages_header_rejected`: Create an `LlmProducer` with default config (`max_header_json_bytes = 65536`). Build an exchange with a `CamelLlmMessages` header containing a JSON array of 1000 messages, each with a 200-byte content string (total > 64 KB). Call `build_chat_request` (or `process` to exercise the full path). Assert the result is `Err` containing "CamelLlmMessages" and "max_header_json_bytes".
- `test_oversized_tools_header_rejected`: Create an `LlmProducer` with default config. Build an exchange with a `CamelLlmTools` header containing a JSON array of 1000 tool definitions with long names (total > 64 KB). Assert `Err` containing "CamelLlmTools" and "max_header_json_bytes".
- `test_normal_headers_accepted`: Create an `LlmProducer` with default config. Build an exchange with `CamelLlmMessages` containing 5 small messages (total < 1 KB). Assert `build_chat_request` succeeds (returns `Ok(ChatRequest)`).
- `test_custom_threshold_respected`: Create an `LlmProducer` with `.with_max_header_json_bytes(1024)`. Build an exchange with a `CamelLlmMessages` header of 2 KB. Assert `Err` containing "max_header_json_bytes".
- `test_oversized_tool_choice_header_rejected`: Create an `LlmProducer` with `.with_max_header_json_bytes(1024)`. Build an exchange with a `CamelLlmToolChoice` header containing a large JSON object (> 1 KB). Assert `Err` containing "CamelLlmToolChoice" and "max_header_json_bytes". This exercises the third `check_header_json_size` call site.

**Acceptance:**
- `CARGO_TARGET_DIR=/home/kenny/.cache/rust-camel-target-audit-fix-availability-dos cargo test -p camel-component-llm --lib` — all existing + new tests pass
- `CARGO_TARGET_DIR=/home/kenny/.cache/rust-camel-target-audit-fix-availability-dos cargo clippy -p camel-component-llm --all-targets -- -D warnings` — clean
- `max_header_json_bytes` field exists on `LlmGlobalConfig` with serde default 65_536
- Builder `.with_max_header_json_bytes()` exists on `LlmProducer`
- `check_header_json_size` helper exists and is called before all 3 `from_value` sites
- No hardcoded `65_536` literal in `producer.rs` (uses `default_max_header_json_bytes()`)

- [x] L1
