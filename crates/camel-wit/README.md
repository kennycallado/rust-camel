# camel-wit

WIT interface definitions for rust-camel WASM components (processors, beans, sources, authorization policies).

Published to crates.io with raw `.wit` files included in the tarball,
making them accessible to any language toolchain.

## Rust usage

```rust
// Embed the combined WIT package as a &str
let wit: &str = camel_wit::FULL_WIT;

// Or individual worlds
let plugin: &str = camel_wit::PLUGIN_WIT;
let bean: &str = camel_wit::BEAN_WIT;
let source: &str = camel_wit::SOURCE_WIT;
```

## WIT files

- `wit/camel-all.wit` — canonical merged package (all worlds, single file)
- `wit/camel-plugin.wit` — `plugin` world standalone (also defines `authorization-policy`)
- `wit/camel-bean.wit` — `bean` world standalone
- `wit/camel-source.wit` — `source` world standalone

All four files belong to `package camel:plugin@1.0.0;`.

## Worlds

- **`plugin`** — route processor. The host drives each call; the guest exports `init`/`process`.
- **`bean`** — multi-method DI component. The guest exports `init`/`methods`/`invoke(method)`.
- **`authorization-policy`** — security backend. The guest exports `init`/`evaluate`; defined in `camel-plugin.wit`.
- **`source`** — inbound source. The guest owns the consumption loop (`configure` → `run`) over a host-granted `http-listener` resource.

## Other languages

Guest authors in Python, Go, JavaScript, and other wit-bindgen-supported
languages compile against the `camel:plugin` WIT package shipped in this crate.

### Quickstart

1. **Fetch the WIT.** Either add the crate to read the embedded strings
   (`camel_wit::PLUGIN_WIT`, `BEAN_WIT`, `SOURCE_WIT`, `FULL_WIT`), or download
   the raw `.wit` files from the crates.io source tarball or the GitHub
   repository (`crates/camel-wit/wit/`).
2. **Generate bindings.** Run your language's `wit-bindgen` against the world
   you target — one world per compilation unit:
   - `camel-plugin.wit` → `plugin` or `authorization-policy`
   - `camel-bean.wit` → `bean`
   - `camel-source.wit` → `source`
3. **Implement the guest.** Export the world entrypoints:
   - `plugin`: `init`, `process`
   - `bean`: `init`, `methods`, `invoke`
   - `authorization-policy`: `init`, `evaluate`
   - `source`: `configure`, `run` (drives the loop over a host-granted
     `http-listener`)
   
   The Rust reference guests under `examples/wasm-*/guest/` show the full
   contract shape.
4. **Compile and deploy.** Build to `wasm32-wasip2` and register the component
   with the rust-camel host.

The package follows ADR-0053: one SemVer (`camel:plugin@1.0.0`) covers every
interface and world and is independent of the Rust crate version.
