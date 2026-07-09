use crate::error::XsltError;
use crate::proto::{
    CompileStylesheetRequest, TransformRequest, xslt_transformer_client::XsltTransformerClient,
};
use async_trait::async_trait;
use camel_bridge::{process::BridgeError, reconnect::BridgeReconnectHandler};
use camel_component_api::RuntimeObservability;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use tokio::sync::watch;
use tonic::Code;
use tonic::transport::Channel;
use tracing::{error, warn};

pub type StylesheetId = String;

const DEFAULT_MAX_CACHE_ENTRIES: usize = 256;

#[derive(Debug)]
struct StylesheetCache {
    inner: Mutex<StylesheetCacheInner>,
}

#[derive(Debug)]
struct StylesheetCacheInner {
    max_entries: usize,
    stylesheets: HashMap<StylesheetId, Vec<u8>>,
    insertion_order: VecDeque<StylesheetId>,
}

impl StylesheetCache {
    fn new() -> Self {
        Self::with_max_entries(DEFAULT_MAX_CACHE_ENTRIES)
    }

    fn with_max_entries(max_entries: usize) -> Self {
        Self {
            inner: Mutex::new(StylesheetCacheInner {
                max_entries,
                stylesheets: HashMap::new(),
                insertion_order: VecDeque::new(),
            }),
        }
    }

    fn contains_key(&self, key: &str) -> bool {
        let inner = self.inner.lock().expect("stylesheet cache mutex poisoned"); // allow-unwrap
        inner.stylesheets.contains_key(key)
    }

    fn insert(&self, key: impl Into<StylesheetId>, value: Vec<u8>) {
        let key = key.into();
        let mut inner = self.inner.lock().expect("stylesheet cache mutex poisoned"); // allow-unwrap

        if let std::collections::hash_map::Entry::Occupied(mut entry) =
            inner.stylesheets.entry(key.clone())
        {
            entry.insert(value);
            return;
        }

        if inner.max_entries == 0 {
            return;
        }

        if inner.stylesheets.len() >= inner.max_entries
            && let Some(oldest_key) = inner.insertion_order.pop_front()
        {
            inner.stylesheets.remove(&oldest_key);
        }

        inner.insertion_order.push_back(key.clone());
        inner.stylesheets.insert(key, value);
    }

    fn snapshot(&self) -> Vec<(StylesheetId, Vec<u8>)> {
        let inner = self.inner.lock().expect("stylesheet cache mutex poisoned"); // allow-unwrap
        inner
            .stylesheets
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }

    fn len(&self) -> usize {
        let inner = self.inner.lock().expect("stylesheet cache mutex poisoned"); // allow-unwrap
        inner.stylesheets.len()
    }
}

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
        channel: &Channel,
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
            .map_err(map_transport_status)?
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
            .map_err(map_transport_status)?
            .into_inner();

        Ok((response.result, response.error.map(|e| e.message)))
    }

    async fn recompile_all(
        &self,
        channel: &Channel,
        stylesheets: Vec<(StylesheetId, Vec<u8>)>,
    ) -> Result<(), XsltError> {
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

fn map_transport_status(status: tonic::Status) -> XsltError {
    let code = status.code();
    let message = status.to_string();
    if matches!(code, Code::Unavailable | Code::Unknown) || message.contains("transport") {
        XsltError::BridgeTransport { code, message }
    } else {
        XsltError::Bridge(message)
    }
}

pub struct XsltBridgeClient {
    state_rx: Arc<watch::Receiver<BridgeState>>,
    backend: Arc<dyn XsltTransformBackend>,
    stylesheets: Arc<StylesheetCache>,
    observability: OnceLock<(Arc<dyn RuntimeObservability>, String)>,
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
            stylesheets: Arc::new(StylesheetCache::new()),
            observability: OnceLock::new(),
        }
    }

    pub fn set_observability(&self, runtime: Arc<dyn RuntimeObservability>, route_id: String) {
        self.observability.set((runtime, route_id)).ok();
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
    fn on_reconnect(&self, channel: &Channel) -> Result<(), BridgeError> {
        let channel = channel.clone();
        let handle = tokio::runtime::Handle::try_current().map_err(|e| {
            BridgeError::Transport(format!("tokio runtime unavailable for reconnect: {e}"))
        })?;

        let backend = Arc::clone(&self.backend);
        let stylesheets = self.stylesheets.snapshot();

        let observability = self
            .observability
            .get()
            .map(|(rt, rid)| (Arc::clone(rt), rid.clone()));

        handle.spawn(async move {
            if let Err(err) = backend.recompile_all(&channel, stylesheets).await {
                if let Some((rt, rid)) = observability {
                    rt.metrics()
                        .increment_errors(&rid, "e:xslt:reconnect-reseed");
                }
                // log-policy: outside-contract
                error!(error = %err, "failed to re-seed stylesheets after reconnect");
            }
        });

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stylesheet_a() -> Vec<u8> {
        b"a".to_vec()
    }

    fn stylesheet_b() -> Vec<u8> {
        b"b".to_vec()
    }

    fn stylesheet_c() -> Vec<u8> {
        b"c".to_vec()
    }

    fn stylesheet_d() -> Vec<u8> {
        b"d".to_vec()
    }

    #[test]
    fn test_xslt_cache_does_not_grow_beyond_max() {
        let mut cache = StylesheetCache::with_max_entries(3);
        cache.insert("a", stylesheet_a());
        cache.insert("b", stylesheet_b());
        cache.insert("c", stylesheet_c());
        cache.insert("d", stylesheet_d()); // triggers eviction
        assert!(cache.len() <= 3);
    }
}
