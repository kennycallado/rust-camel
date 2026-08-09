# Custom component

A custom component connects rust-camel to a system the built-in components do not cover. You implement the `Component` trait, wrap it in a `ComponentBundle`, and register the bundle against a TOML config key.

## Implement the Component and Endpoint

```rust,ignore
{{#include ../../../examples/custom-component-bundle/src/main.rs:echo-component}}
```

```rust,ignore
{{#include ../../../examples/custom-component-bundle/src/main.rs:echo-endpoint}}
```

The contract layers from factory to worker. A `Component` is a factory for one URI scheme. `create_endpoint` builds an `Endpoint` for a specific URI. The `Endpoint` creates a `Consumer` for inbound traffic or a `Producer` for outbound traffic. A `Producer` is a `Service<Exchange>` that does the actual work.

`EchoComponent::scheme` returns `"echo"`, so the runtime resolves any `echo:...` URI to this component. `create_endpoint` stamps the configured prefix onto each `EchoEndpoint`. This endpoint is producer-only. `create_consumer` returns an error to signal that inbound traffic is unsupported. `create_producer` returns a `BoxProcessor` that logs the exchange body with the prefix. The exchange passes through unchanged.

## Wrap the component in a bundle

```rust,ignore
{{#include ../../../examples/custom-component-bundle/src/main.rs:echo-bundle-impl}}
```

A `ComponentBundle` owns one TOML config key and registers every scheme the bundle owns. `config_key` returns `"echo"`, which maps to `[components.echo]` in `Camel.toml`. `from_toml` deserializes the raw TOML block into `EchoConfig`. `register_all` receives a `ComponentRegistrar` and calls `register_component_dyn` for each component the bundle owns.

## Register and use the component

```rust,ignore
{{#include ../../../examples/custom-component-bundle/src/main.rs:echo-bundle-register}}
```

```yaml
{{#include ../../../examples/custom-component-bundle/routes/echo.yaml:echo-route}}
```

In `main`, read the config block from `CamelConfig` and call `register_all`. Fall back to defaults when the block is absent. The route references `echo:hello` like any built-in scheme. The timer fires every two seconds, the producer logs the body, and the exchange continues down the pipeline.

**Reference**: [Component SPI](https://github.com/kennycallado/rust-camel/blob/main/crates/components/camel-component-api/CONTEXT.md) · [Example source](https://github.com/kennycallado/rust-camel/tree/main/examples/custom-component-bundle)
