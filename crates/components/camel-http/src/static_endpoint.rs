use std::collections::HashMap;
use std::path::PathBuf;

use async_trait::async_trait;
use camel_component_api::{
    BoxProcessor, CamelError, Component, ComponentContext, ConcurrencyModel, Consumer,
    ConsumerContext, Endpoint, ProducerContext, UriConfig,
};
use tower_http::services::ServeDir;

use crate::ServerRegistry;
use crate::registry::StaticMount;
use crate::static_config::HttpStaticConfig;

// ---------------------------------------------------------------------------
// HttpStaticComponent
// ---------------------------------------------------------------------------

/// Component factory for `http-static:` scheme.
pub struct HttpStaticComponent {
    config: Option<HttpStaticConfig>,
}

impl HttpStaticComponent {
    pub fn new() -> Self {
        Self { config: None }
    }

    pub fn with_config(config: HttpStaticConfig) -> Self {
        Self {
            config: Some(config),
        }
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
        _ctx: &dyn ComponentContext,
    ) -> Result<Box<dyn Endpoint>, CamelError> {
        // Parse URI into config
        let uri_config = HttpStaticConfig::from_uri(uri)?;

        // Merge with TOML-provided defaults (if any)
        let config = if let Some(ref defaults) = self.config {
            uri_config.with_defaults(defaults)
        } else {
            uri_config
        };

        // Validate (clone since validate takes ownership)
        config.clone().validate()?;

        Ok(Box::new(HttpStaticEndpoint {
            uri: uri.to_string(),
            config,
        }))
    }
}

// ---------------------------------------------------------------------------
// HttpStaticEndpoint
// ---------------------------------------------------------------------------

pub struct HttpStaticEndpoint {
    uri: String,
    config: HttpStaticConfig,
}

impl Endpoint for HttpStaticEndpoint {
    fn uri(&self) -> &str {
        &self.uri
    }

    fn create_consumer(&self) -> Result<Box<dyn Consumer>, CamelError> {
        Ok(Box::new(HttpStaticConsumer {
            config: self.config.clone(),
        }))
    }

    fn create_producer(&self, _ctx: &ProducerContext) -> Result<BoxProcessor, CamelError> {
        Err(CamelError::EndpointCreationFailed(
            "http-static does not support producers".into(),
        ))
    }
}

// ---------------------------------------------------------------------------
// HttpStaticConsumer
// ---------------------------------------------------------------------------

pub struct HttpStaticConsumer {
    config: HttpStaticConfig,
}

#[async_trait]
impl Consumer for HttpStaticConsumer {
    async fn start(&mut self, ctx: ConsumerContext) -> Result<(), CamelError> {
        // Canonicalize dir
        let dir = std::fs::canonicalize(&self.config.dir).map_err(|e| {
            CamelError::Config(format!(
                "http-static: cannot canonicalize dir '{}': {e}",
                self.config.dir.display()
            ))
        })?;

        // Canonicalize error_pages paths
        let mut error_pages: HashMap<u16, PathBuf> = HashMap::new();
        for (code, path) in &self.config.error_pages {
            let canonical = std::fs::canonicalize(path).map_err(|e| {
                CamelError::Config(format!(
                    "http-static: cannot canonicalize error page '{:?}': {e}",
                    path
                ))
            })?;
            error_pages.insert(*code, canonical);
        }

        // Build ServeDir
        let serve_dir = ServeDir::new(&dir)
            .precompressed_gzip()
            .precompressed_br()
            .append_index_html_on_directories(true);

        // Get registry
        let registry = ServerRegistry::global()
            .get_or_spawn(
                &self.config.host,
                self.config.port,
                2 * 1024 * 1024,  // default max_request_body
                10 * 1024 * 1024, // default max_response_body
                100,              // default max_inflight
            )
            .await?;

        // Register mount
        let mount = StaticMount {
            dir: dir.clone(),
            cache_control: self.config.cache_control.clone(),
            error_pages: error_pages.clone(),
            serve_dir,
        };

        if self.config.spa_fallback {
            // First-wins: check if spa_mount already exists
            {
                let inner = registry.inner.read().await;
                if inner.spa_mount.is_some() {
                    return Err(CamelError::Config(
                        "http-static: spaFallback already registered on this port".to_string(),
                    ));
                }
            }
            let mut inner = registry.inner.write().await;
            // Double-check after acquiring write lock
            if inner.spa_mount.is_some() {
                return Err(CamelError::Config(
                    "http-static: spaFallback already registered on this port".to_string(),
                ));
            }
            inner.spa_mount = Some(mount);
        } else {
            // Check for overlapping dirs
            {
                let inner = registry.inner.read().await;
                for existing in &inner.static_mounts {
                    if existing.dir == dir {
                        return Err(CamelError::Config(format!(
                            "http-static: directory already mounted: {}",
                            dir.display()
                        )));
                    }
                }
            }
            let mut inner = registry.inner.write().await;
            // Double-check
            for existing in &inner.static_mounts {
                if existing.dir == dir {
                    return Err(CamelError::Config(format!(
                        "http-static: directory already mounted: {}",
                        dir.display()
                    )));
                }
            }
            inner.static_mounts.push(mount);
        }

        // Wait for cancellation
        ctx.cancelled().await;

        // Cleanup on shutdown
        if self.config.spa_fallback {
            let mut inner = registry.inner.write().await;
            if inner.spa_mount.as_ref().is_some_and(|m| m.dir == dir) {
                inner.spa_mount = None;
            }
        } else {
            let mut inner = registry.inner.write().await;
            inner.static_mounts.retain(|m| m.dir != dir);
        }

        Ok(())
    }

    async fn stop(&mut self) -> Result<(), CamelError> {
        Ok(())
    }

    fn concurrency_model(&self) -> ConcurrencyModel {
        ConcurrencyModel::Sequential
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_component_api::NoOpComponentContext;
    use std::fs;

    fn make_temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "http-static-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn test_static_component_scheme() {
        let component = HttpStaticComponent::new();
        assert_eq!(component.scheme(), "http-static");
    }

    #[test]
    fn test_static_endpoint_creation() {
        let dir = make_temp_dir();
        let uri = format!("http-static:{}?port=19200", dir.display());
        let component = HttpStaticComponent::new();
        let endpoint = component
            .create_endpoint(&uri, &NoOpComponentContext)
            .unwrap();
        assert!(endpoint.create_consumer().is_ok());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_static_consumer_rejects_nonexistent_dir() {
        let component = HttpStaticComponent::new();
        let result = component.create_endpoint(
            "http-static:/nonexistent/path?port=19201",
            &NoOpComponentContext,
        );
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_spa_conflict_rejected() {
        use crate::ServerRegistry;
        use camel_component_api::ConsumerContext;

        // Reset registry for clean test
        ServerRegistry::reset();

        let dir = make_temp_dir();
        let uri1 = format!("http-static:{}?port=19202&spaFallback=true", dir.display());
        let uri2 = format!("http-static:{}?port=19202&spaFallback=true", dir.display());

        let component = HttpStaticComponent::new();
        let endpoint1 = component
            .create_endpoint(&uri1, &NoOpComponentContext)
            .unwrap();
        let mut consumer1 = endpoint1.create_consumer().unwrap();

        // Start first consumer in background
        let (tx1, _rx1) = tokio::sync::mpsc::channel::<camel_component_api::ExchangeEnvelope>(16);
        let token1 = tokio_util::sync::CancellationToken::new();
        let ctx1 = ConsumerContext::new(tx1, token1.clone());
        let handle1 = tokio::spawn(async move { consumer1.start(ctx1).await });

        // Give it time to register
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        let endpoint2 = component
            .create_endpoint(&uri2, &NoOpComponentContext)
            .unwrap();
        let mut consumer2 = endpoint2.create_consumer().unwrap();
        let (tx2, _rx2) = tokio::sync::mpsc::channel::<camel_component_api::ExchangeEnvelope>(16);
        let token2 = tokio_util::sync::CancellationToken::new();
        let ctx2 = ConsumerContext::new(tx2, token2.clone());
        let result = consumer2.start(ctx2).await;

        // Second should fail with config error
        assert!(
            result.is_err(),
            "Second SPA mount on same port should be rejected"
        );

        // Cleanup
        let _ = fs::remove_dir_all(&dir);
        ServerRegistry::reset();
        // Cancel first consumer
        token1.cancel();
        let _ = handle1.await;
    }
}
