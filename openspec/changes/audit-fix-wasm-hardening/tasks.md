# Tasks: audit-fix-wasm-hardening

## Task W1: WasmConfig limit fields + StateStore bounds

### Files

- `crates/components/camel-component-wasm/src/config.rs` (modified)
- `crates/components/camel-component-wasm/src/state_store.rs` (modified)
- `crates/camel-config/src/wasm_limits.rs` (modified)

### Steps

1. Add three constants to `config.rs`:
   - `DEFAULT_MAX_KV_ENTRIES: usize = 256`
   - `DEFAULT_MAX_KEY_BYTES: usize = 1024`
   - `DEFAULT_MAX_VALUE_BYTES: usize = 65536`

2. Add three fields to `WasmConfig` struct in `config.rs`:
   - `max_kv_entries: usize`
   - `max_key_bytes: usize`
   - `max_value_bytes: usize`

3. Set the three fields to their `DEFAULT_*` constants in `WasmConfig::default()`.

4. In `WasmConfig::from_limits`, read `limits.max_kv_entries`, `limits.max_key_bytes`,
   `limits.max_value_bytes` with `unwrap_or(DEFAULT_*)`. Add three fields to
   `WasmLimitsConfig` in `crates/camel-config/src/wasm_limits.rs`:
   `max_kv_entries: Option<usize>`, `max_key_bytes: Option<usize>`,
   `max_value_bytes: Option<usize>`, each with `#[serde(default,
   skip_serializing_if = "Option::is_none")]` and a doc comment, matching the
   `max_instances` precedent. Update the `defaults_to_all_none` and
   `deserialises_full_block` tests to include the three new fields.

5. In `WasmConfig::from_uri`, parse `max-kv-entries`, `max-key-bytes`,
   `max-value-bytes` from the query string, using `parse::<usize>()` with a `> 0`
   guard (same pattern as `max-concurrent-calls` / `max-instances`).

6. In `StateStore`, add three `usize` fields: `max_entries`, `max_key_bytes`,
   `max_value_bytes`. Add a constructor `StateStore::with_limits(max_entries: usize,
   max_key_bytes: usize, max_value_bytes: usize)` that stores them. Change
   `StateStore::new()` to delegate to `with_limits` with defaults (256, 1024, 65536).

7. In `StateStore::store`, before `guard.insert(...)`, add three checks in order:
   - If `key.len() > self.max_key_bytes` → return `Err("key exceeds max_key_bytes
     limit (N)")`
   - If `value.len() > self.max_value_bytes` → return `Err("value exceeds
     max_value_bytes limit (N)")`
   - If `!guard.contains_key(key)` and `guard.len() >= self.max_entries` → return
     `Err("kv entry limit exceeded (N)")`

### Tests

- **name**: `test_store_rejects_oversized_key`
  - **setup**: `StateStore::with_limits(256, 10, 65536)` (10-byte key cap)
  - **action**: `store.store("this_key_is_too_long_for_the_limit", "val")`
  - **assert**: returns `Err` whose message contains `"max_key_bytes"`
  - **command**: `cargo test -p camel-component-wasm --lib test_store_rejects_oversized_key`
  - **expected**: pass after implementation; fail before (current code returns `Ok`)

- **name**: `test_store_rejects_oversized_value`
  - **setup**: `StateStore::with_limits(256, 1024, 10)` (10-byte value cap)
  - **action**: `store.store("k", "this_value_is_too_long_for_limit")`
  - **assert**: returns `Err` whose message contains `"max_value_bytes"`
  - **command**: `cargo test -p camel-component-wasm --lib test_store_rejects_oversized_value`
  - **expected**: pass after implementation; fail before

- **name**: `test_store_rejects_entry_count_overflow`
  - **setup**: `StateStore::with_limits(2, 1024, 65536)` (2-entry cap), insert
    `"a"/"1"` and `"b"/"2"` successfully
  - **action**: `store.store("c", "3")` (new key, at cap)
  - **assert**: returns `Err` whose message contains `"kv entry limit"`
  - **command**: `cargo test -p camel-component-wasm --lib test_store_rejects_entry_count_overflow`
  - **expected**: pass after implementation; fail before

- **name**: `test_store_allows_update_within_bounds`
  - **setup**: `StateStore::with_limits(2, 1024, 65536)`, insert `"a"/"1"` and
    `"b"/"2"` (at cap)
  - **action**: `store.store("a", "updated")` (existing key)
  - **assert**: returns `Ok(())`, and `store.load("a")` returns `Some("updated")`
  - **command**: `cargo test -p camel-component-wasm --lib test_store_allows_update_within_bounds`
  - **expected**: pass after implementation (existing key update does not trigger
    entry-count check)

### Acceptance

- `cargo test -p camel-component-wasm --lib test_store_` passes (4 tests)
- `cargo clippy -p camel-component-wasm -- -D warnings` exits 0
- No `...` or `TODO` tokens introduced

- [x] W1

## Task W2: set_property_impl bounds via StateStore limits

### Files

- `crates/components/camel-component-wasm/src/state_store.rs` (modified — add getters)
- `crates/components/camel-component-wasm/src/host_functions.rs` (modified)

### Steps

1. In `StateStore`, add three pub(crate) getter methods:
   - `pub(crate) fn max_key_bytes(&self) -> usize { self.max_key_bytes }`
   - `pub(crate) fn max_value_bytes(&self) -> usize { self.max_value_bytes }`
   (These are plain `usize` fields already on `StateStore` from W1 — no lock
   needed to read them.)

2. In `host_functions.rs`, change `set_property_impl` from:
   ```rust
   pub(crate) fn set_property_impl(&mut self, key: String, value: String) {
       let parsed = serde_json::from_str::<Value>(&value).unwrap_or(Value::String(value));
       self.properties.insert(key, parsed);
   }
   ```
   to check `key.len() > self.state_store.max_key_bytes()` and
   `value.len() > self.state_store.max_value_bytes()` before insertion. If either
   exceeds the limit, return early without modifying `self.properties`.

   This design reads the limits from `StateStore` (already carrying them from W1)
   instead of adding new fields to `WasmHostState`. No changes to
   `create_host_state` signature, no call-site updates, no struct-literal updates.

### Tests

- **name**: `test_set_property_rejects_oversized_key`
  - **setup**: Construct `WasmHostState` with
    `state_store: StateStore::with_limits(256, 5, 65536)` (5-byte key cap). Call
    `set_property_impl("very_long_key_name", "val")`.
  - **action**: Check `self.properties` does not contain the key
  - **assert**: `state.properties.get("very_long_key_name").is_none()`
  - **command**: `cargo test -p camel-component-wasm --lib test_set_property_rejects_oversized_key`
  - **expected**: pass after implementation; fail before (current code inserts
    unconditionally)

- **name**: `test_set_property_rejects_oversized_value`
  - **setup**: Construct `WasmHostState` with
    `state_store: StateStore::with_limits(256, 1024, 5)` (5-byte value cap). Call
    `set_property_impl("k", "this_value_is_way_too_long")`.
  - **action**: Check `self.properties` does not contain the key
  - **assert**: `state.properties.get("k").is_none()`
  - **command**: `cargo test -p camel-component-wasm --lib test_set_property_rejects_oversized_value`
  - **expected**: pass after implementation; fail before

- **name**: `test_set_property_allows_within_bounds`
  - **setup**: Construct `WasmHostState` with
    `state_store: StateStore::with_limits(256, 1024, 65536)` (defaults). Call
    `set_property_impl("key", "{\"x\":true}")`.
  - **action**: Check the parsed value
  - **assert**: `state.properties.get("key")` returns `Some(&Value::Object(...))`
    (JSON parsed correctly)
  - **command**: `cargo test -p camel-component-wasm --lib test_set_property_allows_within_bounds`
  - **expected**: pass (existing behavior preserved for in-bounds values)

### Acceptance

- `cargo test -p camel-component-wasm --lib test_set_property_` passes (3 tests)
- `cargo test -p camel-component-wasm --lib` passes (all existing tests still pass)
- `cargo clippy -p camel-component-wasm -- -D warnings` exits 0

- [x] W2

## Task W3: Selective WASI registration

### Files

- `crates/components/camel-component-wasm/src/runtime.rs` (modified)
- `crates/components/camel-component-wasm/src/wasm_plugin_context.rs` (modified)
- `crates/components/camel-component-wasm/src/source_host.rs` (modified)

### Steps

1. Create a new module `wasi_surface.rs` in `crates/components/camel-component-wasm/src/`
   with a `pub(crate) fn register_minimal_wasi<T: wasmtime_wasi::WasiView>(linker:
   &mut wasmtime::component::Linker<T>) -> Result<(), wasmtime::Error>` function body:
   ```rust
   use wasmtime_wasi::clocks::{WasiClocks, WasiClocksView as _};
   use wasmtime_wasi::random::{WasiRandom, WasiRandomView as _};
   use wasmtime_wasi::p2::bindings::{clocks, random};

   pub(crate) fn register_minimal_wasi<T: wasmtime_wasi::WasiView>(
       linker: &mut wasmtime::component::Linker<T>,
   ) -> Result<(), wasmtime::Error> {
       clocks::wall_clock::add_to_linker::<T, WasiClocks>(linker, T::clocks)?;
       clocks::monotonic_clock::add_to_linker::<T, WasiClocks>(linker, T::clocks)?;
       random::random::add_to_linker::<T, WasiRandom>(linker, T::random)?;
       random::insecure::add_to_linker::<T, WasiRandom>(linker, T::random)?;
       random::insecure_seed::add_to_linker::<T, WasiRandom>(linker, T::random)?;
       Ok(())
   }
   ```
   The `WasiClocksView` and `WasiRandomView` traits provide the `clocks()` and
   `random()` accessor methods on `T: WasiView` — they must be imported (as `_`)
   to bring the trait methods into scope. This matches the wasmtime-wasi 46.0.2
   internal pattern (`p2/mod.rs:230`).
   If the `T::clocks` / `T::random` accessor syntax requires adjustment for the
   exact trait method signatures in wasmtime-wasi 46.0.2, adjust to the form
   `|t: &mut T| t.clocks()` or similar as the trait requires. The key
   invariant is: only clocks and random interfaces are registered, NOT the full
   `add_to_linker_async` surface. Register `mod wasi_surface;` in `lib.rs`.

2. In `runtime.rs` `WasmRuntime::new`, replace:
   `wasmtime_wasi::p2::add_to_linker_async(&mut linker)` with
   `crate::wasi_surface::register_minimal_wasi(&mut linker)`.

3. In `wasm_plugin_context.rs` `WasmPluginContext::new`, replace:
   `wasmtime_wasi::p2::add_to_linker_async(&mut linker)` with
   `crate::wasi_surface::register_minimal_wasi(&mut linker)`.

4. In `source_host.rs` `add_to_linker`, replace:
   `wasmtime_wasi::p2::add_to_linker_async(linker)` with
   `crate::wasi_surface::register_minimal_wasi(linker)`. Note:
   `SourceHostState` already implements `WasiView`, so the generic constraint
   is satisfied.

### Tests

- **name**: `test_no_full_wasi_registration_in_runtime`
  - **setup**: Read `runtime.rs` source
  - **action**: `grep -c 'add_to_linker_async' runtime.rs`
  - **assert**: count is 0
  - **command**: `grep -c 'add_to_linker_async' crates/components/camel-component-wasm/src/runtime.rs`
  - **expected**: 0 after implementation

- **name**: `test_no_full_wasi_registration_in_source_host`
  - **setup**: Read `source_host.rs` source
  - **action**: `grep -c 'add_to_linker_async' source_host.rs`
  - **assert**: count is 0
  - **command**: `grep -c 'add_to_linker_async' crates/components/camel-component-wasm/src/source_host.rs`
  - **expected**: 0 after implementation

- **name**: `test_no_full_wasi_registration_in_wasm_plugin_context`
  - **setup**: Read `wasm_plugin_context.rs` source
  - **action**: `grep -c 'add_to_linker_async' wasm_plugin_context.rs`
  - **assert**: count is 0
  - **command**: `grep -c 'add_to_linker_async' crates/components/camel-component-wasm/src/wasm_plugin_context.rs`
  - **expected**: 0 after implementation

- **name**: `test_register_minimal_wasi_compiles`
  - **setup**: The function must compile and link against wasmtime-wasi 46.0.2
  - **action**: `cargo build -p camel-component-wasm`
  - **assert**: exits 0
  - **command**: `cargo build -p camel-component-wasm`
  - **expected**: build succeeds

- **name**: `test_register_minimal_wasi_links_clocks_and_random`
  - **setup**: Create a `wasmtime::component::Linker<WasmHostState>` for the test
    `WasmHostState` (using defaults). Call
    `crate::wasi_surface::register_minimal_wasi(&mut linker)`.
  - **action**: Inspect the linker's defined items to confirm `wasi:clocks` and
    `wasi:random` are present. Use `linker`'s public API or wasmtime's
    `Linker::module_for_test` if available. If the linker does not expose item
    enumeration, verify by attempting to instantiate a minimal `.wasm` component
    that imports only `wasi:clocks/wall-clock` — success means the interface
    is registered.
  - **assert**: The linker accepts the clocks+random imports without error
  - **command**: `cargo test -p camel-component-wasm --lib test_register_minimal_wasi_links_clocks_and_random`
  - **expected**: pass after implementation

### Acceptance

- Zero `add_to_linker_async` calls in `crates/components/camel-component-wasm/src/`
  (runtime.rs, wasm_plugin_context.rs, source_host.rs all count 0)
- `cargo build -p camel-component-wasm` exits 0
- `cargo test -p camel-component-wasm --lib` passes
- `cargo clippy -p camel-component-wasm -- -D warnings` exits 0

- [x] W3

## Task W4: Remove inherit_stderr + CONTEXT.md update

### Files

- `crates/components/camel-component-wasm/src/runtime.rs` (modified)
- `crates/components/camel-component-wasm/src/host_functions.rs` (modified)
- `crates/components/camel-component-wasm/src/stream_bridge.rs` (modified)
- `crates/components/camel-component-wasm/src/source_consumer.rs` (modified — verify only)
- `crates/components/camel-component-wasm/CONTEXT.md` (modified)

### Steps

1. In `runtime.rs`, remove `.inherit_stderr()` from `create_host_state` (line ~154).
   The line becomes `wasi: WasiCtxBuilder::new().build(),`.

2. In `runtime.rs` test `test_wasm_host_state_creation`, remove `.inherit_stderr()`.

3. In `host_functions.rs` test host-state literal, remove `.inherit_stderr()`.

4. In `stream_bridge.rs`, remove `.inherit_stderr()` from both test host-state
   literals (2 sites).

5. Verify `source_consumer.rs:99` already has no `inherit_stderr` — no change needed.

6. Update `CONTEXT.md`:
   - Replace the F-camel-component-wasm-I4 gap text ("set_property and host-side
     StateStore allocations do not have independent size limits... This is the
     known gap F-camel-component-wasm-I4") with: "StateStore enforces
     configurable limits on key count (default 256), key byte length (default
     1024), and value byte length (default 65536). set_property_impl enforces
     key and value byte limits. Over-limit calls are rejected."
   - WASI surface section: change to "The linker registers only `wasi:clocks`
     and `wasi:random` per ADR-0050. Filesystem, sockets, CLI, environment, and
     stdio are absent from the linker."
   - Remove the text "Processor, bean, and policy guests inherit host stderr.
     Source guests do not." and the "This difference is historical" note.
   - Add: "No world inherits host stdio. Guests use `camel_call` for logging
     output per ADR-0050."

### Tests

- **name**: `test_no_inherit_stderr_in_crate`
  - **setup**: Search all `.rs` files in the crate
  - **action**: `grep -rn 'inherit_stderr' crates/components/camel-component-wasm/src/`
  - **assert**: zero matches
  - **command**: `! grep -rn 'inherit_stderr' crates/components/camel-component-wasm/src/ || echo 'FOUND'`
  - **expected**: `FOUND` is NOT printed (grep returns non-zero) after implementation

### Acceptance

- Zero `inherit_stderr` occurrences in `crates/components/camel-component-wasm/src/`
- `cargo test -p camel-component-wasm --lib` passes
- `cargo clippy -p camel-component-wasm -- -D warnings` exits 0
- `cargo fmt --check --all` exits 0
- CONTEXT.md updated with selective WASI + no stderr + bounded StateStore

- [x] W4
