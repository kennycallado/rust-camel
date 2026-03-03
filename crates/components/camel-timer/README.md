# camel-component-timer

> Timer component for rust-camel

## Overview

The Timer component produces exchanges at periodic intervals. It's useful for scheduling tasks, generating periodic events, and triggering routes on a time-based schedule.

This is a **consumer-only** component - it can only be used as a source (`from`) in routes, not as a destination (`to`).

## Features

- Periodic tick generation
- Configurable initial delay
- Limited or unlimited repetition
- Cancellation support
- Header metadata (name, counter)

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
camel-component-timer = "0.2"
```

## URI Format

```
timer:name[?options]
```

## URI Options

| Option | Default | Description |
|--------|---------|-------------|
| `period` | `1000` | Interval between ticks in milliseconds |
| `delay` | `0` | Initial delay before first tick in milliseconds |
| `repeatCount` | unlimited | Maximum number of ticks |

## Usage

### Basic Timer

```rust
use camel_builder::RouteBuilder;
use camel_component_timer::TimerComponent;
use camel_core::CamelContext;

let mut ctx = CamelContext::new();
ctx.register_component("timer", Box::new(TimerComponent::new()));

// Fire every second
let route = RouteBuilder::from("timer:tick?period=1000")
    .log("Timer fired!", camel_processor::LogLevel::Info)
    .to("mock:result")
    .build()?;
```

### With Initial Delay

```rust
// Wait 5 seconds, then fire every 10 seconds
let route = RouteBuilder::from("timer:delayed?delay=5000&period=10000")
    .to("log:info")
    .build()?;
```

### Limited Repetition

```rust
// Fire exactly 3 times
let route = RouteBuilder::from("timer:limited?repeatCount=3&period=1000")
    .to("mock:result")
    .build()?;
```

## Exchange Headers

The timer sets the following headers on each exchange:

| Header | Type | Description |
|--------|------|-------------|
| `CamelTimerName` | String | Timer name from URI |
| `CamelTimerCounter` | Number | Tick counter (1, 2, 3...) |

## Example: Scheduled Task

```rust
use camel_builder::RouteBuilder;
use camel_component_timer::TimerComponent;
use camel_component_log::LogComponent;
use camel_core::CamelContext;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut ctx = CamelContext::new();
    ctx.register_component("timer", Box::new(TimerComponent::new()));
    ctx.register_component("log", Box::new(LogComponent::new()));

    // Run every minute
    let route = RouteBuilder::from("timer:cleanup?period=60000")
        .process(|ex| async move {
            println!("Running cleanup task...");
            Ok(ex)
        })
        .to("log:info")
        .build()?;

    ctx.add_route(route).await?;
    ctx.start().await?;

    tokio::signal::ctrl_c().await?;
    ctx.stop().await?;

    Ok(())
}
```

## Documentation

- [API Documentation](https://docs.rs/camel-component-timer)
- [Repository](https://github.com/kennycallado/rust-camel)

## License

Apache-2.0

## Contributing

Contributions are welcome! Please see the [main repository](https://github.com/kennycallado/rust-camel) for details.
