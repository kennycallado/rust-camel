# security-wasm-policy

Fully runnable example: a WASM authorization-policy plugin (`role-check`) that grants access only when the "admin" role is present in exchange properties.

## Running

```bash
cargo run -p security-wasm-policy
```

No extra setup required — the pre-built plugin is in `fixtures/role-check.wasm`.

## What it shows

- **`authorization-policy` world** — the WASM guest implements `evaluate()` returning `None` (granted) or `Some(reason)` (denied)
- **`camel.auth.roles` property** — the host sets roles on the exchange; the guest reads them via `get_property`
- **`Granted` / `Denied` decisions** — `Ok(None)` means access granted, `Ok(Some(reason))` means denied

## Route

```
secured-route:
  timer:tick?period=1000&repeatCount=5
    → AuthenticatedWasmPolicy (validate Alice JWT, store camel.auth.roles, run role-check.wasm)
    → log:info?showHeaders=true
```

Route security is applied before route steps. The example wraps `WasmSecurityPolicy` in `AuthenticatedWasmPolicy`: the wrapper validates Alice's JWT, stores `camel.auth.roles`, then delegates to `role-check.wasm`.

## How it works

1. `WasmSecurityPolicy::new()` loads `role-check.wasm` with the `authorization-policy` world
2. `AuthenticatedWasmPolicy` wraps the WASM policy with JWT authentication
3. `RouteBuilder::security_policy()` attaches the wrapper to the route
4. For each exchange, the wrapper validates Alice's JWT and stores `camel.auth.roles`
5. The guest reads `camel.auth.roles` via `get_property()`, splits by comma, and checks for "admin"
6. If found, returns `Ok(None)` (granted); otherwise `Ok(Some("role 'admin' required, got: ..."))`

## Expected output

```
=== WASM Security Policy Example ===
Plugin: role-check.wasm (authorization-policy world)
Required role: admin

Route: timer → AuthenticatedWasmPolicy(auth + WASM check) → log
Running for ~7s...

[info] Exchange[...]
[info] Exchange[...]
...

Done.
```

## Building the guest plugin

The guest source is in `guest/`. It uses `wit-bindgen` directly because `wit_bindgen::generate!` with `pub_export_macro` requires the bindings to be generated in the guest crate itself.

```bash
cd guest
cargo build --target wasm32-wasip2 --release
cp target/wasm32-wasip2/release/role_check_policy.wasm ../fixtures/role-check.wasm
```

> **Note:** The guest is excluded from the workspace (separate `[workspace]` in its Cargo.toml) because it targets `wasm32-wasip2`, not the native host.

## Files

```
examples/security-wasm-policy/
  Cargo.toml          Host example crate
  README.md           This file
  fixtures/
    role-check.wasm   Pre-built WASM plugin (authorization-policy world)
  guest/
    Cargo.toml        Guest crate (standalone workspace, wasm32-wasip2 target)
    wit/
      camel-plugin.wit  WIT interface definition (copied from camel-component-wasm)
    src/
      lib.rs          Role-check policy implementation
  src/
    main.rs          Host example: timer → AuthenticatedWasmPolicy → log
```
