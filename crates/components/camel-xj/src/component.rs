use crate::config::{Direction, XjEndpointConfig};
use crate::endpoint::XjEndpoint;
use crate::error::XjError;
use crate::identity::{JSON_TO_XML_XSLT, XML_TO_JSON_XSLT};
use camel_bridge::{
    download::ensure_binary_for_spec,
    process::{BridgeProcess, BridgeProcessConfig},
    reconnect::BridgeReconnectHandler,
    spec::XML_BRIDGE,
};
use camel_component_api::NetworkRetryPolicy;
use camel_component_api::{CamelError, Component, ComponentContext, Endpoint};
use camel_xslt::{BridgeState, XsltBridgeClient};
use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, watch};
use tonic::Code;

#[derive(Debug, Clone)]
pub struct XjComponentConfig {
    pub bridge_binary_path: Option<PathBuf>,
    pub bridge_start_timeout_ms: u64,
    pub bridge_version: String,
    pub bridge_cache_dir: PathBuf,
}

impl Default for XjComponentConfig {
    fn default() -> Self {
        Self {
            bridge_binary_path: None,
            bridge_start_timeout_ms: 30_000,
            bridge_version: camel_xslt::BRIDGE_VERSION.to_string(),
            bridge_cache_dir: camel_bridge::download::default_cache_dir_for_spec(&XML_BRIDGE),
        }
    }
}

pub struct XjBridgeRuntime {
    config: XjComponentConfig,
    process: Arc<Mutex<Option<BridgeProcess>>>,
    state_tx: watch::Sender<BridgeState>,
    state_rx: Arc<watch::Receiver<BridgeState>>,
    start_lock: Arc<Mutex<()>>,
}

impl XjBridgeRuntime {
    pub(crate) fn new(
        config: XjComponentConfig,
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

    async fn ensure_bridge_started(
        &self,
        reconnect_handler: &dyn BridgeReconnectHandler,
    ) -> Result<(), XjError> {
        if matches!(&*self.state_rx.borrow(), BridgeState::Ready { .. }) {
            return Ok(());
        }

        let _guard = self.start_lock.lock().await;
        if matches!(&*self.state_rx.borrow(), BridgeState::Ready { .. }) {
            return Ok(());
        }

        let (process, channel) = self.start_bridge_process().await?;
        {
            let mut process_guard = self.process.lock().await;
            *process_guard = Some(process);
        }
        let _ = self.state_tx.send(BridgeState::Ready {
            channel: channel.clone(),
        });
        reconnect_handler.on_reconnect(&channel).map_err(|e| {
            XjError::Xslt(camel_xslt::XsltError::Bridge(format!(
                "xml-bridge reconnect handler failed: {e}"
            )))
        })?;

        Ok(())
    }

    async fn restart_bridge(
        &self,
        reconnect_handler: &dyn BridgeReconnectHandler,
    ) -> Result<(), XjError> {
        let _guard = self.start_lock.lock().await;
        let _ = self.state_tx.send(BridgeState::Restarting { attempt: 0 });

        let old_process = {
            let mut process_guard = self.process.lock().await;
            process_guard.take()
        };
        if let Some(process) = old_process {
            let _ = process.stop().await;
        }

        let (process, channel) = self.start_bridge_process().await?;
        {
            let mut process_guard = self.process.lock().await;
            *process_guard = Some(process);
        }
        let _ = self.state_tx.send(BridgeState::Ready {
            channel: channel.clone(),
        });
        reconnect_handler.on_reconnect(&channel).map_err(|e| {
            XjError::Xslt(camel_xslt::XsltError::Bridge(format!(
                "xml-bridge reconnect handler failed: {e}"
            )))
        })?;

        Ok(())
    }

    pub(crate) async fn transform_with_retry_configured(
        &self,
        client: &XsltBridgeClient,
        stylesheet_id: &str,
        document: Vec<u8>,
        params: Vec<(String, String)>,
        retry_count: u32,
        retry_delay_ms: u64,
    ) -> Result<Vec<u8>, XjError> {
        self.ensure_bridge_started(client).await?;

        let policy = NetworkRetryPolicy {
            enabled: true,
            max_attempts: retry_count.saturating_add(1),
            initial_delay: Duration::from_millis(retry_delay_ms),
            multiplier: 2.0,
            max_delay: Duration::from_secs(30),
            jitter_factor: 0.0,
        };
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
                    None,
                )
                .await
                .map_err(XjError::from)
            {
                Ok(result) => return Ok(result),
                Err(mapped)
                    if Self::is_transient_error(&mapped) && policy.should_retry(attempt) =>
                {
                    let delay = policy.delay_for(attempt - 1);
                    tracing::warn!(
                        attempt,
                        max_attempts = policy.max_attempts,
                        delay_ms = delay.as_millis(),
                        error = %mapped,
                        "XJ transform transient error, retrying"
                    );
                    self.restart_bridge(client).await?;
                    tokio::time::sleep(delay).await;
                }
                Err(mapped) => return Err(mapped),
            }
        }
    }

    fn is_transport_error(msg: &str) -> bool {
        msg.contains(&Code::Unavailable.to_string())
            || msg.contains(&Code::Unknown.to_string())
            || msg.contains("transport")
    }

    pub(crate) fn state_rx(&self) -> &Arc<watch::Receiver<BridgeState>> {
        &self.state_rx
    }

    pub async fn shutdown(&self) {
        let mut guard = self.process.lock().await;
        if let Some(p) = guard.take()
            && let Err(e) = p.stop().await
        {
            tracing::warn!("Failed to stop XJ bridge process: {}", e);
        }
    }

    fn is_transient_error(err: &XjError) -> bool {
        let msg = err.to_string().to_lowercase();
        Self::is_transport_error(&msg)
            || msg.contains("io")
            || msg.contains("network")
            || msg.contains("timed out")
            || msg.contains("connection")
    }

    #[cfg(test)]
    async fn retry_transient_for_test<F, Fut>(
        retry_count: u32,
        retry_delay_ms: u64,
        mut op: F,
    ) -> Result<Vec<u8>, XjError>
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = Result<Vec<u8>, XjError>>,
    {
        let max_attempts = retry_count.saturating_add(1);
        let mut attempt = 1u32;
        let delay = Duration::from_millis(retry_delay_ms);

        loop {
            match op().await {
                Ok(v) => return Ok(v),
                Err(err) => {
                    if !Self::is_transient_error(&err) || attempt >= max_attempts {
                        return Err(err);
                    }
                    tokio::time::sleep(delay).await;
                    attempt = attempt.saturating_add(1);
                }
            }
        }
    }

    async fn start_bridge_process(
        &self,
    ) -> Result<(BridgeProcess, tonic::transport::Channel), XjError> {
        let binary_path = match &self.config.bridge_binary_path {
            Some(path) => path.clone(),
            None => ensure_binary_for_spec(
                &XML_BRIDGE,
                &self.config.bridge_version,
                &self.config.bridge_cache_dir,
            )
            .await
            .map_err(|e| XjError::Xslt(camel_xslt::XsltError::Bridge(e.to_string())))?,
        };

        let (process, channel) = BridgeProcess::start_and_connect(&BridgeProcessConfig::xml(
            binary_path,
            self.config.bridge_start_timeout_ms,
        ))
        .await
        .map_err(|e| XjError::Xslt(camel_xslt::XsltError::Bridge(e.to_string())))?;

        Ok((process, channel))
    }
}

pub struct XjComponent {
    runtime: Arc<XjBridgeRuntime>,
    client: Arc<XsltBridgeClient>,
}

impl Default for XjComponent {
    fn default() -> Self {
        Self::new(XjComponentConfig::default())
    }
}

impl XjComponent {
    pub fn new(config: XjComponentConfig) -> Self {
        let (state_tx, state_rx) = watch::channel(BridgeState::Starting);
        let state_rx = Arc::new(state_rx);
        let client = Arc::new(XsltBridgeClient::new(Arc::clone(&state_rx)));
        let runtime = Arc::new(XjBridgeRuntime::new(
            config,
            Arc::new(Mutex::new(None)),
            state_tx,
            Arc::clone(&state_rx),
        ));

        Self { runtime, client }
    }

    #[allow(dead_code)]
    pub fn with_client_for_testing(
        state_tx: watch::Sender<BridgeState>,
        state_rx: watch::Receiver<BridgeState>,
        client: Arc<XsltBridgeClient>,
    ) -> Self {
        let state_rx = Arc::new(state_rx);
        let runtime = Arc::new(XjBridgeRuntime::new(
            XjComponentConfig::default(),
            Arc::new(Mutex::new(None)),
            state_tx,
            state_rx,
        ));

        Self { runtime, client }
    }

    pub fn bridge_runtime(&self) -> Arc<XjBridgeRuntime> {
        Arc::clone(&self.runtime)
    }

    fn read_stylesheet(
        &self,
        stylesheet_uri: &str,
        direction: Direction,
    ) -> Result<Vec<u8>, XjError> {
        if stylesheet_uri == "classpath:identity" {
            return Ok(match direction {
                Direction::XmlToJson => XML_TO_JSON_XSLT.as_bytes().to_vec(),
                Direction::JsonToXml => JSON_TO_XML_XSLT.as_bytes().to_vec(),
            });
        }

        let path = if stylesheet_uri.starts_with("file://") {
            let url = url::Url::parse(stylesheet_uri).map_err(|e| {
                XjError::Xslt(camel_xslt::XsltError::Bridge(format!(
                    "invalid stylesheet URI: {e}"
                )))
            })?;
            url.to_file_path().map_err(|_| {
                XjError::Xslt(camel_xslt::XsltError::Bridge(
                    "invalid file:// stylesheet URI".to_string(),
                ))
            })?
        } else if stylesheet_uri.starts_with("classpath:") {
            return Err(XjError::Xslt(camel_xslt::XsltError::Bridge(format!(
                "unsupported classpath stylesheet URI: {stylesheet_uri}"
            ))));
        } else {
            PathBuf::from(stylesheet_uri)
        };

        std::fs::read(path).map_err(|e| {
            XjError::Xslt(camel_xslt::XsltError::Bridge(format!(
                "failed to read stylesheet: {e}"
            )))
        })
    }

    /// Block on an async future from a synchronous context (`create_endpoint`).
    ///
    /// This method is intentionally synchronous because `Component::create_endpoint`
    /// is a sync trait method. When called from within a tokio runtime (the common
    /// case), it uses `block_in_place` to avoid starving other tasks. When called
    /// from outside any runtime (e.g. tests), it creates a ephemeral single-threaded
    /// runtime.
    ///
    /// TODO(XJ-014): Consider making `create_endpoint` async upstream so this
    /// blocking shim can be removed entirely.
    fn block_on_result<F, T>(&self, fut: F) -> Result<T, CamelError>
    where
        F: Future<Output = Result<T, XjError>>,
    {
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            tokio::task::block_in_place(|| {
                handle
                    .block_on(fut)
                    .map_err(|e| CamelError::EndpointCreationFailed(e.to_string()))
            })
        } else {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|e| CamelError::EndpointCreationFailed(e.to_string()))?;
            runtime
                .block_on(fut)
                .map_err(|e| CamelError::EndpointCreationFailed(e.to_string()))
        }
    }
}

impl Component for XjComponent {
    fn scheme(&self) -> &str {
        "xj"
    }

    fn create_endpoint(
        &self,
        uri: &str,
        ctx: &dyn ComponentContext,
    ) -> Result<Box<dyn Endpoint>, CamelError> {
        let endpoint_config = XjEndpointConfig::from_uri(uri)?;
        self.block_on_result(self.runtime.ensure_bridge_started(self.client.as_ref()))?;

        let health_check = crate::health::XjHealthCheck::new(Arc::clone(self.runtime.state_rx()));
        ctx.register_current_route_health_check(Arc::new(health_check));

        let stylesheet_bytes = self
            .read_stylesheet(&endpoint_config.stylesheet_uri, endpoint_config.direction)
            .map_err(|e| CamelError::EndpointCreationFailed(e.to_string()))?;
        let stylesheet_id = self.block_on_result(async {
            self.client
                .compile(stylesheet_bytes)
                .await
                .map_err(XjError::from)
        })?;

        Ok(Box::new(XjEndpoint::new(
            uri.to_string(),
            stylesheet_id,
            endpoint_config.params,
            Arc::clone(&self.client),
            Arc::clone(&self.runtime),
            endpoint_config.direction,
            endpoint_config.max_payload_bytes,
            endpoint_config.retry_count,
            endpoint_config.retry_delay_ms,
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test]
    async fn retry_succeeds_after_two_transient_failures() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let attempts_clone = Arc::clone(&attempts);

        let result = XjBridgeRuntime::retry_transient_for_test(3, 1, move || {
            let attempts = Arc::clone(&attempts_clone);
            async move {
                let current = attempts.fetch_add(1, Ordering::SeqCst) + 1;
                if current <= 2 {
                    Err(XjError::Xslt(camel_xslt::XsltError::Bridge(
                        "transport unavailable".to_string(),
                    )))
                } else {
                    Ok(b"ok".to_vec())
                }
            }
        })
        .await;

        assert_eq!(result.unwrap(), b"ok".to_vec());
        assert_eq!(attempts.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn retry_returns_err_after_max_transient_failures() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let attempts_clone = Arc::clone(&attempts);

        let result = XjBridgeRuntime::retry_transient_for_test(2, 1, move || {
            let attempts = Arc::clone(&attempts_clone);
            async move {
                attempts.fetch_add(1, Ordering::SeqCst);
                Err(XjError::Xslt(camel_xslt::XsltError::Bridge(
                    "network timeout".to_string(),
                )))
            }
        })
        .await;

        assert!(result.is_err());
        assert_eq!(attempts.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn default_component_cache_dir_uses_xml_bridge() {
        let cfg = XjComponentConfig::default();
        assert!(
            cfg.bridge_cache_dir.ends_with("xml-bridge"),
            "expected xml bridge cache dir, got {}",
            cfg.bridge_cache_dir.display()
        );
        assert!(
            !cfg.bridge_cache_dir.ends_with("jms-bridge"),
            "XJ must not use JMS bridge cache dir"
        );
    }

    /// Regression: max_attempts=N → exactly N invocations (caught OpenSearch off-by-one 1f5c4c2a).
    /// Replicates the exact retry loop from `transform_with_retry_configured` (component.rs:146-176):
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
