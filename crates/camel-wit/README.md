# camel-wit

WIT interface definitions for rust-camel WASM plugins.

Published to crates.io with raw `.wit` files included in the tarball,
making them accessible to any language toolchain.

## Rust usage

```rust
// Embed the combined WIT package as a &str
let wit: &str = camel_wit::FULL_WIT;

// Or individual worlds
let plugin: &str = camel_wit::PLUGIN_WIT;
let bean: &str = camel_wit::BEAN_WIT;

// Filesystem path to the wit/ directory (camel-wit's own directory)
let dir: &std::path::Path = camel_wit::wit_dir();
```

## WIT files

- `wit/camel-all.wit` — canonical merged package (both worlds, single file)
- `wit/camel-plugin.wit` — `plugin` world standalone
- `wit/camel-bean.wit` — `bean` world standalone

All three files belong to `package camel:plugin;`.

## Other languages

Download the `.wit` files from the crates.io source tarball or the GitHub repository
and use with your language's WIT toolchain (`wasm-tools`, `wit-bindgen`, etc.).
