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
use tonic::Code;

pub(crate) struct XsltBridgeRuntime {
    config: XsltComponentConfig,
    process: Arc<Mutex<Option<BridgeProcess>>>,
    state_tx: watch::Sender<BridgeState>,
    state_rx: Arc<watch::Receiver<BridgeState>>,
    start_lock: Arc<Mutex<()>>,
}

impl XsltBridgeRuntime {
    fn new(
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

        match client
            .transform(
                &stylesheet_id.to_string(),
                document.clone(),
                params.clone(),
                output_method.clone(),
            )
            .await
        {
            Ok(result) => Ok(result),
            Err(err) if Self::is_transport_error(&err.to_string()) => {
                self.restart_bridge(client).await?;
                client
                    .transform(&stylesheet_id.to_string(), document, params, output_method)
                    .await
            }
            Err(err) => Err(err),
        }
    }

    fn is_transport_error(msg: &str) -> bool {
        msg.contains(&Code::Unavailable.to_string())
            || msg.contains(&Code::Unknown.to_string())
            || msg.contains("transport")
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
            .map_err(|e| XsltError::Bridge(e.to_string()))?,
        };

        let process = BridgeProcess::start(&BridgeProcessConfig::xml(
            binary_path,
            self.config.bridge_start_timeout_ms,
        ))
        .await
        .map_err(|e| XsltError::Bridge(e.to_string()))?;
        let port = process.grpc_port();

        let channel = connect_channel(port)
            .await
            .map_err(|e| XsltError::Bridge(e.to_string()))?;

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

    fn read_stylesheet(&self, stylesheet_uri: &str) -> Result<Vec<u8>, XsltError> {
        let path = if stylesheet_uri.starts_with("file://") {
            let url = url::Url::parse(stylesheet_uri)
                .map_err(|e| XsltError::Bridge(format!("invalid stylesheet URI: {e}")))?;
            url.to_file_path()
                .map_err(|_| XsltError::Bridge("invalid file:// stylesheet URI".to_string()))?
        } else {
            PathBuf::from(stylesheet_uri)
        };

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
        _ctx: &dyn ComponentContext,
    ) -> Result<Box<dyn Endpoint>, CamelError> {
        let endpoint_config = XsltEndpointConfig::from_uri(uri)?;
        let stylesheet_bytes = self
            .read_stylesheet(&endpoint_config.stylesheet_uri)
            .map_err(|e| CamelError::EndpointCreationFailed(e.to_string()))?;

        Ok(Box::new(XsltEndpoint::new(
            uri.to_string(),
            stylesheet_bytes,
            endpoint_config.params,
            endpoint_config.output_method,
            Arc::clone(&self.client),
            Arc::clone(&self.runtime),
        )))
    }
}
