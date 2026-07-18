//! Domain layer of the datasource vertical slice.
//!
//! This module owns the `RuntimeDatasourceCatalog` aggregate and its
//! `CacheKey` type alias. The aggregate holds the configuration map, the
//! factory registry (`RwLock<HashMap>`), the pool cache (`DashMap`), and an
//! optional `HealthCheckRegistry` reference. These are all field types — the
//! domain does not perform I/O. The use-case orchestration (pool resolution,
//! factory matching, lazy pool creation, health-check registration) lives in
//! the sibling `application` module. The external `DatasourceCatalog` port
//! implementation lives in the sibling ring-three module.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use camel_api::datasource::{DatasourceConfig, DatasourceHandle, PoolFactory};
use dashmap::DashMap;
use tokio::sync::OnceCell;

use crate::health_registry::HealthCheckRegistry;

pub(super) type CacheKey = (String, String);

pub struct RuntimeDatasourceCatalog {
    configs: HashMap<String, DatasourceConfig>,
    factories: RwLock<HashMap<String, Arc<dyn PoolFactory>>>,
    pools: DashMap<CacheKey, Arc<OnceCell<DatasourceHandle>>>,
    health_registry: Option<Arc<HealthCheckRegistry>>,
}

impl RuntimeDatasourceCatalog {
    pub fn new(configs: HashMap<String, DatasourceConfig>) -> Self {
        Self {
            configs,
            factories: RwLock::new(HashMap::new()),
            pools: DashMap::new(),
            health_registry: None,
        }
    }

    pub fn with_health_registry(mut self, registry: Arc<HealthCheckRegistry>) -> Self {
        self.health_registry = Some(registry);
        self
    }

    /// Accessor for the use-case layer: returns a snapshot of the configs map
    /// entry (cloned, since the configs are plain data).
    pub(super) fn configs(&self) -> &HashMap<String, DatasourceConfig> {
        &self.configs
    }

    /// Accessor for the use-case layer: the pool cache, keyed by
    /// `(datasource_name, factory_name)`.
    pub(super) fn pools(&self) -> &DashMap<CacheKey, Arc<OnceCell<DatasourceHandle>>> {
        &self.pools
    }

    /// Accessor for the use-case layer: the optional health registry wired in
    /// via `with_health_registry`. Used by `get_pool` to register a
    /// `DatasourceHealthCheck` after lazy pool creation.
    pub(super) fn health_registry(&self) -> Option<&Arc<HealthCheckRegistry>> {
        self.health_registry.as_ref()
    }

    /// Accessor for the use-case layer: the factory registry (mutable form
    /// for `register_factory`, read form for `resolve_factory`).
    pub(super) fn factories(&self) -> &RwLock<HashMap<String, Arc<dyn PoolFactory>>> {
        &self.factories
    }
}
