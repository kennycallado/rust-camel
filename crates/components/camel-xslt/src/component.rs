use crate::client::{BridgeState, XsltBridgeClient};
use crate::config::{XsltComponentConfig, XsltEndpointConfig};
use crate::endpoint::XsltEndpoint;
use crate::error::XsltError;
use camel_bridge::{
    channel::connect_channel,
    download::ensure_binary_for_spec,
    process::{BridgeProcess, BridgeProcessConfig},
    reconnect::BridgeReconnectHandler,
    spec::XML_BRIDGE,
};
use camel_component_api::{CamelError, Component, ComponentContext, Endpoint};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{Mutex, watch};

pub struct XsltBridgeRuntime {
    config: XsltComponentConfig,
    process: Arc<Mutex<Option<BridgeProcess>>>,
    state_tx: watch::Sender<BridgeState>,
    state_rx: Arc<watch::Receiver<BridgeState>>,
    start_lock: Arc<Mutex<()>>,
}

impl XsltBridgeRuntime {
    pub(crate) fn new(
        config: XsltComponentConfig,
        process: Arc<Mutex<Option<BridgeProcess>>>,
        state_tx: watch::Sender<BridgeState>,
        state_rx: Arc<watch::Receiver<BridgeState>>,
    ) -> Self {
        Self {
            config,
            process,
            state_tx,
            state_rx,
            start_lock: Arc::new(Mutex::new(())),
        }
    }

    /// Trigger bridge start from `poll_ready`.
    ///
    /// Unlike `ensure_bridge_started`, on failure this transitions the state
    /// to `Degraded` so that `poll_ready` can surface the error on the next
    /// poll instead of looping forever in `Starting`.
    pub(crate) async fn ensure_started_or_degrade(
        &self,
        reconnect_handler: &dyn BridgeReconnectHandler,
    ) {
        if matches!(
            &*self.state_rx.borrow(),
            BridgeState::Ready { .. } | BridgeState::Stopped | BridgeState::Degraded(_)
        ) {
            return;
        }
        if let Err(e) = self.ensure_bridge_started(reconnect_handler).await {
            let _ = self.state_tx.send(BridgeState::Degraded(e.to_string()));
        }
    }

    pub(crate) async fn ensure_bridge_started(
        &self,
        reconnect_handler: &dyn BridgeReconnectHandler,
    ) -> Result<(), XsltError> {
        if matches!(&*self.state_rx.borrow(), BridgeState::Ready { .. }) {
            return Ok(());
        }

        let _guard = self.start_lock.lock().await;
        if matches!(&*self.state_rx.borrow(), BridgeState::Ready { .. }) {
            return Ok(());
        }

        let (process, channel, port) = self.start_bridge_process().await?;
        {
            let mut process_guard = self.process.lock().await;
            *process_guard = Some(process);
        }
        let _ = self.state_tx.send(BridgeState::Ready {
            channel: channel.clone(),
        });
        reconnect_handler
            .on_reconnect(port)
            .map_err(|e| XsltError::Bridge(format!("xml-bridge reconnect handler failed: {e}")))?;

        Ok(())
    }

    async fn restart_bridge(
        &self,
        reconnect_handler: &dyn BridgeReconnectHandler,
    ) -> Result<(), XsltError> {
        let _guard = self.start_lock.lock().await;
        let _ = self.state_tx.send(BridgeState::Restarting { attempt: 0 });

        let old_process = {
            let mut process_guard = self.process.lock().await;
            process_guard.take()
        };
        if let Some(process) = old_process {
            let _ = process.stop().await;
        }

        let (process, channel, port) = self.start_bridge_process().await?;
        {
            let mut process_guard = self.process.lock().await;
            *process_guard = Some(process);
        }
        let _ = self.state_tx.send(BridgeState::Ready {
            channel: channel.clone(),
        });
        reconnect_handler
            .on_reconnect(port)
            .map_err(|e| XsltError::Bridge(format!("xml-bridge reconnect handler failed: {e}")))?;

        Ok(())
    }

    pub(crate) async fn transform_with_retry(
        &self,
        client: &XsltBridgeClient,
        stylesheet_id: &str,
        document: Vec<u8>,
        params: Vec<(String, String)>,
        output_method: Option<String>,
    ) -> Result<Vec<u8>, XsltError> {
        self.ensure_bridge_started(client).await?;
        let mut attempt: u32 = 0;
        // Manual retry loop (not retry_async / retry_async_cancelable)
        // because between attempts self.restart_bridge(client).await must
        // tear down and recreate the gRPC channel. This is an async
        // lifecycle side effect, not just mutable state. The HRTB variant
        // (bd rc-cvq) would only solve &mut borrow re-entrancy, not async
        // side-effect orchestration. See camel-redis consumer.rs for a
        // similar polling-loop justification.
        loop {
            attempt += 1;
            match client
                .transform(
                    &stylesheet_id.to_string(),
                    document.clone(),
                    params.clone(),
                    output_method.clone(),
                )
                .await
            {
                Ok(result) => return Ok(result),
                Err(err)
                    if Self::is_transport_error(&err)
                        && self.config.reconnect.should_retry(attempt) =>
                {
                    let delay = self.config.reconnect.delay_for(attempt - 1);
                    tracing::warn!(
                        attempt,
                        delay_ms = delay.as_millis(),
                        error = %err,
                        "XSLT transform transport error, retrying"
                    );
                    self.restart_bridge(client).await?;
                    tokio::time::sleep(delay).await;
                }
                Err(err) => return Err(err),
            }
        }
    }

    fn is_transport_error(err: &XsltError) -> bool {
        matches!(err, XsltError::BridgeTransport { .. })
    }

    pub(crate) fn state_rx(&self) -> &Arc<watch::Receiver<BridgeState>> {
        &self.state_rx
    }

    pub async fn shutdown(&self) {
        let mut guard = self.process.lock().await;
        if let Some(p) = guard.take()
            && let Err(e) = p.stop().await
        {
            tracing::warn!("Failed to stop XSLT bridge process: {}", e);
        }
    }

    async fn start_bridge_process(
        &self,
    ) -> Result<(BridgeProcess, tonic::transport::Channel, u16), XsltError> {
        let binary_path = match &self.config.bridge_binary_path {
            Some(path) => path.clone(),
            None => ensure_binary_for_spec(
                &XML_BRIDGE,
                &self.config.bridge_version,
                &self.config.bridge_cache_dir,
            )
            .await
            .map_err(|e| XsltError::Bridge(format!("failed to resolve xml-bridge binary: {e}")))?,
        };

        let process = BridgeProcess::start(&BridgeProcessConfig::xml(
            binary_path,
            self.config.bridge_start_timeout_ms,
        ))
        .await
        .map_err(|e| XsltError::Bridge(format!("failed to start xml-bridge process: {e}")))?;
        let port = process.grpc_port();

        let channel = connect_channel(port).await.map_err(|e| {
            XsltError::Bridge(format!(
                "failed to connect to xml-bridge on port {port}: {e}"
            ))
        })?;

        Ok((process, channel, port))
    }
}

pub struct XsltComponent {
    runtime: Arc<XsltBridgeRuntime>,
    client: Arc<XsltBridgeClient>,
}

impl Default for XsltComponent {
    fn default() -> Self {
        Self::new(XsltComponentConfig::default())
    }
}

impl XsltComponent {
    pub fn new(config: XsltComponentConfig) -> Self {
        let (state_tx, state_rx) = watch::channel(BridgeState::Starting);
        let state_rx = Arc::new(state_rx);
        let client = Arc::new(XsltBridgeClient::new(Arc::clone(&state_rx)));
        let runtime = Arc::new(XsltBridgeRuntime::new(
            config,
            Arc::new(Mutex::new(None)),
            state_tx,
            Arc::clone(&state_rx),
        ));

        Self { runtime, client }
    }

    #[allow(dead_code)]
    pub fn with_client_for_testing(
        config: XsltComponentConfig,
        state_tx: watch::Sender<BridgeState>,
        state_rx: watch::Receiver<BridgeState>,
        client: Arc<XsltBridgeClient>,
    ) -> Self {
        let state_rx = Arc::new(state_rx);
        let runtime = Arc::new(XsltBridgeRuntime::new(
            config,
            Arc::new(Mutex::new(None)),
            state_tx,
            state_rx,
        ));

        Self { runtime, client }
    }

    pub fn bridge_runtime(&self) -> Arc<XsltBridgeRuntime> {
        Arc::clone(&self.runtime)
    }

    fn read_stylesheet(&self, stylesheet_uri: &str) -> Result<Vec<u8>, XsltError> {
        let path = PathBuf::from(stylesheet_uri);

        Ok(std::fs::read(path)?)
    }
}

impl Component for XsltComponent {
    fn scheme(&self) -> &str {
        "xslt"
    }

    fn create_endpoint(
        &self,
        uri: &str,
        ctx: &dyn ComponentContext,
    ) -> Result<Box<dyn Endpoint>, CamelError> {
        let endpoint_config = XsltEndpointConfig::from_uri(uri)?;
        let stylesheet_bytes = self
            .read_stylesheet(&endpoint_config.stylesheet_uri)
            .map_err(|e| CamelError::EndpointCreationFailed(e.to_string()))?;

        let health_check = crate::health::XsltHealthCheck::new(Arc::clone(self.runtime.state_rx()));
        ctx.register_current_route_health_check(Arc::new(health_check));

        Ok(Box::new(XsltEndpoint::new(
            uri.to_string(),
            stylesheet_bytes,
            endpoint_config.params,
            endpoint_config.output_method,
            endpoint_config.fail_on_null_body,
            Arc::clone(&self.client),
            Arc::clone(&self.runtime),
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_component_api::NetworkRetryPolicy;
    use std::time::Duration;

    /// Regression: max_attempts=N → exactly N invocations (caught OpenSearch off-by-one 1f5c4c2a).
    /// Replicates the exact retry loop from `transform_with_retry` (component.rs:129-157):
    ///   attempt starts at 0, incremented at top, should_retry(attempt), delay_for(attempt-1)
    #[tokio::test]
    async fn retry_loop_invokes_operation_exactly_max_attempts_times() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicU32, Ordering};

        let policy = NetworkRetryPolicy {
            max_attempts: 3,
            initial_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(1),
            multiplier: 1.0,
            ..NetworkRetryPolicy::default()
        };

        let calls = Arc::new(AtomicU32::new(0));
        let calls_clone = Arc::clone(&calls);

        let mut attempt: u32 = 0;
        loop {
            attempt += 1;
            calls_clone.fetch_add(1, Ordering::SeqCst);
            let op_result: Result<(), ()> = Err(());
            match op_result {
                Ok(_) => unreachable!(),
                Err(_) if policy.should_retry(attempt) => {
                    let delay = policy.delay_for(attempt - 1);
                    tokio::time::sleep(delay).await;
                    continue;
                }
                Err(_) => break,
            }
        }

        assert_eq!(
            calls.load(Ordering::SeqCst),
            3,
            "max_attempts=3 must yield exactly 3 invocations"
        );
    }
}
