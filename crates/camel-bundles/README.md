# camel-bundles

> Component-bundle registration cascade and boot lifecycle handle for rust-camel

Owns the `ComponentBundle::register_all` cascade extracted from `camel run`
(ADR-0069): one registration path shared by the CLI and the integration-test
harness, plus the `BootHandle` that sequences JMS/CXF pool and bridge-cleanup
teardown.

## Usage

```rust
use camel_bundles::boot;

// ctx is created and pre-configured by the caller (e.g. through
// camel_config::CamelConfig::configure_context_with_beans).
let handle = boot(&mut ctx, &config, project_root).await?;

// ... load routes, run startup checks, ctx.start() ...

handle.shutdown(&mut ctx).await?;
```

`boot` reads each `[components.<key>]` table from the `Camel.toml` behind
`config`, falling back to an empty table so every bundle registers with its
serde defaults. Feature gates mirror the `camel run` cfg lines: `grpc`,
`wasm`, `http-static`, `llm`, `surrealdb`, `mqtt`, `mcp`, and the eight
per-bridge gates `jms`, `sql`, `redis`, `opensearch`, `ws`, `cxf`, `xj`,
`xslt` are default-on; `kafka` and `security` are opt-in. `http-static`
gates the `HttpStaticBundle` registration only (see CONTEXT.md).

## Related crates

- **camel-core** — engine; owns `CamelContext` and the `add_lifecycle` seam
- **camel-config** — `Camel.toml` loading and context composition
- **camel-cli** — `camel run`, which forwards its feature gates here
