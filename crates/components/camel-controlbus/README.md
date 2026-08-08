# camel-component-controlbus

> ControlBus component for rust-camel

## Overview

The ControlBus component implements the [ControlBus EIP pattern](https://www.enterpriseintegrationpatterns.com/patterns/messaging/ControlBus.html), allowing routes to manage other routes at runtime. It provides operations to start, stop, suspend, resume, and check the status of routes.

This is a **producer-only** component - it can only be used as a destination (`to`) in routes, not as a source (`from`).

## Features

- Start, stop, suspend, resume routes
- Restart routes
- Query route status
- Dynamic route management from within routes
- Static route ID declaration in the endpoint URI (no header override)
- Runtime-bus execution path (no controller fallback)
- Per-exchange command IDs for idempotent command processing

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
camel-component-controlbus = "*"
```

## URI Format

```
controlbus:route?routeId=xxx&action=yyy&authorizedRoutes=xxx
```

## Actions

| Action | Description |
|--------|-------------|
| `start` | Start a stopped route |
| `suspend` | Suspend a route (pause consumer intake, in-flight exchanges continue) |
| `resume` | Resume a suspended route (restart consumer intake) |
| `stop` | Stop a running route (full stop) |
| `restart` | Stop and start a route |
| `status` | Get route status (returned in body) |

## Parameters

| Parameter | Required | Description |
|-----------|----------|-------------|
| `action` | Yes | Action to perform |
| `routeId` | Yes | Target route ID (must be declared in the URI; header override removed in ADR-0034) |
| `authorizedRoutes` | Yes | Comma-separated allowlist of route IDs this endpoint may target. If absent, every command is rejected (fail-closed per ADR-0034). The target `routeId` must appear in this list. |

## Usage

### Static Route ID

```rust
use camel_builder::RouteBuilder;
use camel_component_controlbus::ControlBusComponent;

let mut ctx = CamelContext::new();
ctx.register_component("controlbus", Box::new(ControlBusComponent::new()));

// Start a route
let start_route = RouteBuilder::from("timer:start-schedule?period=86400000")
    .to("controlbus:route?routeId=nightly-job&action=start&authorizedRoutes=nightly-job")
    .build()?;

// Stop a route
let stop_route = RouteBuilder::from("timer:stop-schedule?period=86400000")
    .to("controlbus:route?routeId=nightly-job&action=stop&authorizedRoutes=nightly-job")
    .build()?;
```

### Security: route ID is static (ADR-0034)

The `CamelRouteId` exchange header cannot select or override the target route. ADR-0034
removed header-based route targeting because Exchange data is untrusted (ADR-0032) and a
header-driven control plane would let any in-process caller escalate privileges by writing
the header. Route IDs and the `authorizedRoutes` allowlist MUST be declared statically in
the endpoint URI:

```
controlbus:route?routeId=my-route&action=status&authorizedRoutes=my-route
```

Authorization failures return `CamelError::Unauthorized`. The endpoint also denies
self-targeting (the calling route cannot suspend or stop itself).

### Get Route Status

```rust
let route = RouteBuilder::from("direct:check")
    .to("controlbus:route?routeId=my-route&action=status&authorizedRoutes=my-route")
    .process(|ex| async move {
        let status = ex.input.body.as_text().unwrap_or("unknown");
        println!("Route status: {}", status);
        Ok(ex)
    })
    .build()?;
```

### Suspend and Resume

```rust
// Suspend during maintenance
let suspend = RouteBuilder::from("direct:maintenance-start")
    .to("controlbus:route?routeId=api-route&action=suspend&authorizedRoutes=api-route")
    .build()?;

// Resume after maintenance
let resume = RouteBuilder::from("direct:maintenance-end")
    .to("controlbus:route?routeId=api-route&action=resume&authorizedRoutes=api-route")
    .build()?;
```

## Response Body

| Action | Body Content |
|--------|--------------|
| `status` | Route status: "Started", "Stopped", "Suspended", "Starting", "Stopping", "Failed: <message>" |
| Other actions | Empty |

## Runtime Notes

- `controlbus` requires a runtime handle in `ProducerContext`.
- `start/stop/suspend/resume/restart` commands are sent with unique command IDs derived from route, operation, and exchange correlation ID.

## Example: Scheduled Route Control

```rust
use camel_builder::RouteBuilder;
use camel_component_controlbus::ControlBusComponent;
use camel_component_timer::TimerComponent;
use camel_core::CamelContext;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut ctx = CamelContext::new();
    ctx.register_component("controlbus", Box::new(ControlBusComponent::new()));
    ctx.register_component("timer", Box::new(TimerComponent::new()));

    // The route we want to control
    let work_route = RouteBuilder::from("timer:work?period=1000")
        .route_id("work-route")
        .auto_startup(false)  // Don't start automatically
        .process(|_| async { println!("Working..."); Ok(()) })
        .build()?;

    // Route that starts the work route on demand
    let control_route = RouteBuilder::from("http://0.0.0.0:8080/control/start")
        .to("controlbus:route?routeId=work-route&action=start&authorizedRoutes=work-route")
        .set_body(Body::Text("Started work-route"))
        .build()?;

    // Route that stops the work route on demand
    let stop_route = RouteBuilder::from("http://0.0.0.0:8080/control/stop")
        .to("controlbus:route?routeId=work-route&action=stop&authorizedRoutes=work-route")
        .set_body(Body::Text("Stopped work-route"))
        .build()?;

    // Route to check status
    let status_route = RouteBuilder::from("http://0.0.0.0:8080/control/status")
        .to("controlbus:route?routeId=work-route&action=status&authorizedRoutes=work-route")
        .build()?;

    ctx.add_route(work_route).await?;
    ctx.add_route(control_route).await?;
    ctx.add_route(stop_route).await?;
    ctx.add_route(status_route).await?;

    ctx.start().await?;
    tokio::signal::ctrl_c().await?;
    ctx.stop().await?;

    Ok(())
}
```

## Error Handling

The component returns errors for:
- Unknown route ID
- Invalid action
- Missing `routeId` in the URI
- Missing `authorizedRoutes` (fail-closed)
- Target `routeId` not present in `authorizedRoutes`
- Self-targeting (calling route ID == target `routeId`)

```rust
let route = RouteBuilder::from("direct:control")
    .error_handler(ErrorHandlerConfig::log_only())
    .to("controlbus:route?routeId=maybe-nonexistent&action=start&authorizedRoutes=maybe-nonexistent")
    .build()?;
```

## Documentation

- [API Documentation](https://docs.rs/camel-component-controlbus)
- [Repository](https://github.com/kennycallado/rust-camel)

## License

Apache-2.0

## Contributing

Contributions are welcome! Please see the [main repository](https://github.com/kennycallado/rust-camel) for details.
