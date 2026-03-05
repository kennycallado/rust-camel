//! # Camel Core
//!
//! This crate provides the core functionality for the Apache Camel implementation in Rust.
//!
//! ## Tracer EIP
//!
//! The Tracer Enterprise Integration Pattern (EIP) provides automatic message flow tracing
//! throughout your Camel routes. It captures detailed information about each step as messages
//! flow through the integration routes, helping with debugging, monitoring, and observability.
//!
//! ### Configuration
//!
//! You can configure the tracer in your `Camel.toml` file:
//!
//! ```toml
//! [observability.tracer]
//! enabled = true
//! detail_level = "minimal"  # minimal | medium | full
//!
//! [observability.tracer.outputs.stdout]
//! enabled = true
//! format = "json"
//! ```
//!
//! Or enable it programmatically:
//!
//! ```rust
//! use camel_core::CamelContext;
//! let mut ctx = CamelContext::new();
//! ctx.set_tracing(true);
//! ```
//!
//! ### Span Fields
//!
//! Each trace span includes the following fields:
//!
//! - `correlation_id`: Unique identifier that links all spans in a single message flow
//! - `route_id`: Identifier for the route being traced
//! - `step_id`: Unique identifier for this specific step in the route
//! - `step_index`: Sequential index of this step within the route
//! - `timestamp`: When the step was executed (Unix timestamp)
//! - `duration_ms`: How long the step took to execute in milliseconds
//! - `status`: The status of the step execution (e.g., "success", "error")
//!
//! ### Detail Levels
//!
//! The tracer supports three levels of detail:
//!
//! - **Minimal**: Includes only the base fields listed above
//! - **Medium**: Includes the base fields plus:
//!   - `headers_count`: Number of message headers
//!   - `body_type`: Type of the message body
//!   - `has_error`: Whether the message contains an error
//!   - `output_body_type`: Type of the output body after processing
//! - **Full**: Includes all fields from Minimal and Medium plus:
//!   - Up to 3 message headers (`header_0`, `header_1`, `header_2`)
//!
//! ### OpenTelemetry Support
//!
//! OpenTelemetry output is prepared in the configuration types but not yet implemented.
//! It is planned for a future release.
//!
//! //! Configuration types for the Tracer EIP live in `camel-core` rather than `camel-config`
//! //! to avoid a circular dependency — `camel-config` depends on `camel-core`.
//!
pub mod config;
pub mod context;
pub mod registry;
pub mod reload;
pub mod route;
pub mod route_controller;
pub mod supervising_route_controller;
pub mod tracer;

pub use config::{
    DetailLevel, FileOutput, OpenTelemetryOutput, OutputFormat, StdoutOutput, TracerConfig,
    TracerOutputs,
};
pub use context::CamelContext;
pub use registry::Registry;
pub use route::{Route, RouteDefinition};
pub use route_controller::{DefaultRouteController, RouteControllerInternal};
pub use supervising_route_controller::SupervisingRouteController;
pub use tracer::TracingProcessor;

// Re-export route controller types from camel-api (they live there to avoid cyclic dependencies).
pub use camel_api::{RouteAction, RouteController, RouteStatus};
