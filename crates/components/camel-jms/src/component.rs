use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use camel_api::{BoxProcessor, CamelError, Exchange};
use camel_component::{
    Component, ConcurrencyModel, Consumer, ConsumerContext, Endpoint, ProducerContext,
};
use camel_bridge::{
    channel::connect_channel,
    download::ensure_binary,
    health::wait_for_health,
    process::{BridgeProcess, BridgeProcessConfig},
};
use tokio::sync::{RwLock, Semaphore};
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
}

pub struct JmsComponent {
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
        Self {
            bridge: Arc::new(RwLock::new(None)),
            config,
            restart_semaphore: Arc::new(Semaphore::new(1)),
        }
    }

    pub async fn ensure_bridge(&self) -> Result<Channel, CamelError> {
        let mut evict_cached = false;
        let cached_channel = {
            let guard = self.bridge.read().await;
            guard.as_ref().map(|handle| handle.channel.clone())
        };

        if let Some(channel) = cached_channel {
            if self.cached_channel_healthy(channel.clone()).await {
                return Ok(channel);
            }
            evict_cached = true;
        }

        let mut guard = self.bridge.write().await;
        if evict_cached
            && let Some(stale) = guard.take()
        {
            let _ = stale.process.stop().await;
        }
        if let Some(handle) = guard.as_ref() {
            return Ok(handle.channel.clone());
        }

        // Acquire semaphore to prevent concurrent bridge starts
        let _permit = self.restart_semaphore.acquire().await.map_err(|e| {
            CamelError::ProcessorError(format!("JMS bridge semaphore error: {e}"))
        })?;

        // Double-check after acquiring semaphore (another caller may have started the bridge)
        if let Some(handle) = guard.as_ref() {
            return Ok(handle.channel.clone());
        }

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

        let process = BridgeProcess::start(&process_config)
            .await
            .map_err(|e| CamelError::ProcessorError(format!("JMS bridge start failed: {e}")))?;

        let port = process.grpc_port();
        let channel = connect_channel(port)
            .await
            .map_err(|e| CamelError::ProcessorError(format!("JMS bridge channel error: {e}")))?;

        let ch = channel.clone();
        wait_for_health(
            &channel,
            Duration::from_millis(self.config.bridge_start_timeout_ms),
            move |_| {
                let ch = ch.clone();
                async move {
                    let mut client = BridgeServiceClient::new(ch);
                    let r = client.health(HealthRequest {}).await?;
                    Ok(r.into_inner().healthy)
                }
            },
        )
        .await
        .map_err(|e| CamelError::ProcessorError(format!("JMS bridge health check failed: {e}")))?;

        info!(port, "JMS bridge ready");
        *guard = Some(BridgeHandle {
            process,
            channel: channel.clone(),
        });
        Ok(channel)
    }

    async fn cached_channel_healthy(&self, channel: Channel) -> bool {
        let timeout = Duration::from_millis(self.config.bridge_start_timeout_ms.min(1_000));
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
        // Acquire semaphore to prevent concurrent restarts
        let _permit = self.restart_semaphore.acquire().await.map_err(|e| {
            CamelError::ProcessorError(format!("JMS bridge restart semaphore error: {e}"))
        })?;

        let mut guard = self.bridge.write().await;
        if let Some(handle) = guard.take() {
            let _ = handle.process.stop().await;
        }
        warn!("JMS bridge restarting...");
        drop(guard);
        self.ensure_bridge().await.map(|_| ())
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

impl LazyJmsConsumer {
    async fn start_with_channel(
        &mut self,
        ctx: ConsumerContext,
        channel: Channel,
    ) -> Result<(), CamelError> {
        let mut inner = JmsConsumer::new(
            channel,
            self.endpoint_config.clone(),
            self.component.config.broker_reconnect_interval_ms,
        );
        inner.start(ctx).await?;
        self.inner = Some(inner);
        Ok(())
    }
}

#[async_trait::async_trait]
impl Consumer for LazyJmsConsumer {
    async fn start(&mut self, ctx: ConsumerContext) -> Result<(), CamelError> {
        let channel = self.component.ensure_bridge().await?;
        self.start_with_channel(ctx, channel).await
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
        "jms"
    }

    fn create_endpoint(&self, uri: &str) -> Result<Box<dyn Endpoint>, CamelError> {
        let endpoint_config = JmsEndpointConfig::from_uri(uri)?;
        let component = Arc::new(JmsComponent {
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
        assert!(err.to_string().contains("expected scheme 'jms'"));
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

        let channel = tonic::transport::Channel::from_static("http://127.0.0.1:1").connect_lazy();
        let (tx, _rx) = mpsc::channel(8);
        let cancel = CancellationToken::new();
        let ctx = ConsumerContext::new(tx, cancel);

        consumer.start_with_channel(ctx, channel).await.unwrap();
        assert!(consumer.stop().await.is_ok());
        assert!(consumer.stop().await.is_ok());
    }
}
