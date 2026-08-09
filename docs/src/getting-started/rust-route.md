# First route in Rust

Build a route that produces five log messages from a timer. The route stamps
a header onto each message and prints it through the log component.

The code comes from the compiled
[`hello-world`](https://github.com/kennycallado/rust-camel/tree/main/examples/hello-world)
example.

## The complete route

```rust,ignore
{{#include ../../../examples/hello-world/src/main.rs:first-route}}
```

<details>
<summary>YAML equivalent</summary>

```yaml
routes:
  - id: "hello-world"
    from: "timer:tick?period=1000&repeatCount=5"
    steps:
      - set_header:
          key: "source"
          value: "timer"
      - to: "log:info?showHeaders=true&showCorrelationId=true"
```

</details>

## Dependencies

Add these crates to your `Cargo.toml`:

```toml
{{#include ../../../examples/hello-world/Cargo.toml:dependencies}}
```

Each crate has one job. `camel-builder` gives you the fluent `RouteBuilder`
API. `camel-core` gives you `CamelContext`, the runtime that owns routes and
components. `camel-component-timer` and `camel-component-log` provide the two
endpoints the route connects. `camel-api` provides the shared types `Value`
and `CamelError`. `tokio` runs the async runtime. `tracing-subscriber`
formats the log output.

## How it works

### Build the context

`CamelContext::builder().build().await` constructs the runtime. The context
is the composition root for the whole process. It holds the component,
language, function, and service registries. It also controls route
lifecycle: start, stop, suspend, and resume. You create one context per
process.

The route references two endpoint schemes, `timer` and `log`. The context
resolves a scheme to a component only after you register that component.
`ctx.register_component(TimerComponent::new())` registers the `timer`
scheme. `ctx.register_component(LogComponent::new())` registers the `log`
scheme. Without registration, `RouteBuilder::from("timer:...")` fails at
build time with an unknown scheme.

### Author the route

`RouteBuilder::from("timer:tick?period=1000&repeatCount=5")` opens a route
and attaches a timer consumer. The endpoint URI has three parts. `timer` is
the component scheme. `tick` is the endpoint name inside the component. The
query string configures the schedule: `period=1000` fires once per second,
and `repeatCount=5` stops the timer after five ticks.

`.route_id("hello-world")` names the route. Named routes are easier to
inspect and to stop individually at runtime.

`.set_header("source", Value::String("timer".into()))` stamps a header onto
every exchange. The timer consumer creates one exchange per tick. The
header travels with the exchange so downstream steps can read it.

`.to("log:info?showHeaders=true&showCorrelationId=true")` sends each
exchange to a log producer. The `log` component formats the exchange body
and writes it through `tracing`. The query parameters tell the component to
include the headers and the correlation ID in each output line.

`.build()` consumes the builder and returns a `RouteDefinition`. The
builder is a single-shot object. You cannot clone or reuse it after build.

### Register and start the route

`ctx.add_route_definition(route).await` hands the route to the context. The
context stores the route but does not start it.

`ctx.start().await` starts every registered route. The timer consumer
begins to fire. Each tick produces an exchange, the route stamps the
header, and the log component writes a line.

`tokio::signal::ctrl_c().await` blocks the main task until you press
Ctrl+C. `ctx.stop().await` then shuts the context down cleanly.

## Run it

```console
cargo run -p hello-world
```

The timer fires once per second. After five ticks it stops producing. The
program keeps running until you press Ctrl+C.

The output shows five log lines. Each line carries the `source` header and
a correlation ID that traces the exchange through the pipeline.

## Next steps

- [First route in YAML](yaml-route.md): the same route in declarative form.
- [CLI usage](cli.md): run, scaffold, and inspect routes from the terminal.
- [Core concepts](../concepts/index.md): the Exchange, Message, and
  CamelContext data model.

**Reference**: [camel-builder](https://github.com/kennycallado/rust-camel/blob/main/crates/camel-builder/CONTEXT.md),
[camel-core](https://github.com/kennycallado/rust-camel/blob/main/crates/camel-core/CONTEXT.md)
