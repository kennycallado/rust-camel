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

// Filesystem path to the wit/ directory (camel-wit's own directory)
let dir: &std::path::Path = camel_wit::wit_dir();
```

## WIT files

- `wit/camel-all.wit` — canonical merged package (all worlds, single file)
- `wit/camel-plugin.wit` — `plugin` world standalone (also defines `authorization-policy`)
- `wit/camel-bean.wit` — `bean` world standalone
- `wit/camel-source.wit` — `source` world standalone

All four files belong to `package camel:plugin;`.

## Worlds

- **`plugin`** — route processor. The host drives each call; the guest exports `init`/`process`.
- **`bean`** — multi-method DI component. The guest exports `init`/`invoke(method)`.
- **`authorization-policy`** — security backend. The guest exports `init`/`evaluate`; defined in `camel-plugin.wit`.
- **`source`** — inbound source. The guest owns the consumption loop (`configure` → `run`) over a host-granted `http-listener` resource.

## Other languages

Download the `.wit` files from the crates.io source tarball or the GitHub repository
and use with your language's WIT toolchain (`wasm-tools`, `wit-bindgen`, etc.).
