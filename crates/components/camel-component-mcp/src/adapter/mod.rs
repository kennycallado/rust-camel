//! rmcp adapter boundary (ADR-0020): the only module that touches the rmcp SDK.
//!
//! Everything outside `src/adapter/` speaks Camel-shaped types
//! (`McpClient`, `McpToolResult`, `McpResource`, `McpError`); the rmcp
//! client/server wiring is confined here so the SDK stays swappable.

pub mod client;
pub mod server;

pub use client::RmcpClient;
pub use server::McpServerAdapter;
