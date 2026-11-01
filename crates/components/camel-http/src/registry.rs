use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::PoisonError;

use arc_swap::ArcSwap;
use camel_component_api::CamelError;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tower_http::services::ServeDir;

use crate::RequestEnvelope;
use crate::rest_match::RestEndpoint;

/// Discriminates between a plain static file mount and
/// a mount that also performs SPA‑style fallback to index.html.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MountMode {
    Static,
    Spa,
}

#[allow(dead_code)]
#[derive(Clone)]
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

/// One registered consumer route: its envelope sender and the `http`-scoped
/// metrics facade built from the consumer's own runtime at `start()`.
///
/// The shared Axum server is process-global and keyed by host/port, so the
/// metrics handle must travel with the route entry rather than live on the
/// server: a server-level handle would charge requests from another context
/// to the wrong collector and the wrong effective switch.
///
/// **Internal plumbing** — subject to change without notice.
#[doc(hidden)]
#[derive(Clone)]
pub struct RouteEntry {
    pub sender: mpsc::Sender<RequestEnvelope>,
    pub metrics: camel_api::ComponentMetrics,
}

#[derive(Clone)]
pub(crate) struct HttpRouteRegistryInner {
    /// Legacy path-keyed API route registry. Used for `http:` routes
    /// registered without an `httpMethod=` URI param. Paths must be unique.
    pub api_routes: HashMap<String, RouteEntry>,
    /// Method-aware REST endpoint registry. Populated by REST-lowered
    /// `http:` routes (those whose URI carries `httpMethod=...`). Allows
    /// GET and POST on the same path to coexist, and supports path
    /// templates like `/users/{id}`. Per plan expert guidance E1.
    pub rest_endpoints: Vec<RestEndpoint<RouteEntry>>,
    pub mounts: Vec<StaticMount>,
}

impl std::fmt::Debug for HttpRouteRegistryInner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpRouteRegistryInner")
            .field("api_routes", &self.api_routes.keys())
            .field("rest_endpoints", &self.rest_endpoints.len())
            .field("mounts", &self.mounts.len())
            .finish()
    }
}

#[derive(Clone)]
pub struct HttpRouteRegistry {
    pub(crate) inner: Arc<ArcSwap<HttpRouteRegistryInner>>,
    /// Serializes clone-modify-store writers so duplicate rejection stays
    /// atomic. Shared through the Arc so all Clone'd handles gate on the SAME
    /// mutex. std Mutex: never held across `.await` (writer bodies have none).
    pub(crate) writer_gate: Arc<Mutex<()>>,
    /// Cancelled by `monitor_axum_task` when the shared Axum server task
    /// backing this registry exits unexpectedly (panic or abort). Every
    /// `HttpConsumer` hosted on that server selects on this token in its
    /// `start()` loop; on cancellation the consumer returns `Err`, which
    /// camel-core's consumer watcher converts into a per-route
    /// `CrashNotification` → `FailRoute` → supervision backoff restart
    /// (ADR-0007 route-supervised contract — shared transports must fail
    /// their hosted routes like per-route transports do).
    ///
    /// Registries constructed directly via [`HttpRouteRegistry::new`]
    /// (tests, embedding) carry a token that is never cancelled — their
    /// consumers keep the pre-fix semantics.
    pub(crate) server_exited: CancellationToken,
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

/// Bench-only re-export so `benches/registry.rs` can build REST templates
/// without access to the crate-private `rest_match` module. Not a stable
/// API.
#[doc(hidden)]
pub use crate::rest_match::PathSegment;

/// Outcome of the bench-only lookup probe. Mirrors `dispatch_handler`'s
/// read path: exact legacy hit, REST hit, ambiguous template match, or miss.
#[doc(hidden)]
pub enum ProbeLookup {
    Exact(RouteEntry),
    Rest(RouteEntry),
    Ambiguous,
    Miss,
}

impl HttpRouteRegistry {
    pub fn new() -> Self {
        Self::new_with_server_exited(CancellationToken::new())
    }

    /// Crate-private constructor used by `spawn_entry`: binds the registry
    /// to the death signal of the shared server that serves it.
    pub(crate) fn new_with_server_exited(server_exited: CancellationToken) -> Self {
        Self {
            inner: Arc::new(ArcSwap::from_pointee(HttpRouteRegistryInner {
                api_routes: HashMap::new(),
                rest_endpoints: Vec::new(),
                mounts: Vec::new(),
            })),
            writer_gate: Arc::new(Mutex::new(())),
            server_exited,
        }
    }

    pub async fn register_api_route(
        &self,
        path: String,
        sender: mpsc::Sender<RequestEnvelope>,
        metrics: camel_api::ComponentMetrics,
    ) -> Result<(), CamelError> {
        let _gate = self
            .writer_gate
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let current = self.inner.load();
        if current.api_routes.contains_key(&path) {
            return Err(CamelError::Config(format!(
                "duplicate HTTP consumer path '{path}' on this listener"
            )));
        }
        let mut next = HttpRouteRegistryInner::clone(&current);
        drop(current);
        next.api_routes.insert(path, RouteEntry { sender, metrics });
        self.inner.store(Arc::new(next));
        Ok(())
    }

    pub async fn unregister_api_route(&self, path: &str) {
        let _gate = self
            .writer_gate
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let current = self.inner.load();
        let mut next = HttpRouteRegistryInner::clone(&current);
        drop(current);
        next.api_routes.remove(path);
        self.inner.store(Arc::new(next));
    }

    /// Register a method-aware REST endpoint. Two endpoints on the same
    /// path with different methods coexist; duplicate `(method, segments)`
    /// keys are rejected without replacing the original sender.
    pub async fn register_rest_endpoint(
        &self,
        method: String,
        segments: Vec<crate::rest_match::PathSegment>,
        sender: mpsc::Sender<RequestEnvelope>,
        metrics: camel_api::ComponentMetrics,
    ) -> Result<(), CamelError> {
        let _gate = self
            .writer_gate
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let current = self.inner.load();
        if current
            .rest_endpoints
            .iter()
            .any(|ep| ep.method == method && ep.segments == segments)
        {
            let template = segments
                .iter()
                .map(|segment| match segment {
                    crate::rest_match::PathSegment::Literal(value) => value.clone(),
                    crate::rest_match::PathSegment::Param(name) => format!("{{{name}}}"),
                })
                .collect::<Vec<_>>()
                .join("/");
            return Err(CamelError::Config(format!(
                "duplicate HTTP consumer endpoint '{method} /{template}' on this listener"
            )));
        }
        let mut next = HttpRouteRegistryInner::clone(&current);
        drop(current);
        next.rest_endpoints.push(RestEndpoint {
            method,
            segments,
            payload: RouteEntry { sender, metrics },
        });
        self.inner.store(Arc::new(next));
        Ok(())
    }

    /// Remove the REST endpoint matching `(method, path_template)`.
    ///
    /// Only the endpoint with the SAME method AND segments is removed —
    /// other HTTP methods sharing the path template are preserved. Stopping
    /// the `GET /users` consumer must not tear down the live `POST /users`
    /// endpoint (the core REST multi-verb use case). Fixes review C1.
    pub async fn unregister_rest_endpoint(&self, method: &str, path_template: &str) {
        let target_segments = crate::rest_match::parse_path_template(path_template);
        let _gate = self
            .writer_gate
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let current = self.inner.load();
        let mut next = HttpRouteRegistryInner::clone(&current);
        drop(current);
        next.rest_endpoints.retain(|ep| {
            // Match BOTH method and segments: a sibling verb on the same
            // path template stays registered.
            !(ep.method == method && ep.segments == target_segments)
        });
        self.inner.store(Arc::new(next));
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
        let _gate = self
            .writer_gate
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let current = self.inner.load();
        if current
            .mounts
            .iter()
            .any(|m| m.mount_path == mount.mount_path)
        {
            return Err(CamelError::Config(format!(
                "duplicate static mount path '{}' on this port",
                mount.mount_path
            )));
        }
        let mut next = HttpRouteRegistryInner::clone(&current);
        drop(current);
        next.mounts.push(mount);
        self.inner.store(Arc::new(next));
        Ok(())
    }

    /// Unregister a static mount by its unique `mount_path`.
    #[allow(dead_code)]
    pub async fn unregister_static_mount(&self, mount_path: &str) {
        let _gate = self
            .writer_gate
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let current = self.inner.load();
        let mut next = HttpRouteRegistryInner::clone(&current);
        drop(current);
        next.mounts.retain(|m| m.mount_path != mount_path);
        self.inner.store(Arc::new(next));
    }

    /// Bench-only lookup probe — one snapshot load serving both the exact
    /// legacy check and the REST scan, exactly like `dispatch_handler`.
    /// Not a stable API; exists so `benches/registry.rs` can measure the real
    /// read path against the pre/post lock implementations.
    #[doc(hidden)]
    pub async fn probe_lookup(&self, method: &str, path: &str) -> ProbeLookup {
        let snapshot = self.inner.load();
        if let Some(sender) = snapshot.api_routes.get(path) {
            return ProbeLookup::Exact(sender.clone());
        }
        match crate::rest_match::match_endpoint(method, path, &snapshot.rest_endpoints) {
            crate::rest_match::MatchOutcome::Found(result) => ProbeLookup::Rest(result.payload),
            crate::rest_match::MatchOutcome::Ambiguous => ProbeLookup::Ambiguous,
            crate::rest_match::MatchOutcome::NotFound => ProbeLookup::Miss,
        }
    }

    /// Bench-only static-mount selection probe — loads one snapshot and
    /// returns the mode + mount_path of the longest-prefix match (selection
    /// only; no ServeDir IO). Mirrors `dispatch_static`'s mount selection.
    #[doc(hidden)]
    pub async fn probe_static_select(&self, path: &str) -> Option<(MountMode, String)> {
        let snapshot = self.inner.load();
        let mut mounts: Vec<(&str, MountMode)> = snapshot
            .mounts
            .iter()
            .map(|mount| (mount.mount_path.as_str(), mount.mode))
            .collect();
        mounts.sort_by_key(|(mount_path, _)| std::cmp::Reverse(mount_path.len()));
        mounts.into_iter().find_map(|(mount_path, mode)| {
            probe_prefix_matches_segment(path, mount_path).then(|| (mode, mount_path.to_string()))
        })
    }
}

/// Bench-only copy of `static_dispatch::prefix_matches_segment` semantics:
/// `prefix` must match `path` at a segment boundary, and the root `/`
/// matches every path.
fn probe_prefix_matches_segment(path: &str, prefix: &str) -> bool {
    if prefix == "/" {
        return true;
    }
    if !path.starts_with(prefix) {
        return false;
    }
    path.len() == prefix.len() || path.as_bytes()[prefix.len()] == b'/'
}
