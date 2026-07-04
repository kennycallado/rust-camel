# WASM Guest Async Migration Guide

## What changed
The `process`, `invoke`, and `camel-call`/`camel-poll` functions are now async in the WIT.
Guests must declare their exports as `async fn`.

## Migration steps
1. Update WIT copies to match `crates/components/camel-component-wasm/wit/`
2. Change `fn process(...)` to `async fn process(...)` in your `impl Guest` block
3. Change `fn invoke(...)` to `async fn invoke(...)` (bean world)
4. `camel-call`/`camel-poll` host imports are now async — if you call them, `.await` the result
5. `get-property`/`set-property`/`host-store`/`host-load` remain sync — no change
6. Rebuild: `cargo build --target wasm32-wasip2 --release`

## What stays sync
- `get-property`, `set-property`, `host-store`, `host-load` (host imports)
- `init`, `configure`, `methods` (guest exports)
- `accept-http`, `submit-exchange`, `run` (source world — entirely sync)
- `evaluate` (authorization-policy world)

## wit-bindgen version
wit-bindgen 0.58+ supports async guest exports natively. No feature flag needed.
