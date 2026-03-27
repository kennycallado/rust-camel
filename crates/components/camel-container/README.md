# camel-component-container

> Docker container component for [rust-camel](https://github.com/kennycallado/rust-camel)

## Overview

The Container component provides Docker container lifecycle management for rust-camel routes. It supports both **producer** operations (list, run, start, stop, remove containers) and **consumer** mode (subscribe to Docker events).

## Features

- **Batteries Included**: Auto-pull images, auto-remove containers, sensible defaults
- **Container Lifecycle**: Create, start, stop, and remove containers
- **Event Streaming**: Subscribe to Docker daemon events (formatted for readability)
- **Container Tracking**: Track created containers for cleanup on shutdown (hot-reload safe)
- **Header-based Control**: Override operations and parameters via exchange headers
- **Automatic Cleanup**: Orphaned containers are removed if start fails after create
- **Reconnection**: Consumer automatically reconnects on connection failures
- **Graceful Shutdown**: Clean cancellation via `CancellationToken`

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
camel-component-container = "0.3"
```

## URI Format

```
container:<operation>[?options]
```

## Operations

| Operation | Mode     | Description                                |
| --------- | -------- | ------------------------------------------ |
| `list`    | Producer | List all containers                        |
| `run`     | Producer | Create and start a container from an image |
| `start`   | Producer | Start an existing container                |
| `stop`    | Producer | Stop a running container                   |
| `remove`  | Producer | Remove a container                         |
| `events`  | Consumer | Subscribe to Docker daemon events          |
| `logs`    | Consumer | Stream logs from a container               |

## URI Options

| Option        | Default                       | Description                                                                                                   |
| ------------- | ----------------------------- | ------------------------------------------------------------------------------------------------------------- |
| `image`       | -                             | Container image for `run` operation                                                                           |
| `name`        | -                             | Container name for `run` operation                                                                            |
| `cmd`         | -                             | Command to run in container (e.g., `sleep 30`)                                                                |
| `ports`       | -                             | Port mappings in format `hostPort:containerPort` (e.g., `8080:80,8443:443`)                                   |
| `env`         | -                             | Environment variables in format `KEY=value` (e.g., `FOO=bar,BAZ=qux`)                                         |
| `network`     | -                             | Network mode (`bridge`, `host`, `none`, or custom network name). When unset, Docker uses `bridge` by default. |
| `containerId` | -                             | Container ID or name for `logs` consumer                                                                      |
| `follow`      | `true`                        | Follow log output (logs consumer only)                                                                        |
| `timestamps`  | `false`                       | Include timestamps in logs (logs consumer only)                                                               |
| `tail`        | `all`                         | Number of lines to show from end of logs (e.g., `100`)                                                        |
| `autoPull`    | `true`                        | Automatically pull image if not present locally                                                               |
| `autoRemove`  | `true`                        | Automatically remove container when it exits                                                                  |
| `host`        | `unix:///var/run/docker.sock` | Docker host (Unix socket only)                                                                                |

## Headers

### Input Headers (for operations)

| Header                 | Description                                                   |
| ---------------------- | ------------------------------------------------------------- |
| `CamelContainerAction` | Override operation (`list`, `run`, `start`, `stop`, `remove`) |
| `CamelContainerImage`  | Container image (required for `run`)                          |
| `CamelContainerId`     | Container ID (required for `start`, `stop`, `remove`)         |
| `CamelContainerName`   | Container name (optional for `run`)                           |

### Output Headers (from responses)

| Header                       | Description                                                         |
| ---------------------------- | ------------------------------------------------------------------- |
| `CamelContainerActionResult` | Operation result (`success`)                                        |
| `CamelContainerId`           | Created container ID (from `run`), or container ID for logs         |
| `CamelContainerLogStream`    | Log stream type: `stdout`, `stderr`, `console` (logs consumer only) |
| `CamelContainerLogTimestamp` | Log timestamp when `timestamps=true` (logs consumer only)           |

## Usage

### Simple Container Run (Batteries Included)

The simplest way to run a container - just specify the image:

```rust
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_container::ContainerComponent;

let route = RouteBuilder::from("timer:tick?period=60000")
    .to("container:run?image=alpine:latest&cmd=sleep 30&autoRemove=true")
    .to("log:info?showHeaders=true")
    .build()?;
```

This will:

1. Pull `alpine:latest` if not present (`autoPull=true` by default)
2. Run `sleep 30` in the container
3. Auto-remove the container when it exits (`autoRemove=true`)

### Listing Containers

```rust
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_container::ContainerComponent;

let route = RouteBuilder::from("timer:tick?period=60000")
    .to("container:list")
    .to("log:info?showBody=true")
    .build()?;
```

### Running a Container with Custom Name

```rust
let route = RouteBuilder::from("direct:run-container")
    .set_header("CamelContainerImage", Value::String("alpine:latest".into()))
    .set_header("CamelContainerName", Value::String("my-alpine".into()))
    .to("container:run")
    .to("log:info?showHeaders=true")
    .build()?;
```

### Running a Web Server with Port Mapping

```rust
// Run nginx with port 8080 on host → 80 in container
let route = RouteBuilder::from("timer:start?delay=5000&repeatCount=1")
    .to("container:run?image=nginx:latest&ports=8080:80&autoRemove=false")
    .to("log:info?message=Web server running at http://localhost:8080")
    .build()?;
```

> **Note**: If port mapping doesn't work on your system (firewall issues), use `network=host` instead. The container will share the host's network stack and ports don't need mapping:
>
> ```rust
> .to("container:run?image=nginx:latest&network=host&autoRemove=false")
> // nginx will be accessible at http://localhost:80
> ```

### Running a Container with Environment Variables

```rust
// Run a container with custom environment variables
let route = RouteBuilder::from("timer:start?delay=5000&repeatCount=1")
    .to("container:run?image=postgres:15&env=POSTGRES_PASSWORD=secret,POSTGRES_DB=mydb&ports=5432:5432")
    .to("log:info?message=Database running on localhost:5432")
    .build()?;
```

### Stopping and Removing a Container

```rust
let route = RouteBuilder::from("direct:cleanup")
    .set_header("CamelContainerId", Value::String("container-123".into()))
    .to("container:stop")
    .to("container:remove")
    .build()?;
```

### Subscribing to Docker Events

Events are formatted for readability:

```
[CREATE] Container my-container (alpine:latest)
[START]  Container my-container
[DIE]    Container my-container (exit: 0)
[DESTROY] Container my-container
```

```rust
let route = RouteBuilder::from("container:events")
    .to("log:info?showBody=true")
    .build()?;
```

### Dynamic Operation via Header

```rust
// Override operation at runtime
let route = RouteBuilder::from("direct:container-op")
    .set_header("CamelContainerAction", Value::String("list".into()))
    .to("container:run")  // Base URI, overridden by header
    .build()?;
```

### Streaming Container Logs

The `logs` consumer streams log output from a container:

```rust
// Stream logs from a container
let route = RouteBuilder::from("container:logs?containerId=my-app&timestamps=true")
    .to("log:info?showHeaders=true")
    .build()?;
```

Each log line becomes an exchange with:

- **Body**: The log line content
- **CamelContainerId**: Container ID/name
- **CamelContainerLogStream**: `stdout` or `stderr`
- **CamelContainerLogTimestamp**: Timestamp (when `timestamps=true`)

Options:

- `follow=true` (default): Stream new logs as they arrive
- `follow=false`: Get historical logs and exit
- `tail=100`: Get last 100 lines only
- `timestamps=true`: Include Docker timestamps

```yaml
# YAML DSL example
routes:
  - id: "nginx-logs"
    from: "container:logs?containerId=nginx-demo&timestamps=true"
    steps:
      - log: "[${header.CamelContainerLogStream}] ${body}"
```

## Error Handling

The component provides clear, actionable error messages:

- **Image not found**: `Image 'foo' not found locally. Set autoPull=true to pull automatically, or run: docker pull foo`
- **Auth required**: `Authentication required for image 'private/image'. Configure Docker credentials: docker login`
- **Container conflict**: `Container name 'my-container' already exists. Use a unique name or remove the existing container first`
- **Missing headers**: Specific messages indicating which header is required
- **Connection failures**: Clear Docker daemon connection errors

## Known Limitations

- **Unix socket only**: TCP/TLS Docker hosts are not supported
- **No event filtering**: The `events` consumer receives all Docker events
- **Limited `run` options**: Volumes and advanced networking options are not configurable
- **No image operations**: `pull`, `build`, `push`, `tag` are not implemented (use `autoPull=true` instead)
- **No `exec`**: Running commands inside containers is not supported

## Example: Full Lifecycle

```rust
use std::time::{SystemTime, UNIX_EPOCH};
use camel_api::{CamelError, Value};
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_container::{ContainerComponent, HEADER_CONTAINER_NAME};
use camel_component_direct::DirectComponent;
use camel_component_log::LogComponent;
use camel_component_timer::TimerComponent;
use camel_core::context::CamelContext;

#[tokio::main]
async fn main() -> Result<(), CamelError> {
    let mut ctx = CamelContext::new();
    ctx.register_component(TimerComponent::new());
    ctx.register_component(LogComponent::new());
    ctx.register_component(DirectComponent::new());
    ctx.register_component(ContainerComponent::new());

    // Run a container every 20s with auto-pull and auto-remove
    let route = RouteBuilder::from("timer:lifecycle?period=20000")
        .route_id("container-lifecycle")
        .process(|mut exchange| async move {
            let timestamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_millis();
            let name = format!("example-{}", timestamp);
            exchange.input.set_header(HEADER_CONTAINER_NAME, Value::String(name));
            Ok(exchange)
        })
        .to("container:run?image=alpine:latest&cmd=sleep 15&autoRemove=true")
        .to("log:info?showHeaders=true")
        .build()?;

    ctx.add_route_definition(route).await?;
    ctx.start().await?;

    tokio::signal::ctrl_c().await.ok();
    ctx.stop().await?;
    Ok(())
}
```

## YAML DSL

The container component works with the YAML DSL:

```yaml
routes:
  - id: "container-monitor"
    from: "timer:tick?period=60000"
    steps:
      - log: "Listing containers..."
      - to: "container:list"
      - to: "log:info?showBody=true"

  - id: "run-container"
    from: "timer:run?period=30000"
    steps:
      - to: "container:run?image=alpine:latest&cmd=sleep 10&autoRemove=true"
      - to: "log:info?showHeaders=true"

  - id: "container-events"
    from: "container:events"
    steps:
      - log: "${body}"
```

## Cleanup on Shutdown

Containers created via `run` operation are tracked globally. Call `cleanup_tracked_containers()` on shutdown to force-remove any containers that are still running:

```rust
use camel_component_container::cleanup_tracked_containers;

#[tokio::main]
async fn main() -> Result<(), CamelError> {
    // ... setup routes and context ...

    ctx.start().await?;

    tokio::signal::ctrl_c().await.ok();
    ctx.stop().await?;

    // Cleanup any tracked containers (important for hot-reload)
    cleanup_tracked_containers().await;

    Ok(())
}
```

This is especially important for:

- **Hot-reload scenarios**: Prevents orphaned containers when the application restarts
- **Long-running containers**: Ensures cleanup even if containers haven't finished naturally
- **Graceful shutdown**: Guarantees no containers are left behind

Note: With `autoRemove=true` (the default), Docker will automatically remove containers when they exit naturally. The cleanup function handles cases where the application exits before containers finish.

## Global Configuration

Configure default Docker host in `Camel.toml` that applies to all Container endpoints:

```toml
[default.components.container]
docker_host = "unix:///var/run/docker.sock"  # Docker daemon socket (default)
```

### Remote Docker Host

```toml
[production.components.container]
docker_host = "tcp://docker-proxy:2375"  # TCP socket (note: only Unix sockets fully supported)
```

URI parameter `host` always overrides global default:

```rust
// Uses global docker_host
.to("container:list")

// Overrides to use different host
.to("container:list?host=unix:///var/run/docker.sock")
```

## Documentation

- [API Documentation](https://docs.rs/camel-component-container)
- [Repository](https://github.com/kennycallado/rust-camel)

## License

Apache-2.0

## Contributing

Contributions are welcome! Please see the [main repository](https://github.com/kennycallado/rust-camel) for details.
