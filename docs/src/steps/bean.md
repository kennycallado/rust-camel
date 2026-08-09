# Bean

The Bean step calls a method on a registered bean. A bean is a named Rust
object that lives in the bean registry. The step looks up the bean by name,
calls the named method, and passes exchange data to it.

The `#[bean_impl]` and `#[handler]` macros turn a Rust impl block into a bean.
Each `#[handler]` method becomes a callable entry point. The handler signature
controls how the framework extracts data from the exchange. A parameter of type
`Order` receives the deserialized body. The macro generates the binding code at
compile time, so the binding is type-checked before the route runs.

```rust,ignore
RouteBuilder::from("timer:tick?period=1000")
    .route_id("bean-demo")
    .bean("orderProcessor", "handle")
    .to("log:processed?showBody=true")
    .build()?;
```

<details>
<summary>YAML equivalent</summary>

```yaml
- id: bean-demo
  from: timer:tick?period=1000
  steps:
    - bean:
        name: orderProcessor
        method: handle
    - to: log:processed?showBody=true
```

</details>

The `.bean("name", "method")` call takes the bean name and method name as
strings. The registry resolves the name to an instance. The method receives the
exchange data and returns its result into the exchange body.

The Bean step is not an EIP. It is a Message Endpoint utility from the Hohpe and
Woolf vocabulary. In Rust, the `.process()` closure serves the same role with a
direct function reference. The Bean step exists primarily for the YAML DSL,
where a closure is not available. A route that needs custom logic in YAML
defines a bean, registers it, and calls it by name.

The Bean differs from [Script](../eip/script.md). Script runs inline code
declared in the route. Bean calls a pre-registered instance. A change to bean
behavior needs a rebuild and re-registration. A change to script behavior needs
only a route file edit.

Per [ADR-0001](../adr/0001-tower-data-plane-split-from-control-plane.md), the
step compiles into a `Service<Exchange>` in the Tower pipeline. The bean
registry contract is documented in
[camel-bean/CONTEXT.md](https://github.com/kennycallado/rust-camel/blob/main/crates/camel-bean/CONTEXT.md).
