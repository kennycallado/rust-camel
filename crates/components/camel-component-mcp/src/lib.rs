//! camel-component-mcp — Model Context Protocol (MCP) component for rust-camel.
//!
//! First-class MCP support (scheme `mcp:`) with dual roles: the Consumer
//! (server) role — a shared Streamable-HTTP listener serving tool and resource
//! routes — and the Producer (client) role — `mcp:call` / `mcp:read` dispatch
//! to remote MCP servers. Protocol baseline is `2026-07-28` (stateless: no
//! `initialize` handshake, no sessions, per-request `_meta`). The rmcp SDK is
//! confined to `src/adapter/` (ADR-0020 pattern).

pub mod adapter;
pub mod bundle;
pub mod client;
pub mod component;
pub mod config;
pub mod consumer;
pub mod endpoint;
pub mod error;
pub mod headers;
pub mod producer;
pub mod registry;
pub mod types;

pub use bundle::McpBundle;
pub use component::McpComponent;
pub use registry::McpServerRegistry;
