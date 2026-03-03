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
- Support for routeId from URI or header

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
camel-component-controlbus = "0.2"
```

## URI Format

```
controlbus:route?routeId=xxx&action=yyy
```

## Actions

| Action | Description |
|--------|-------------|
| `start` | Start a stopped route |
| `stop` | Stop a running route |
| `suspend` | Suspend a route (pause consumers) |
| `resume` | Resume a suspended route |
| `restart` | Stop and start a route |
| `status` | Get route status (returned in body) |

## Parameters

| Parameter | Required | Description |
|-----------|----------|-------------|
| `action` | Yes | Action to perform |
| `routeId` | No* | Target route ID (*required unless using header) |

## Usage

### Static Route ID

```rust
use camel_builder::RouteBuilder;
use camel_component_controlbus::ControlBusComponent;

let mut ctx = CamelContext::new();
ctx.register_component("controlbus", Box::new(ControlBusComponent::new()));

// Start a route
let start_route = RouteBuilder::from("timer:start-schedule?period=86400000")
    .to("controlbus:route?routeId=nightly-job&action=start")
    .build()?;

// Stop a route
let stop_route = RouteBuilder::from("timer:stop-schedule?period=86400000")
    .to("controlbus:route?routeId=nightly-job&action=stop")
    .build()?;
```

### Dynamic Route ID (from Header)

```rust
// Use CamelRouteId header for route ID
let route = RouteBuilder::from("direct:control")
    .set_header("CamelRouteId", Value::String("target-route".into()))
    .to("controlbus:route?action=status")
    .log("Route status: ${body}", camel_processor::LogLevel::Info)
    .build()?;
```

### Get Route Status

```rust
let route = RouteBuilder::from("direct:check")
    .to("controlbus:route?routeId=my-route&action=status")
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
    .to("controlbus:route?routeId=api-route&action=suspend")
    .build()?;

// Resume after maintenance
let resume = RouteBuilder::from("direct:maintenance-end")
    .to("controlbus:route?routeId=api-route&action=resume")
    .build()?;
```

## Response Body

| Action | Body Content |
|--------|--------------|
| `status` | Route status: "Started", "Stopped", "Suspended", "Starting", "Stopping", "Failed: <message>" |
| Other actions | Empty |

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
        .to("controlbus:route?routeId=work-route&action=start")
        .set_body(Body::Text("Started work-route"))
        .build()?;

    // Route that stops the work route on demand
    let stop_route = RouteBuilder::from("http://0.0.0.0:8080/control/stop")
        .to("controlbus:route?routeId=work-route&action=stop")
        .set_body(Body::Text("Stopped work-route"))
        .build()?;

    // Route to check status
    let status_route = RouteBuilder::from("http://0.0.0.0:8080/control/status")
        .to("controlbus:route?routeId=work-route&action=status")
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
- Missing routeId (when not in header)

```rust
let route = RouteBuilder::from("direct:control")
    .error_handler(ErrorHandlerConfig::log_only())
    .to("controlbus:route?routeId=maybe-nonexistent&action=start")
    .build()?;
```

## Documentation

- [API Documentation](https://docs.rs/camel-component-controlbus)
- [Repository](https://github.com/kennycallado/rust-camel)

## License

Apache-2.0

## Contributing

Contributions are welcome! Please see the [main repository](https://github.com/kennycallado/rust-camel) for details.
