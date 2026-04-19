use crate::error::XsltError;
use crate::proto::{
    CompileStylesheetRequest, TransformRequest, xslt_transformer_client::XsltTransformerClient,
};
use async_trait::async_trait;
use camel_bridge::{process::BridgeError, reconnect::BridgeReconnectHandler};
use dashmap::DashMap;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;
use tonic::transport::Channel;
use tracing::warn;

pub type StylesheetId = String;

#[derive(Debug, Clone)]
pub enum BridgeState {
    Starting,
    Ready { channel: Channel },
    Degraded(String),
    Restarting { attempt: u32 },
    Stopped,
}

#[async_trait]
pub trait XsltTransformBackend: Send + Sync + std::fmt::Debug {
    async fn compile(
        &self,
        channel: Channel,
        stylesheet_id: StylesheetId,
        stylesheet: Vec<u8>,
    ) -> Result<Option<String>, XsltError>;

    async fn transform(
        &self,
        channel: Channel,
        stylesheet_id: StylesheetId,
        document: Vec<u8>,
        parameters: HashMap<String, String>,
        output_method: String,
    ) -> Result<(Vec<u8>, Option<String>), XsltError>;

    async fn recompile_all(
        &self,
        port: u16,
        stylesheets: Vec<(StylesheetId, Vec<u8>)>,
    ) -> Result<(), XsltError>;
}

#[derive(Debug, Default)]
struct GrpcXsltBackend;

#[async_trait]
impl XsltTransformBackend for GrpcXsltBackend {
    async fn compile(
        &self,
        channel: Channel,
        stylesheet_id: StylesheetId,
        stylesheet: Vec<u8>,
    ) -> Result<Option<String>, XsltError> {
        let mut client = XsltTransformerClient::new(channel);
        let response = client
            .compile_stylesheet(CompileStylesheetRequest {
                stylesheet_id,
                stylesheet,
            })
            .await
            .map_err(|e| XsltError::Bridge(e.to_string()))?
            .into_inner();

        Ok(response.error.map(|e| e.message))
    }

    async fn transform(
        &self,
        channel: Channel,
        stylesheet_id: StylesheetId,
        document: Vec<u8>,
        parameters: HashMap<String, String>,
        output_method: String,
    ) -> Result<(Vec<u8>, Option<String>), XsltError> {
        let mut client = XsltTransformerClient::new(channel);
        let mut request = tonic::Request::new(TransformRequest {
            stylesheet_id,
            document,
            parameters,
            output_method,
        });
        request.set_timeout(Duration::from_secs(60));

        let response = client
            .transform(request)
            .await
            .map_err(|e| XsltError::Bridge(e.to_string()))?
            .into_inner();

        Ok((response.result, response.error.map(|e| e.message)))
    }

    async fn recompile_all(
        &self,
        port: u16,
        stylesheets: Vec<(StylesheetId, Vec<u8>)>,
    ) -> Result<(), XsltError> {
        let channel = camel_bridge::channel::connect_channel(port)
            .await
            .map_err(|e| XsltError::Bridge(e.to_string()))?;

        for (stylesheet_id, stylesheet) in stylesheets {
            if let Some(err) = self
                .compile(channel.clone(), stylesheet_id.clone(), stylesheet)
                .await?
            {
                warn!(stylesheet_id = %stylesheet_id, error = %err, "re-seed compile failed");
            }
        }

        Ok(())
    }
}

pub struct XsltBridgeClient {
    state_rx: Arc<watch::Receiver<BridgeState>>,
    backend: Arc<dyn XsltTransformBackend>,
    stylesheets: Arc<DashMap<StylesheetId, Vec<u8>>>,
}

impl std::fmt::Debug for XsltBridgeClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("XsltBridgeClient")
            .field("stylesheets", &self.stylesheets.len())
            .finish()
    }
}

impl XsltBridgeClient {
    pub fn new(state_rx: Arc<watch::Receiver<BridgeState>>) -> Self {
        Self::with_backend(state_rx, Arc::new(GrpcXsltBackend))
    }

    pub fn with_backend(
        state_rx: Arc<watch::Receiver<BridgeState>>,
        backend: Arc<dyn XsltTransformBackend>,
    ) -> Self {
        Self {
            state_rx,
            backend,
            stylesheets: Arc::new(DashMap::new()),
        }
    }

    pub fn stylesheet_id_for(xslt_bytes: &[u8]) -> StylesheetId {
        let digest = Sha256::digest(xslt_bytes);
        let mut hex = String::with_capacity(digest.len() * 2);
        for byte in digest {
            use std::fmt::Write as _;
            let _ = write!(hex, "{byte:02x}");
        }
        format!("xslt-{hex}")
    }

    pub async fn compile(&self, xslt_bytes: Vec<u8>) -> Result<StylesheetId, XsltError> {
        let stylesheet_id = Self::stylesheet_id_for(&xslt_bytes);
        if self.stylesheets.contains_key(&stylesheet_id) {
            return Ok(stylesheet_id);
        }

        let channel = self.ready_channel()?;

        if let Some(err) = self
            .backend
            .compile(channel, stylesheet_id.clone(), xslt_bytes.clone())
            .await?
        {
            return Err(XsltError::CompileFailed(err));
        }

        self.stylesheets.insert(stylesheet_id.clone(), xslt_bytes);
        Ok(stylesheet_id)
    }

    pub async fn transform(
        &self,
        id: &StylesheetId,
        document: Vec<u8>,
        params: Vec<(String, String)>,
        output_method: Option<String>,
    ) -> Result<Vec<u8>, XsltError> {
        let channel = self.ready_channel()?;
        let parameters = params.into_iter().collect::<HashMap<_, _>>();
        let (result, error) = self
            .backend
            .transform(
                channel,
                id.clone(),
                document,
                parameters,
                output_method.unwrap_or_default(),
            )
            .await?;

        if let Some(err) = error {
            return Err(XsltError::TransformFailed(err));
        }

        Ok(result)
    }

    fn ready_channel(&self) -> Result<Channel, XsltError> {
        match &*self.state_rx.borrow() {
            BridgeState::Ready { channel } => Ok(channel.clone()),
            _ => Err(XsltError::Bridge("bridge not ready".to_string())),
        }
    }
}

impl BridgeReconnectHandler for XsltBridgeClient {
    fn on_reconnect(&self, port: u16) -> Result<(), BridgeError> {
        let handle = tokio::runtime::Handle::try_current().map_err(|e| {
            BridgeError::Transport(format!("tokio runtime unavailable for reconnect: {e}"))
        })?;

        let backend = Arc::clone(&self.backend);
        let stylesheets = self
            .stylesheets
            .iter()
            .map(|entry| (entry.key().clone(), entry.value().clone()))
            .collect::<Vec<_>>();

        handle.spawn(async move {
            if let Err(err) = backend.recompile_all(port, stylesheets).await {
                warn!(error = %err, "failed to re-seed stylesheets after reconnect");
            }
        });

        Ok(())
    }
}
