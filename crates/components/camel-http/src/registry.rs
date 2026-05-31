use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use camel_component_api::CamelError;
use tokio::sync::{RwLock, mpsc};
use tower_http::services::ServeDir;

use crate::RequestEnvelope;

/// Discriminates between a plain static file mount and
/// a mount that also performs SPA‑style fallback to index.html.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MountMode {
    Static,
    Spa,
}

#[allow(dead_code)]
pub struct StaticMount {
    pub mount_path: String,
    pub mode: MountMode,
    pub dir: PathBuf,
    pub cache_control: String,
    pub error_pages: HashMap<u16, PathBuf>,
    pub serve_dir: ServeDir,
}

impl std::fmt::Debug for StaticMount {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StaticMount")
            .field("mount_path", &self.mount_path)
            .field("mode", &self.mode)
            .field("dir", &self.dir)
            .field("cache_control", &self.cache_control)
            .field("error_pages", &self.error_pages)
            .finish_non_exhaustive()
    }
}

pub(crate) struct HttpRouteRegistryInner {
    pub api_routes: HashMap<String, mpsc::Sender<RequestEnvelope>>,
    pub mounts: Vec<StaticMount>,
}

impl std::fmt::Debug for HttpRouteRegistryInner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpRouteRegistryInner")
            .field("api_routes", &self.api_routes.keys())
            .field("mounts", &self.mounts.len())
            .finish()
    }
}

#[derive(Clone)]
pub struct HttpRouteRegistry {
    pub(crate) inner: Arc<RwLock<HttpRouteRegistryInner>>,
}

impl std::fmt::Debug for HttpRouteRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpRouteRegistry").finish_non_exhaustive()
    }
}

impl Default for HttpRouteRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl HttpRouteRegistry {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(HttpRouteRegistryInner {
                api_routes: HashMap::new(),
                mounts: Vec::new(),
            })),
        }
    }

    pub async fn register_api_route(&self, path: String, sender: mpsc::Sender<RequestEnvelope>) {
        let mut inner = self.inner.write().await;
        inner.api_routes.insert(path, sender);
    }

    pub async fn unregister_api_route(&self, path: &str) {
        let mut inner = self.inner.write().await;
        inner.api_routes.remove(path);
    }

    /// Register a static mount. Duplicate detection is by `mount_path`
    /// only — every mount on a given port must have a unique prefix.
    ///
    /// A single SPA mount is still the convention, but it is no longer
    /// enforced structurally; the dispatch loop treats all mounts
    /// uniformly (sorted by longest prefix) and uses `mode` to decide
    /// whether to attempt SPA-fallback after ServeDir fails.
    #[allow(dead_code)]
    pub async fn register_static_mount(&self, mount: StaticMount) -> Result<(), CamelError> {
        let mut inner = self.inner.write().await;
        if inner
            .mounts
            .iter()
            .any(|m| m.mount_path == mount.mount_path)
        {
            return Err(CamelError::Config(format!(
                "duplicate static mount path '{}' on this port",
                mount.mount_path
            )));
        }
        inner.mounts.push(mount);
        Ok(())
    }

    /// Unregister a static mount by its unique `mount_path`.
    #[allow(dead_code)]
    pub async fn unregister_static_mount(&self, mount_path: &str) {
        let mut inner = self.inner.write().await;
        inner.mounts.retain(|m| m.mount_path != mount_path);
    }
}
