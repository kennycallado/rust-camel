# Function runtime

The `camel-function` crate runs user code in isolated containers, invoked as `function:` pipeline steps. The model follows serverless functions: stateless, event-driven units scoped to one execution each.

```rust,no_run
use camel_function::{ContainerProvider, FunctionConfig, FunctionRuntimeService};
use camel_core::context::CamelContext;
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let provider = ContainerProvider::builder()
        .image("kennycallado/deno-runner:latest")
        .boot_timeout(Duration::from_secs(30))
        .build()?;

    let service = FunctionRuntimeService::with_container_provider(
        FunctionConfig::default(),
        provider,
    );

    let mut ctx = CamelContext::builder()
        .with_lifecycle(service)
        .build()
        .await?;

    ctx.start().await?;
    Ok(())
}
```

A `Function` runs in its own container. The runtime starts a Deno runner per registered runtime, then loads each `function:` step's source into that runner. The step sends an Exchange snapshot, gets an `ExchangePatch` back, and applies it to the message.

The `FunctionInvoker` is the contract the `function:` step calls. It owns the `RunnerPool` keyed by runtime, manages function registration with ref counting, and dispatches invocations. Errors map into the pipeline as `FunctionInvocationError` with `NotRegistered`, `RunnerUnavailable`, `Timeout`, or `Transport` variants.

`function:` differs from `script:`. `script:` runs synchronously inside the pipeline for predicates and simple transformations. `function:` runs out of process for logic that needs its own runtime. See [ADR-0006](../adr/0006-script-synchronous-boa-async-to-function.md) for the engine split.

The runtime manages container lifecycle through `ContainerProvider`. Spawn on first use, warm pool, health check on a background task. `ctx.stop()` triggers clean shutdown of all runners. See [ADR-0005](../adr/0005-function-out-of-process-staged-reload.md) for the staged prepare, finalize, discard flow that keeps hot reloads transactional.

<details>
<summary>YAML equivalent for a function step</summary>

The Rust code above wires the `FunctionRuntimeService` into the context. The actual `function:` step lives in a YAML route and gets loaded at startup. The body, header, and property helpers match the Deno runner API.

```yaml
routes:
  - id: "enrich-users"
    from: "timer:enrich?period=2000"
    steps:
      - set_body: "hello world"
      - function:
          runtime: deno
          timeout_ms: 5000
          source: |
            export default (camel) => {
              camel.setBody(String(camel.body()).toUpperCase());
              camel.setHeader("X-Enriched", "true");
            };
      - log: "enriched"
```

</details>

## Running the example

The `function-deno-enrich` example loads a route from YAML, spawns a Deno container, and enriches a timer-driven body. Build the runner image first:

```bash
cd crates/services/camel-function
docker build -t kennycallado/deno-runner:latest runner/
```

Then start the example with `cargo run` from `examples/function-deno-enrich`. The route fires every 2 seconds.

[Example source on GitHub](https://github.com/kennycallado/rust-camel/blob/main/examples/function-deno-enrich/)

**Reference**: [camel-function crate](https://github.com/kennycallado/rust-camel/blob/main/crates/services/camel-function/CONTEXT.md) | [ADR-0005: function out-of-process staged reload](../adr/0005-function-out-of-process-staged-reload.md)
