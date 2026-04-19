use crate::config::{Direction, XjEndpointConfig};
use crate::endpoint::XjEndpoint;
use crate::error::XjError;
use crate::identity::{JSON_TO_XML_XSLT, XML_TO_JSON_XSLT};
use camel_bridge::{
    channel::connect_channel,
    download::ensure_binary_for_spec,
    process::{BridgeProcess, BridgeProcessConfig},
    reconnect::BridgeReconnectHandler,
    spec::XML_BRIDGE,
};
use camel_component_api::{CamelError, Component, ComponentContext, Endpoint};
use camel_xslt::{BridgeState, XsltBridgeClient};
use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;
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
            bridge_cache_dir: camel_bridge::download::default_cache_dir(),
        }
    }
}

pub(crate) struct XjBridgeRuntime {
    config: XjComponentConfig,
    process: Arc<Mutex<Option<BridgeProcess>>>,
    state_tx: watch::Sender<BridgeState>,
    state_rx: Arc<watch::Receiver<BridgeState>>,
    start_lock: Arc<Mutex<()>>,
}

impl XjBridgeRuntime {
    fn new(
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

        let (process, channel, port) = self.start_bridge_process().await?;
        {
            let mut process_guard = self.process.lock().await;
            *process_guard = Some(process);
        }
        let _ = self.state_tx.send(BridgeState::Ready {
            channel: channel.clone(),
        });
        reconnect_handler.on_reconnect(port).map_err(|e| {
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

        let (process, channel, port) = self.start_bridge_process().await?;
        {
            let mut process_guard = self.process.lock().await;
            *process_guard = Some(process);
        }
        let _ = self.state_tx.send(BridgeState::Ready {
            channel: channel.clone(),
        });
        reconnect_handler.on_reconnect(port).map_err(|e| {
            XjError::Xslt(camel_xslt::XsltError::Bridge(format!(
                "xml-bridge reconnect handler failed: {e}"
            )))
        })?;

        Ok(())
    }

    pub(crate) async fn transform_with_retry(
        &self,
        client: &XsltBridgeClient,
        stylesheet_id: &str,
        document: Vec<u8>,
        params: Vec<(String, String)>,
    ) -> Result<Vec<u8>, XjError> {
        self.ensure_bridge_started(client).await?;

        match client
            .transform(
                &stylesheet_id.to_string(),
                document.clone(),
                params.clone(),
                None,
            )
            .await
        {
            Ok(result) => Ok(result),
            Err(err) if Self::is_transport_error(&err.to_string()) => {
                self.restart_bridge(client).await?;
                client
                    .transform(&stylesheet_id.to_string(), document, params, None)
                    .await
                    .map_err(XjError::from)
            }
            Err(err) => Err(XjError::from(err)),
        }
    }

    fn is_transport_error(msg: &str) -> bool {
        msg.contains(&Code::Unavailable.to_string())
            || msg.contains(&Code::Unknown.to_string())
            || msg.contains("transport")
    }

    async fn start_bridge_process(
        &self,
    ) -> Result<(BridgeProcess, tonic::transport::Channel, u16), XjError> {
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

        let process = BridgeProcess::start(&BridgeProcessConfig::xml(
            binary_path,
            self.config.bridge_start_timeout_ms,
        ))
        .await
        .map_err(|e| XjError::Xslt(camel_xslt::XsltError::Bridge(e.to_string())))?;
        let port = process.grpc_port();

        let channel = connect_channel(port)
            .await
            .map_err(|e| XjError::Xslt(camel_xslt::XsltError::Bridge(e.to_string())))?;

        Ok((process, channel, port))
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
        _ctx: &dyn ComponentContext,
    ) -> Result<Box<dyn Endpoint>, CamelError> {
        let endpoint_config = XjEndpointConfig::from_uri(uri)?;
        self.block_on_result(self.runtime.ensure_bridge_started(self.client.as_ref()))?;
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
        )))
    }
}
