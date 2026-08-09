# API reference

Rust API documentation lives in `cargo doc`, not in this guide. Read the
rustdoc for canonical type signatures, trait contracts, and method details.

## Read the API docs locally

Build and open the rustdoc for the whole workspace from the repository root:

```console
cargo doc --open
```

For a single crate, target it by name. The `--no-deps` flag skips dependencies
and builds faster:

```console
cargo doc -p camel-api --no-deps --open
```

The local build always matches your checkout. Use it when you edit the
code or track a branch that is not released yet.

## Read the API docs online

Published crates are on docs.rs. docs.rs builds the latest released rustdoc for
each crate:

- [`camel-api`](https://docs.rs/camel-api/) - core types: `Exchange`, `Message`,
  `Body`, `CamelError`, `Processor`, `BoxProcessor`, `PipelineOutcome`, the CQRS
  bus traits, and `CanonicalRouteSpec`.
- [`camel-core`](https://docs.rs/camel-core/) - `CamelContext`, route lifecycle,
  hot reload, and the component, language, function, and service registries.
- [`camel-builder`](https://docs.rs/camel-builder/) - the fluent `RouteBuilder`
  API that constructs a `RouteDefinition` by method chaining.
- [`camel-dsl`](https://docs.rs/camel-dsl/) - YAML and JSON route parsing into
  `RouteDefinition`.

Each component, language, service, and platform crate publishes its own docs.rs
page from its package metadata. Only `camel-bench` is unpublished.

## Where each concern lives

The workspace is a monorepo of focused crates. Pick the crate that matches the
question you are asking.

| You want to read about | Open this crate |
| --- | --- |
| Core types and contracts | `camel-api` |
| Route authoring in Rust | `camel-builder` |
| Runtime, lifecycle, registries | `camel-core` |
| Declarative routes (YAML, JSON) | `camel-dsl` |
| EIP pattern implementations | `camel-processor` |
| Inbound and outbound adapters | `camel-component-api` and `components/*` |
| Expression and predicate evaluation | `camel-language-api` and `languages/*` |

For the full crate map with the domain vocabulary behind each one, see
[CONTEXT-MAP.md](https://github.com/kennycallado/rust-camel/blob/main/CONTEXT-MAP.md).
