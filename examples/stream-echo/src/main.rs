//! # stream-echo
//!
//! Interactive stdio echo for rust-camel: every line read from stdin is
//! prefixed with `echo: ` and written to stdout; a `Enter text:` prompt is
//! written to stderr. The context boots through the full `camel run`
//! component cascade ([`camel_bundles::boot`], ADR-0069 section 10), which
//! registers the stream component (`stream:in`, `stream:out`, `stream:err`).
//!
//! See README.md for the interactive and piped lifecycles.

use std::path::Path;

use camel_api::CamelError;
use camel_config::{CamelConfig, discover_routes};

#[tokio::main]
async fn main() -> Result<(), CamelError> {
    // Load configuration from Camel.toml (log_level = "WARN" keeps stdout
    // clean for the stream:out data plane).
    let config =
        CamelConfig::from_file("Camel.toml").map_err(|e| CamelError::Config(e.to_string()))?;

    // Build the context; applies logging/OTel from the config.
    let mut ctx = CamelConfig::configure_context(&config).await?;

    // Boot the `camel run` component cascade on this context: every slim
    // bundle registers, including the stream component under scheme `stream`.
    let boot = camel_bundles::boot(&mut ctx, &config, Path::new(".")).await?;

    let routes = discover_routes(&config.routes).map_err(|e| CamelError::Config(e.to_string()))?;
    let route_count = routes.len();
    for route in routes {
        ctx.add_route_definition(route).await?;
    }

    // Prompt and banner go to stderr: stdout is data only.
    eprintln!("stream-echo: {route_count} route(s) started. Type lines; Ctrl-C to exit.");

    ctx.start().await?;

    // EOF on stdin completes the stream:in consumer's route, but the
    // process lifetime is signal-managed (same semantic as `camel run`,
    // crates/camel-cli/src/commands/run.rs): wait for the first signal,
    // then tear down gracefully.
    #[cfg(unix)]
    {
        let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .map_err(|e| CamelError::Io(e.to_string()))?;
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = sigterm.recv() => {}
        }
    }
    #[cfg(not(unix))]
    tokio::signal::ctrl_c()
        .await
        .map_err(|e| CamelError::Io(e.to_string()))?;

    eprintln!("stream-echo: shutting down.");
    ctx.stop().await?;
    boot.shutdown(&mut ctx).await?;
    Ok(())
}
