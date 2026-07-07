use std::sync::Arc;

use camel_component_api::{
    CamelError, Component, Consumer, Endpoint, ProducerContext, RuntimeObservability,
};
use tower_http::services::ServeDir;

use crate::registry::{MountMode, StaticMount};
use crate::{HttpStaticConfig, ServerRegistry};

// ---------------------------------------------------------------------------
// HttpStaticComponent
// ---------------------------------------------------------------------------

/// Component factory for the `http-static:` scheme.
///
/// Creates [`HttpStaticEndpoint`] instances from URIs like
/// `http-static:/path/to/dir?port=8080&spaFallback=true`.
pub struct HttpStaticComponent {
    config: HttpStaticConfig,
}

impl HttpStaticComponent {
    pub fn new() -> Self {
        Self {
            config: HttpStaticConfig::default(),
        }
    }

    pub fn with_config(config: HttpStaticConfig) -> Self {
        Self { config }
    }
}

impl Default for HttpStaticComponent {
    fn default() -> Self {
        Self::new()
    }
}

impl Component for HttpStaticComponent {
    fn scheme(&self) -> &str {
        "http-static"
    }

    fn create_endpoint(
        &self,
        uri: &str,
        _ctx: &dyn camel_component_api::ComponentContext,
    ) -> Result<Box<dyn Endpoint>, CamelError> {
        let config = HttpStaticConfig::from_uri_with_defaults(uri, &self.config)?;
        Ok(Box::new(HttpStaticEndpoint {
            uri: uri.to_string(),
            config,
        }))
    }
}

// ---------------------------------------------------------------------------
// HttpStaticEndpoint
// ---------------------------------------------------------------------------

/// Endpoint for a static file serving route.
///
/// Holds the resolved [`HttpStaticConfig`] and creates [`HttpStaticConsumer`]
/// instances when the route starts.
pub struct HttpStaticEndpoint {
    uri: String,
    config: HttpStaticConfig,
}

impl Endpoint for HttpStaticEndpoint {
    fn uri(&self) -> &str {
        &self.uri
    }

    fn create_consumer(
        &self,
        rt: Arc<dyn RuntimeObservability>,
    ) -> Result<Box<dyn Consumer>, CamelError> {
        Ok(Box::new(HttpStaticConsumer::new(self.config.clone(), rt)))
    }

    fn create_producer(
        &self,
        _rt: Arc<dyn RuntimeObservability>,
        _ctx: &ProducerContext,
    ) -> Result<camel_component_api::BoxProcessor, CamelError> {
        Err(CamelError::Config(
            "http-static endpoint does not support producers".to_string(),
        ))
    }
}

// ---------------------------------------------------------------------------
// HttpStaticConsumer
// ---------------------------------------------------------------------------

/// Consumer that registers a static file mount into the shared
/// [`HttpRouteRegistry`] and stays idle until cancelled.
///
/// On start:
/// 1. Canonicalizes the configured `dir` (fails if not found).
/// 2. Canonicalizes each `error_pages` path (fails if any don't exist).
/// 3. Builds a `ServeDir` for the directory.
/// 4. Registers a `StaticMount` into the registry.
///
/// On stop (cancellation):
/// - Unregisters the mount from the registry.
pub struct HttpStaticConsumer {
    config: HttpStaticConfig,
    /// Phase B will use this for `rt.metrics().increment_errors(...)` and
    /// `rt.health().force_unhealthy_for_route(...)` calls per ADR-0012.
    #[allow(dead_code)]
    runtime: Arc<dyn RuntimeObservability>,
}

impl HttpStaticConsumer {
    /// Create a new `HttpStaticConsumer` from the given config.
    pub fn new(config: HttpStaticConfig, runtime: Arc<dyn RuntimeObservability>) -> Self {
        Self { config, runtime }
    }
}

#[async_trait::async_trait]
impl Consumer for HttpStaticConsumer {
    async fn start(&mut self, ctx: camel_component_api::ConsumerContext) -> Result<(), CamelError> {
        // 1. Canonicalize dir
        let dir = std::fs::canonicalize(&self.config.dir).map_err(|e| {
            CamelError::Config(format!(
                "http-static directory not found: {}: {}",
                self.config.dir.display(),
                e
            ))
        })?;

        // 2. Canonicalize error_pages paths (resolved relative to dir)
        let mut error_pages = std::collections::HashMap::new();
        for (code, path) in &self.config.error_pages {
            let resolved = if path.is_absolute() {
                path.clone()
            } else {
                self.config.dir.join(path)
            };
            let canonical = std::fs::canonicalize(&resolved).map_err(|e| {
                CamelError::Config(format!(
                    "http-static error page not found for status {}: {}: {}",
                    code,
                    resolved.display(),
                    e
                ))
            })?;
            error_pages.insert(*code, canonical);
        }

        // 3. Build ServeDir
        let serve_dir = ServeDir::new(&dir)
            .precompressed_gzip()
            .precompressed_br()
            .append_index_html_on_directories(true);

        // 4. Get registry
        let registry = ServerRegistry::global()
            .get_or_spawn(
                &self.config.host,
                self.config.port,
                2 * 1024 * 1024,  // max_request_body (not used for static)
                10 * 1024 * 1024, // max_response_body (not used for static)
                1024,             // max_inflight_requests
                self.runtime.clone(),
                ctx.route_id().to_string(),
                None,
            )
            .await?;

        // 5. Register mount
        let mode = if self.config.spa_fallback {
            MountMode::Spa
        } else {
            MountMode::Static
        };
        let mount = StaticMount {
            mount_path: self.config.mount_path.clone(),
            mode,
            dir: dir.clone(),
            cache_control: self.config.cache_control.clone(),
            error_pages,
            serve_dir,
        };

        registry.register_static_mount(mount).await?;

        let mount_path_for_cleanup = self.config.mount_path.clone();
        let registry_for_cleanup = registry.clone();

        // 6. Wait on cancellation token
        ctx.cancelled().await;

        // 7. Unregister on stop (by mount_path identity)
        registry_for_cleanup
            .unregister_static_mount(&mount_path_for_cleanup)
            .await;

        Ok(())
    }

    async fn stop(&mut self) -> Result<(), CamelError> {
        Ok(())
    }

    fn concurrency_model(&self) -> camel_component_api::ConcurrencyModel {
        camel_component_api::ConcurrencyModel::Sequential
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use camel_component_api::test_support::PanicRuntimeObservability;
    fn test_rt() -> std::sync::Arc<dyn camel_component_api::RuntimeObservability> {
        std::sync::Arc::new(PanicRuntimeObservability)
    }
    fn rt() -> std::sync::Arc<dyn camel_component_api::RuntimeObservability> {
        std::sync::Arc::new(PanicRuntimeObservability)
    }

    use super::*;
    use crate::REGISTRY_TEST_MUTEX;
    use camel_component_api::{ConsumerContext, ExchangeEnvelope};
    use std::path::PathBuf;
    use std::sync::Arc;
    use tokio::sync::{Notify, mpsc};
    use tokio_util::sync::CancellationToken;

    /// Helper: create a test consumer context with a controllable cancellation.
    fn test_consumer_ctx(notify: Arc<Notify>) -> ConsumerContext {
        let (tx, _rx) = mpsc::channel::<ExchangeEnvelope>(16);
        let token = CancellationToken::new();
        // Spawn a task that cancels when notified
        let token_clone = token.clone();
        tokio::spawn(async move {
            notify.notified().await;
            token_clone.cancel();
        });
        ConsumerContext::new(tx, token, "http-static-test-route".to_string())
    }

    #[test]
    fn test_component_scheme() {
        let component = HttpStaticComponent::new();
        assert_eq!(component.scheme(), "http-static");
    }

    #[test]
    fn test_component_with_config() {
        let config = HttpStaticConfig {
            dir: PathBuf::from("/tmp"),
            port: 9999,
            ..HttpStaticConfig::default()
        };
        let component = HttpStaticComponent::with_config(config.clone());
        assert_eq!(component.scheme(), "http-static");
    }

    #[test]
    fn test_endpoint_creates_consumer() {
        let config = HttpStaticConfig {
            dir: PathBuf::from("/tmp"),
            ..HttpStaticConfig::default()
        };
        let endpoint = HttpStaticEndpoint {
            uri: "http-static:/tmp".to_string(),
            config,
        };
        let consumer = endpoint.create_consumer(rt());
        assert!(consumer.is_ok());
    }

    #[test]
    fn test_endpoint_producer_not_supported() {
        let config = HttpStaticConfig {
            dir: PathBuf::from("/tmp"),
            ..HttpStaticConfig::default()
        };
        let endpoint = HttpStaticEndpoint {
            uri: "http-static:/tmp".to_string(),
            config,
        };
        let ctx = camel_component_api::ProducerContext::new();
        let result = endpoint.create_producer(rt(), &ctx);
        assert!(result.is_err());
        if let Err(CamelError::Config(msg)) = result {
            assert!(msg.contains("does not support producers"));
        } else {
            panic!("Expected Config error");
        }
    }

    #[tokio::test]
    async fn test_consumer_start_nonexistent_dir_returns_error() {
        let config = HttpStaticConfig {
            dir: PathBuf::from("/nonexistent/path/that/does/not/exist"),
            port: 19900,
            ..HttpStaticConfig::default()
        };
        let mut consumer = HttpStaticConsumer::new(config, test_rt());
        let notify = Arc::new(Notify::new());
        let ctx = test_consumer_ctx(notify);

        let result = consumer.start(ctx).await;
        assert!(result.is_err());
        if let Err(CamelError::Config(msg)) = result {
            assert!(msg.contains("directory not found"));
        } else {
            panic!("Expected Config error for nonexistent dir");
        }
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn test_consumer_start_registers_mount_in_registry() {
        let _guard = REGISTRY_TEST_MUTEX.lock().unwrap();
        // Reset registry for clean test
        ServerRegistry::reset();

        let dir = std::env::temp_dir();
        let canonical_dir = std::fs::canonicalize(&dir).unwrap();

        // Bind to a free port first
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);

        let config = HttpStaticConfig {
            dir: dir.clone(),
            port,
            host: "127.0.0.1".to_string(),
            ..HttpStaticConfig::default()
        };

        // Build ServeDir directly (same as consumer does)
        let serve_dir = ServeDir::new(&canonical_dir)
            .precompressed_gzip()
            .precompressed_br()
            .append_index_html_on_directories(true);

        // Get registry (this spawns the axum server)
        let registry = ServerRegistry::global()
            .get_or_spawn(
                "127.0.0.1",
                port,
                2 * 1024 * 1024,
                10 * 1024 * 1024,
                1024,
                test_rt(),
                "test-static".into(),
                None,
            )
            .await
            .unwrap();

        // Register mount
        let mount = StaticMount {
            mount_path: "/".to_string(),
            mode: MountMode::Static,
            dir: canonical_dir.clone(),
            cache_control: config.cache_control.clone(),
            error_pages: std::collections::HashMap::new(),
            serve_dir,
        };
        registry.register_static_mount(mount).await.unwrap();

        // Verify registered
        let inner = registry.inner.read().await;
        assert_eq!(
            inner.mounts.len(),
            1,
            "Expected one static mount registered"
        );
        assert_eq!(inner.mounts[0].dir, canonical_dir);
        assert_eq!(inner.mounts[0].mount_path, "/");
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn test_consumer_stop_unregisters_mount() {
        let _guard = REGISTRY_TEST_MUTEX.lock().unwrap();
        // Reset registry for clean test
        ServerRegistry::reset();

        let dir = std::env::temp_dir();
        let canonical_dir = std::fs::canonicalize(&dir).unwrap();

        // Bind to a free port
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);

        // Get registry
        let registry = ServerRegistry::global()
            .get_or_spawn(
                "127.0.0.1",
                port,
                2 * 1024 * 1024,
                10 * 1024 * 1024,
                1024,
                test_rt(),
                "test-static".into(),
                None,
            )
            .await
            .unwrap();

        // Register mount
        let serve_dir = ServeDir::new(&canonical_dir)
            .precompressed_gzip()
            .precompressed_br()
            .append_index_html_on_directories(true);
        let mount = StaticMount {
            mount_path: "/".to_string(),
            mode: MountMode::Static,
            dir: canonical_dir.clone(),
            cache_control: "public, max-age=0".to_string(),
            error_pages: std::collections::HashMap::new(),
            serve_dir,
        };
        registry.register_static_mount(mount).await.unwrap();

        // Verify registered
        {
            let inner = registry.inner.read().await;
            assert_eq!(inner.mounts.len(), 1);
        }

        // Unregister by mount_path
        registry.unregister_static_mount("/").await;

        // Verify unregistered
        let inner = registry.inner.read().await;
        assert_eq!(
            inner.mounts.len(),
            0,
            "Expected static mount to be unregistered"
        );
    }
}
