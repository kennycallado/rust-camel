## Route Loading
 YAML route definitions are loaded via `camel-dsl`. See the crate's documentation for the full YAML DSL syntax and available step types:
 language expressions.

```yaml
routes:
  - id: "hello-timer"
    from: "timer:tick?period=2000"
    steps:
      - log: "Timer fired!"
      - to: "log:info"
```

Load routes with `camel-dsl`:

```rust
use camel_dsl::load_from_file;
use camel_core::context::CamelContext;

let routes = load_from_file("routes.yaml")?;
for route in routes {
    ctx.add_route_definition(route)?;
}
```

## Route Definition Format
 YAML files are parsed by `camel-dsl` and compiled into `RouteDefinition` objects. The file format is documented in the `camel-dsl` crate.

 See `examples/yaml-dsl/` for complete examples of all features.

 See [API Documentation](https://docs.rs/camel-dsl) for full details.

