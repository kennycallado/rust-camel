//! # custom-component-bundle
//!
//! Demonstrates how to implement a custom [`ComponentBundle`] for rust-camel.
//!
//! A `ComponentBundle` groups:
//! - One or more components (registered under their URI schemes)
//! - A TOML config key and deserializer (`from_toml`)
//! - An optional constructor argument (e.g., connection pool, shared state)
//!
//! This example defines an `EchoBundle` that registers an `EchoComponent` under
//! the `echo` scheme. Configuration is read from `[components.echo]` in Camel.toml.
//!
//! ## Camel.toml
//! ```toml
//! [components.echo]
//! prefix = ">> "
//! ```
//!
//! ## Route YAML
//! ```yaml
//! - route:
//!     id: echo-demo
//!     from: timer:tick?period=2000
//!     steps:
//!       - to: echo:hello
//! ```

use std::sync::Arc;

use camel_api::{BoxProcessor, BoxProcessorExt, CamelError};
use camel_component_api::{
    Component, ComponentBundle, ComponentContext, ComponentRegistrar, Consumer, Endpoint,
    NoOpComponentContext, ProducerContext,
};
use camel_config::{CamelConfig, discover_routes};
use serde::Deserialize;

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

/// Configuration for EchoComponent, deserialized from `[components.echo]`.
#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct EchoConfig {
    /// String prepended to every logged message.
    pub prefix: String,
}

impl Default for EchoConfig {
    fn default() -> Self {
        Self {
            prefix: "[echo] ".to_string(),
        }
    }
}

// ---------------------------------------------------------------------------
// Component
// ---------------------------------------------------------------------------

pub struct EchoComponent {
    prefix: String,
}

impl EchoComponent {
    pub fn new(prefix: impl Into<String>) -> Self {
        Self {
            prefix: prefix.into(),
        }
    }
}

impl Component for EchoComponent {
    fn scheme(&self) -> &str {
        "echo"
    }

    fn create_endpoint(
        &self,
        uri: &str,
        _ctx: &dyn ComponentContext,
    ) -> Result<Box<dyn Endpoint>, CamelError> {
        Ok(Box::new(EchoEndpoint {
            uri: uri.to_string(),
            prefix: self.prefix.clone(),
        }))
    }
}

// ---------------------------------------------------------------------------
// Endpoint
// ---------------------------------------------------------------------------

struct EchoEndpoint {
    uri: String,
    prefix: String,
}

impl Endpoint for EchoEndpoint {
    fn uri(&self) -> &str {
        &self.uri
    }

    fn create_consumer(&self) -> Result<Box<dyn Consumer>, CamelError> {
        Err(CamelError::RouteError(
            "echo component is producer-only".into(),
        ))
    }

    fn create_producer(&self, _ctx: &ProducerContext) -> Result<BoxProcessor, CamelError> {
        let prefix = self.prefix.clone();
        // Log the exchange body with the configured prefix.
        Ok(BoxProcessor::from_fn(move |exchange| {
            let prefix = prefix.clone();
            Box::pin(async move {
                let body = exchange
                    .input
                    .body
                    .as_text()
                    .unwrap_or("<non-text body>")
                    .to_string();
                tracing::info!("{}{}", prefix, body);
                Ok(exchange)
            })
        }))
    }
}

// ---------------------------------------------------------------------------
// Bundle
// ---------------------------------------------------------------------------

/// Bundles `EchoComponent` with its TOML config key `"echo"`.
pub struct EchoBundle {
    config: EchoConfig,
}

impl ComponentBundle for EchoBundle {
    fn config_key() -> &'static str {
        "echo"
    }

    fn from_toml(raw: toml::Value) -> Result<Self, CamelError> {
        let config: EchoConfig = raw
            .try_into()
            .map_err(|e: toml::de::Error| CamelError::Config(e.to_string()))?;
        Ok(Self { config })
    }

    fn register_all(self, registrar: &mut dyn ComponentRegistrar) {
        registrar.register_component_dyn(Arc::new(EchoComponent::new(self.config.prefix)));
    }
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> Result<(), CamelError> {
    tracing_subscriber::fmt()
        .with_target(false)
        .with_max_level(tracing::Level::INFO)
        .init();

    // Load Camel.toml (fall back to an empty default config if not found)
    let config: CamelConfig = CamelConfig::from_file("Camel.toml").unwrap_or_else(|_| {
        config::Config::builder()
            .build()
            .and_then(|c: config::Config| c.try_deserialize())
            .expect("failed to build default CamelConfig")
    });

    // Build context (applies logging, OTel, etc. from config)
    let mut ctx = CamelConfig::configure_context(&config)
        .await
        .map_err(|e| CamelError::Config(e.to_string()))?;

    // Register always-on built-ins
    ctx.register_component(camel_component_timer::TimerComponent::new());
    ctx.register_component(camel_component_log::LogComponent::new());
    ctx.register_component(camel_component_direct::DirectComponent::new());

    // Register EchoBundle — reads [components.echo] from Camel.toml
    if let Some(raw) = config.components.raw.get(EchoBundle::config_key()).cloned() {
        EchoBundle::from_toml(raw)?.register_all(&mut ctx);
    } else {
        // No config block → use defaults
        EchoBundle {
            config: EchoConfig::default(),
        }
        .register_all(&mut ctx);
    }

    // Demonstrate NoOpComponentContext — useful in tests and standalone checks
    let echo = EchoComponent::new("[standalone] ");
    let noop_ctx = NoOpComponentContext;
    let _ep = echo.create_endpoint("echo:test", &noop_ctx)?;
    tracing::info!("EchoComponent created endpoint with NoOpComponentContext");

    // Load routes
    let patterns = if config.routes.is_empty() {
        vec!["routes/*.yaml".to_string()]
    } else {
        config.routes.clone()
    };

    let routes = discover_routes(&patterns).map_err(|e| CamelError::Config(e.to_string()))?;

    for route in routes {
        ctx.add_route_definition(route).await?;
    }

    ctx.start().await?;
    tracing::info!("Context started — press Ctrl+C to stop");

    tokio::signal::ctrl_c()
        .await
        .map_err(|e| CamelError::Io(e.to_string()))?;

    ctx.stop().await?;
    Ok(())
}
