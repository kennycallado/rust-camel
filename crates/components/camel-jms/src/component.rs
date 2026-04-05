use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use camel_api::{BoxProcessor, CamelError, Exchange};
use camel_bridge::{
    channel::connect_channel,
    download::ensure_binary,
    health::wait_for_health,
    process::{BridgeProcess, BridgeProcessConfig},
};
use camel_component::{
    Component, ConcurrencyModel, Consumer, ConsumerContext, Endpoint, ProducerContext,
};
use tokio::sync::{OwnedSemaphorePermit, RwLock, Semaphore};
use tonic::transport::Channel;
use tower::Service;
use tracing::{info, warn};

use crate::config::{JmsConfig, JmsEndpointConfig};
use crate::consumer::JmsConsumer;
use crate::producer::JmsProducer;
use crate::proto::{HealthRequest, bridge_service_client::BridgeServiceClient};

pub struct BridgeHandle {
    pub process: BridgeProcess,
    pub channel: Channel,
    /// The broker URL used to start this bridge process (for conflict detection).
    pub broker_url: String,
}

/// Cloning a `JmsComponent` shares the same underlying bridge process and
/// connection pool. This allows multiple Camel contexts (e.g. in parallel
/// integration tests) to reuse a single bridge instance per broker, avoiding
/// resource exhaustion from spawning one native process per test.
#[derive(Clone)]
pub struct JmsComponent {
    scheme: String,
    bridge: Arc<RwLock<Option<BridgeHandle>>>,
    config: JmsConfig,
    restart_semaphore: Arc<Semaphore>,
}

impl Default for JmsComponent {
    fn default() -> Self {
        Self::new(JmsConfig::default())
    }
}

/// Check if an error indicates a bridge transport failure that warrants a restart.
fn is_bridge_transport_error(err: &CamelError) -> bool {
    let msg = err.to_string();
    msg.contains("JMS gRPC send error") || msg.contains("JMS gRPC subscribe error")
}

impl JmsComponent {
    pub fn new(config: JmsConfig) -> Self {
        Self::with_scheme(
            "jms",
            config,
            Arc::new(RwLock::new(None)),
            Arc::new(Semaphore::new(1)),
        )
    }

    pub fn with_scheme(
        scheme: impl Into<String>,
        config: JmsConfig,
        bridge: Arc<RwLock<Option<BridgeHandle>>>,
        restart_semaphore: Arc<Semaphore>,
    ) -> Self {
        Self {
            scheme: scheme.into(),
            config,
            bridge,
            restart_semaphore,
        }
    }

    pub async fn ensure_bridge(&self) -> Result<Channel, CamelError> {
        let cached_channel = {
            let guard = self.bridge.read().await;
            guard.as_ref().map(|handle| handle.channel.clone())
        };

        if let Some(channel) = cached_channel {
            if self.cached_channel_healthy(channel.clone()).await {
                return Ok(channel);
            }
        }

        // Acquire semaphore before write-lock to keep canonical lock ordering.
        let permit = self
            .restart_semaphore
            .clone()
            .acquire_owned()
            .await
            .map_err(|e| CamelError::ProcessorError(format!("JMS bridge semaphore error: {e}")))?;

        let latest_channel = {
            let guard = self.bridge.read().await;
            guard.as_ref().map(|handle| handle.channel.clone())
        };
        if let Some(channel) = latest_channel
            && self.cached_channel_healthy(channel.clone()).await
        {
            return Ok(channel);
        }

        let stale = {
            let mut guard = self.bridge.write().await;
            guard.take()
        };
        if let Some(stale) = stale {
            let _ = stale.process.stop().await;
        }

        let handle = self.start_bridge_inner(&permit).await?;
        let channel = handle.channel.clone();
        let mut guard = self.bridge.write().await;
        *guard = Some(handle);
        Ok(channel)
    }

    async fn start_bridge_inner(
        &self,
        _permit: &OwnedSemaphorePermit,
    ) -> Result<BridgeHandle, CamelError> {
        info!("Starting JMS bridge process...");
        let binary_path = ensure_binary(&self.config.bridge_version, &self.config.bridge_cache_dir)
            .await
            .map_err(|e| {
                CamelError::ProcessorError(format!("JMS bridge binary unavailable: {e}"))
            })?;

        let process_config = BridgeProcessConfig {
            binary_path,
            broker_url: self.config.broker_url.clone(),
            broker_type: self.config.broker_type.clone(),
            username: self.config.username.clone(),
            password: self.config.password.clone(),
            start_timeout_ms: self.config.bridge_start_timeout_ms,
        };

        let total_timeout = Duration::from_millis(self.config.bridge_start_timeout_ms);
        let startup_started_at = Instant::now();

        let process = BridgeProcess::start(&process_config)
            .await
            .map_err(|e| CamelError::ProcessorError(format!("JMS bridge start failed: {e}")))?;

        let elapsed = startup_started_at.elapsed();
        if elapsed >= total_timeout {
            return Err(CamelError::ProcessorError(format!(
                "JMS bridge startup exceeded timeout budget before health check (elapsed: {:?}, budget: {:?})",
                elapsed, total_timeout
            )));
        }
        let remaining_timeout = total_timeout - elapsed;

        let port = process.grpc_port();
        let channel = connect_channel(port)
            .await
            .map_err(|e| CamelError::ProcessorError(format!("JMS bridge channel error: {e}")))?;

        let ch = channel.clone();
        wait_for_health(&channel, remaining_timeout, move |_| {
            let ch = ch.clone();
            async move {
                let mut client = BridgeServiceClient::new(ch);
                let r = client.health(HealthRequest {}).await?;
                let resp = r.into_inner();
                if !resp.healthy {
                    tracing::debug!(
                        "bridge health check: not ready — broker message: {}",
                        resp.message
                    );
                }
                Ok(resp.healthy)
            }
        })
        .await
        .map_err(|e| CamelError::ProcessorError(format!("JMS bridge health check failed: {e}")))?;

        info!(port, "JMS bridge ready");
        Ok(BridgeHandle {
            process,
            channel,
            broker_url: self.config.broker_url.clone(),
        })
    }

    async fn cached_channel_healthy(&self, channel: Channel) -> bool {
        let timeout = Duration::from_millis(self.config.bridge_start_timeout_ms.min(5_000));
        match tokio::time::timeout(timeout, async move {
            let mut client = BridgeServiceClient::new(channel);
            client
                .health(HealthRequest {})
                .await
                .map(|r| r.into_inner().healthy)
        })
        .await
        {
            Ok(Ok(true)) => true,
            Ok(Ok(false)) | Ok(Err(_)) | Err(_) => {
                warn!("Cached JMS bridge channel is unhealthy, restarting bridge");
                false
            }
        }
    }

    pub async fn restart_bridge(&self) -> Result<(), CamelError> {
        // Acquire semaphore before write-lock to keep canonical lock ordering.
        let permit = self
            .restart_semaphore
            .clone()
            .acquire_owned()
            .await
            .map_err(|e| {
                CamelError::ProcessorError(format!("JMS bridge restart semaphore error: {e}"))
            })?;

        let stale = {
            let mut guard = self.bridge.write().await;
            guard.take()
        };
        if let Some(handle) = stale {
            let _ = handle.process.stop().await;
        }
        warn!("JMS bridge restarting...");

        let handle = self.start_bridge_inner(&permit).await?;
        let mut guard = self.bridge.write().await;
        *guard = Some(handle);
        Ok(())
    }

    /// Test-only helper: send directly to bridge without a Camel route.
    #[cfg(test)]
    pub async fn send_for_test(
        &self,
        destination: &str,
        body: &[u8],
        content_type: &str,
    ) -> Result<String, CamelError> {
        let channel = self.ensure_bridge().await?;
        let mut client = BridgeServiceClient::new(channel);
        let r = client
            .send(crate::proto::SendRequest {
                destination: destination.to_string(),
                body: body.to_vec(),
                headers: Default::default(),
                content_type: content_type.to_string(),
            })
            .await
            .map_err(|e| CamelError::ProcessorError(format!("test send error: {e}")))?;
        Ok(r.into_inner().message_id)
    }
}

struct JmsEndpointInner {
    component: Arc<JmsComponent>,
    uri: String,
    endpoint_config: JmsEndpointConfig,
}

#[derive(Clone)]
struct LazyJmsProducer {
    component: Arc<JmsComponent>,
    endpoint_config: JmsEndpointConfig,
}

impl Service<Exchange> for LazyJmsProducer {
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, exchange: Exchange) -> Self::Future {
        let component = self.component.clone();
        let endpoint_config = self.endpoint_config.clone();
        Box::pin(async move {
            let channel = component.ensure_bridge().await?;
            let mut producer = JmsProducer::new(channel, endpoint_config);
            match producer.call(exchange).await {
                Ok(exchange) => Ok(exchange),
                Err(err) => {
                    // On transport error the bridge is restarted so the next caller gets a fresh
                    // channel, but this call is not retried — the error propagates to the caller.
                    // The Camel route's error handler / redelivery policy is responsible for retries.
                    if is_bridge_transport_error(&err) {
                        component.restart_bridge().await?;
                    }
                    Err(err)
                }
            }
        })
    }
}

impl Endpoint for JmsEndpointInner {
    fn uri(&self) -> &str {
        &self.uri
    }

    fn create_producer(&self, _ctx: &ProducerContext) -> Result<BoxProcessor, CamelError> {
        Ok(BoxProcessor::new(LazyJmsProducer {
            component: self.component.clone(),
            endpoint_config: self.endpoint_config.clone(),
        }))
    }

    fn create_consumer(&self) -> Result<Box<dyn Consumer>, CamelError> {
        Ok(Box::new(LazyJmsConsumer {
            component: self.component.clone(),
            endpoint_config: self.endpoint_config.clone(),
            inner: None,
        }))
    }
}

struct LazyJmsConsumer {
    component: Arc<JmsComponent>,
    endpoint_config: JmsEndpointConfig,
    inner: Option<JmsConsumer>,
}

#[async_trait::async_trait]
impl Consumer for LazyJmsConsumer {
    async fn start(&mut self, ctx: ConsumerContext) -> Result<(), CamelError> {
        let mut inner = JmsConsumer::new(
            self.component.clone(),
            self.endpoint_config.clone(),
            self.component.config.broker_reconnect_interval_ms,
        );
        inner.start(ctx).await?;
        self.inner = Some(inner);
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), CamelError> {
        if let Some(mut inner) = self.inner.take() {
            inner.stop().await?;
        }
        Ok(())
    }

    fn concurrency_model(&self) -> ConcurrencyModel {
        ConcurrencyModel::Sequential
    }
}

impl Component for JmsComponent {
    fn scheme(&self) -> &str {
        &self.scheme
    }

    fn create_endpoint(&self, uri: &str) -> Result<Box<dyn Endpoint>, CamelError> {
        let endpoint_config = JmsEndpointConfig::from_uri(uri)?;
        let component = Arc::new(JmsComponent {
            scheme: self.scheme.clone(),
            bridge: Arc::clone(&self.bridge),
            config: self.config.clone(),
            restart_semaphore: Arc::clone(&self.restart_semaphore),
        });
        Ok(Box::new(JmsEndpointInner {
            component,
            uri: uri.to_string(),
            endpoint_config,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_component::ConsumerContext;
    use tokio::sync::mpsc;
    use tokio_util::sync::CancellationToken;

    #[test]
    fn with_scheme_returns_correct_scheme() {
        use std::sync::Arc;
        use tokio::sync::{RwLock, Semaphore};
        let bridge = Arc::new(RwLock::new(None));
        let semaphore = Arc::new(Semaphore::new(1));
        let comp = JmsComponent::with_scheme(
            "activemq",
            JmsConfig::default(),
            bridge,
            semaphore,
        );
        assert_eq!(comp.scheme(), "activemq");
    }

    #[test]
    fn new_constructor_scheme_is_jms() {
        let comp = JmsComponent::new(JmsConfig::default());
        assert_eq!(comp.scheme(), "jms");
    }

    #[test]
    fn component_scheme_is_jms() {
        let component = JmsComponent::new(JmsConfig::default());
        assert_eq!(component.scheme(), "jms");
    }

    #[test]
    fn create_endpoint_rejects_wrong_scheme() {
        let component = JmsComponent::new(JmsConfig::default());
        let err = match component.create_endpoint("kafka:orders") {
            Ok(_) => panic!("endpoint creation should fail for wrong scheme"),
            Err(err) => err,
        };
        assert!(err.to_string().contains("expected scheme 'jms', 'activemq', or 'artemis'"));
    }

    #[tokio::test]
    async fn lazy_consumer_stop_after_start_is_idempotent() {
        let component = Arc::new(JmsComponent::new(JmsConfig::default()));
        let endpoint_config = JmsEndpointConfig::from_uri("jms:queue:test").unwrap();
        let mut consumer = LazyJmsConsumer {
            component,
            endpoint_config,
            inner: None,
        };

        let (tx, _rx) = mpsc::channel(8);
        let cancel = CancellationToken::new();
        let ctx = ConsumerContext::new(tx, cancel);

        // start() spawns a background task that will try ensure_bridge internally;
        // since there is no real bridge, the task will keep retrying but we can
        // still verify stop() is idempotent.
        consumer.start(ctx).await.unwrap();
        assert!(consumer.stop().await.is_ok());
        assert!(consumer.stop().await.is_ok());
    }

    #[tokio::test]
    async fn concurrent_ensure_and_restart_do_not_deadlock() {
        use tokio::time::{Duration, timeout};

        struct EnvGuard {
            key: &'static str,
            prev: Option<std::ffi::OsString>,
        }

        impl Drop for EnvGuard {
            fn drop(&mut self) {
                if let Some(v) = &self.prev {
                    // SAFETY: test code restores process env to previous value.
                    unsafe { std::env::set_var(self.key, v) };
                } else {
                    // SAFETY: test code restores process env by removing key.
                    unsafe { std::env::remove_var(self.key) };
                }
            }
        }

        let env_key = "CAMEL_JMS_BRIDGE_BINARY_PATH";
        let env_guard = EnvGuard {
            key: env_key,
            prev: std::env::var_os(env_key),
        };
        // Use a fast-failing executable to avoid network/download latency in this test.
        // SAFETY: test-scoped env mutation, restored by EnvGuard.
        unsafe { std::env::set_var(env_key, "/bin/false") };

        // Component without a real bridge process: calls are expected to fail,
        // but they should still complete without deadlocking.
        let component = Arc::new(JmsComponent::new(JmsConfig::default()));

        let c1 = component.clone();
        let c2 = component.clone();
        let c3 = component.clone();

        let result = timeout(Duration::from_secs(5), async {
            let t1 = tokio::spawn(async move {
                let _ = c1.ensure_bridge().await;
            });
            let t2 = tokio::spawn(async move {
                let _ = c2.ensure_bridge().await;
            });
            let t3 = tokio::spawn(async move {
                let _ = c3.restart_bridge().await;
            });
            let _ = tokio::join!(t1, t2, t3);
        })
        .await;

        assert!(
            result.is_ok(),
            "concurrent ensure_bridge/restart_bridge deadlocked (timeout after 5s)"
        );

        drop(env_guard);
    }
}
