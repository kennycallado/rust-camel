//! Adapters layer of the datasource vertical slice.
//!
//! This module hosts the external trait delegation: the
//! `camel_api::datasource::DatasourceCatalog` port is implemented here as a
//! thin forward to the inherent use-case methods on `RuntimeDatasourceCatalog`
//! (defined in the sibling `application` module). Rust's method resolution
//! prefers inherent methods over trait methods when both are in scope with the
//! same name, so the trait impls are unambiguous forwarders.

use std::sync::Arc;

use camel_api::datasource::{DatasourceCatalog, DatasourceConfig, PoolFactory};
use camel_api::error::CamelError;

use super::domain::RuntimeDatasourceCatalog;

impl DatasourceCatalog for RuntimeDatasourceCatalog {
    fn get_config(&self, name: &str) -> Option<DatasourceConfig> {
        // Forwards to the inherent `get_config` in `application`.
        self.get_config(name)
    }

    fn get_pool<'a>(&'a self, name: &'a str) -> camel_api::datasource::GetPoolFuture<'a> {
        // Forwards to the inherent `get_pool` in `application`.
        self.get_pool(name)
    }

    fn register_factory(
        &self,
        kind: &str,
        factory: Arc<dyn PoolFactory>,
    ) -> Result<(), CamelError> {
        // Forwards to the inherent `register_factory` in `application`.
        self.register_factory(kind, factory)
    }
}
