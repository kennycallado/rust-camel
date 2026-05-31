use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::{RwLock, mpsc};
use tower_http::services::ServeDir;

use crate::RequestEnvelope;

/// A mounted static file directory with its configuration.
pub(crate) struct StaticMount {
    pub dir: PathBuf,
    pub cache_control: String,
    pub error_pages: HashMap<u16, PathBuf>,
    pub serve_dir: ServeDir,
}

/// Inner state of the HTTP route registry.
pub(crate) struct HttpRouteRegistryInner {
    pub api_routes: HashMap<String, mpsc::Sender<RequestEnvelope>>,
    pub static_mounts: Vec<StaticMount>,
    pub spa_mount: Option<StaticMount>,
}

/// Unified registry for API routes, static mounts, and SPA fallback.
///
/// Replaces the old `DispatchTable` (Arc<RwLock<HashMap>>). All lookups go
/// through a single RwLock to prevent split-lock snapshot inconsistency.
#[derive(Clone)]
pub(crate) struct HttpRouteRegistry {
    pub inner: Arc<RwLock<HttpRouteRegistryInner>>,
}

impl HttpRouteRegistry {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(HttpRouteRegistryInner {
                api_routes: HashMap::new(),
                static_mounts: Vec::new(),
                spa_mount: None,
            })),
        }
    }

    /// Register an API route path → sender mapping.
    pub async fn register_api_route(&self, path: String, sender: mpsc::Sender<RequestEnvelope>) {
        let mut inner = self.inner.write().await;
        inner.api_routes.insert(path, sender);
    }

    /// Unregister an API route path.
    pub async fn unregister_api_route(&self, path: &str) {
        let mut inner = self.inner.write().await;
        inner.api_routes.remove(path);
    }
}
