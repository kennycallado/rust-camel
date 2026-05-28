# camel-function

Out-of-process function runtime for [rust-camel](../../../README.md).

Provides the `function:` DSL step — executes user-supplied TypeScript/JavaScript functions inside Deno containers with automatic lifecycle management, hot-reload, and structured error mapping.

See [docs/function-step.md](./docs/function-step.md) for the full DSL reference.

## Quick Start

```rust
use camel_function::{ContainerProvider, FunctionConfig, FunctionRuntimeService, PullPolicy};
use camel_core::context::CamelContext;
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let provider = ContainerProvider::builder()
        .image("kennycallado/deno-runner:latest")
        .pull_policy(PullPolicy::IfMissing) // Never | Always | IfMissing (default)
        .boot_timeout(Duration::from_secs(30)) // max wait for container /health (default: 30s)
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

### ContainerProvider options

| Builder method | Type | Default | Description |
|---|---|---|---|
| `.image(s)` | `&str` | — | Docker image to use as function runner (required) |
| `.pull_policy(p)` | `PullPolicy` | `IfMissing` | `Never`: no pull; `Always`: pull on every start; `IfMissing`: pull only if image not found locally |
| `.boot_timeout(d)` | `Duration` | 30s | Max time to wait for the container `/health` endpoint to respond after spawn |
| `.instance_id(s)` | `&str` | random UUID | Scopes container labels for test isolation |

## Runner

```bash
cd crates/services/camel-function
docker build -t kennycallado/deno-runner:latest runner/
```

Endpoints on port 8080: `GET /health`, `POST /register`, `POST /invoke`, `POST /shutdown`.

Security: `--allow-net=0.0.0.0 --allow-env=PORT` only.

## Container Lifecycle

- Containers are spawned on first function registration per runtime.
- `ContainerProvider::Drop` attempts graceful shutdown when the provider is dropped (best-effort, no panic outside tokio runtime).
- `ctx.stop()` triggers service shutdown which stops all runners.
- After stop, no containers with label `camel.function.runner=true` should remain.
- If a container leaks (e.g. process killed without graceful shutdown), re-running will create new containers; orphaned ones must be cleaned manually with `docker rm $(docker ps -q --filter label=camel.function.runner=true)`.

## Testing

```bash
cargo test -p camel-function
cargo test -p camel-function --features docker-tests
```

## License

Same as rust-camel.
