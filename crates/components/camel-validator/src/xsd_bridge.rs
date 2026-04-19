use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use camel_bridge::channel::connect_channel;
use camel_bridge::download::{default_cache_dir_for_spec, ensure_binary_for_spec};
use camel_bridge::health::wait_for_health;
use camel_bridge::process::{BridgeError, BridgeProcess, BridgeProcessConfig};
use camel_bridge::reconnect::BridgeReconnectHandler;
use camel_bridge::spec::XML_BRIDGE;
use dashmap::DashMap;
use sha2::{Digest, Sha256};
use tokio::sync::{Mutex, RwLock, watch};
use tonic::Code;
use tonic::transport::Channel;
use tracing::warn;

use crate::error::ValidatorError;
use crate::proto;
use crate::proto::{
    HealthCheckRequest, RegisterSchemaRequest, RegisterSchemaResponse, ValidateResponse,
    ValidateWithRequest,
};

pub type SchemaId = String;

#[derive(Debug, Clone)]
pub enum BridgeState {
    Starting,
    Ready { channel: Channel },
    Degraded(String),
    Restarting { attempt: u32, next_at: Instant },
    Stopped,
}

pub struct XmlBridgeSlot {
    pub state_rx: watch::Receiver<BridgeState>,
    pub(crate) state_tx: watch::Sender<BridgeState>,
    pub process: Arc<Mutex<Option<BridgeProcess>>>,
}

impl std::fmt::Debug for XmlBridgeSlot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("XmlBridgeSlot").finish()
    }
}

type ConnectFuture = Pin<Box<dyn Future<Output = Result<Channel, BridgeError>> + Send>>;
type ConnectFn = dyn Fn(u16) -> ConnectFuture + Send + Sync;

#[async_trait]
pub trait XsdBridge: Send + Sync {
    async fn register(&self, xsd_bytes: Vec<u8>) -> Result<SchemaId, ValidatorError>;
    async fn validate(&self, schema_id: &str, doc_bytes: Vec<u8>) -> Result<(), ValidatorError>;
}

#[async_trait]
trait XsdBridgeRpc: Send + Sync {
    async fn register_schema(
        &self,
        channel: Channel,
        request: RegisterSchemaRequest,
    ) -> Result<RegisterSchemaResponse, ValidatorError>;

    async fn validate_with(
        &self,
        channel: Channel,
        request: ValidateWithRequest,
    ) -> Result<ValidateResponse, ValidatorError>;
}

#[derive(Debug)]
struct GrpcXsdBridgeRpc;

#[async_trait]
impl XsdBridgeRpc for GrpcXsdBridgeRpc {
    async fn register_schema(
        &self,
        channel: Channel,
        request: RegisterSchemaRequest,
    ) -> Result<RegisterSchemaResponse, ValidatorError> {
        let mut client = proto::xsd_validator_client::XsdValidatorClient::new(channel);
        let response = client.register_schema(request).await.map_err(|e| {
            ValidatorError::Transport(format!("xml-bridge register_schema RPC failed: {e}"))
        })?;
        Ok(response.into_inner())
    }

    async fn validate_with(
        &self,
        channel: Channel,
        request: ValidateWithRequest,
    ) -> Result<ValidateResponse, ValidatorError> {
        let mut client = proto::xsd_validator_client::XsdValidatorClient::new(channel);
        let response = client.validate_with(request).await.map_err(|e| {
            ValidatorError::Transport(format!("xml-bridge validate_with RPC failed: {e}"))
        })?;
        Ok(response.into_inner())
    }
}

#[derive(Clone)]
pub struct XsdBridgeBackend {
    channel: Arc<RwLock<Option<Channel>>>,
    schemas: Arc<DashMap<SchemaId, Vec<u8>>>,
    slot: Arc<XmlBridgeSlot>,
    rpc: Arc<dyn XsdBridgeRpc>,
    connect_fn: Arc<ConnectFn>,
    start_lock: Arc<Mutex<()>>,
    bridge_version: String,
    bridge_cache_dir: std::path::PathBuf,
    bridge_start_timeout_ms: u64,
}

impl std::fmt::Debug for XsdBridgeBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("XsdBridgeBackend")
            .field("bridge_version", &self.bridge_version)
            .field("bridge_cache_dir", &self.bridge_cache_dir)
            .finish()
    }
}

impl XsdBridgeBackend {
    pub fn new() -> Self {
        let (state_tx, state_rx) = watch::channel(BridgeState::Stopped);
        let slot = Arc::new(XmlBridgeSlot {
            state_rx,
            state_tx,
            process: Arc::new(Mutex::new(None)),
        });

        Self {
            channel: Arc::new(RwLock::new(None)),
            schemas: Arc::new(DashMap::new()),
            slot,
            rpc: Arc::new(GrpcXsdBridgeRpc),
            connect_fn: Arc::new(|port| Box::pin(connect_channel(port))),
            start_lock: Arc::new(Mutex::new(())),
            bridge_version: crate::BRIDGE_VERSION.to_string(),
            bridge_cache_dir: default_cache_dir_for_spec(&XML_BRIDGE),
            bridge_start_timeout_ms: 30_000,
        }
    }

    #[cfg(test)]
    fn for_test(rpc: Arc<dyn XsdBridgeRpc>, connect_fn: Arc<ConnectFn>, channel: Channel) -> Self {
        let (state_tx, state_rx) = watch::channel(BridgeState::Ready {
            channel: channel.clone(),
        });
        let slot = Arc::new(XmlBridgeSlot {
            state_rx,
            state_tx,
            process: Arc::new(Mutex::new(None)),
        });
        Self {
            channel: Arc::new(RwLock::new(Some(channel))),
            schemas: Arc::new(DashMap::new()),
            slot,
            rpc,
            connect_fn,
            start_lock: Arc::new(Mutex::new(())),
            bridge_version: crate::BRIDGE_VERSION.to_string(),
            bridge_cache_dir: default_cache_dir_for_spec(&XML_BRIDGE),
            bridge_start_timeout_ms: 30_000,
        }
    }

    pub fn schema_id_for(xsd_bytes: &[u8]) -> SchemaId {
        let mut hasher = Sha256::new();
        hasher.update(xsd_bytes);
        format!("xsd-{}", hex::encode(hasher.finalize()))
    }

    async fn ensure_bridge_ready(&self) -> Result<Channel, ValidatorError> {
        if let Some(ch) = self.channel.read().await.clone() {
            return Ok(ch);
        }

        let _guard = self.start_lock.lock().await;
        if let Some(ch) = self.channel.read().await.clone() {
            return Ok(ch);
        }

        let _ = self.slot.state_tx.send(BridgeState::Starting);
        let (process, channel, port) = self.start_bridge_process().await?;
        {
            let mut process_guard = self.slot.process.lock().await;
            *process_guard = Some(process);
        }
        {
            let mut ch_guard = self.channel.write().await;
            *ch_guard = Some(channel.clone());
        }
        let _ = self.slot.state_tx.send(BridgeState::Ready {
            channel: channel.clone(),
        });
        self.on_reconnect(port).map_err(|e| {
            ValidatorError::Transport(format!("xml-bridge reconnect handler failed: {e}"))
        })?;

        Ok(channel)
    }

    async fn restart_bridge(&self) -> Result<Channel, ValidatorError> {
        let _guard = self.start_lock.lock().await;

        let _ = self.slot.state_tx.send(BridgeState::Restarting {
            attempt: 0,
            next_at: Instant::now(),
        });

        let old_process = {
            let mut process_guard = self.slot.process.lock().await;
            process_guard.take()
        };
        if let Some(p) = old_process {
            let _ = p.stop().await;
        }

        let (process, channel, port) = self.start_bridge_process().await?;
        {
            let mut process_guard = self.slot.process.lock().await;
            *process_guard = Some(process);
        }
        {
            let mut ch_guard = self.channel.write().await;
            *ch_guard = Some(channel.clone());
        }

        self.on_reconnect(port).map_err(|e| {
            ValidatorError::Transport(format!("xml-bridge reconnect handler failed: {e}"))
        })?;
        let _ = self.slot.state_tx.send(BridgeState::Ready {
            channel: channel.clone(),
        });

        Ok(channel)
    }

    async fn start_bridge_process(&self) -> Result<(BridgeProcess, Channel, u16), ValidatorError> {
        let binary_path =
            ensure_binary_for_spec(&XML_BRIDGE, &self.bridge_version, &self.bridge_cache_dir)
                .await
                .map_err(|e| {
                    ValidatorError::endpoint(format!("XML bridge binary unavailable: {e}"))
                })?;

        let process_config = BridgeProcessConfig::xml(binary_path, self.bridge_start_timeout_ms);
        let process = BridgeProcess::start(&process_config)
            .await
            .map_err(|e| ValidatorError::endpoint(format!("XML bridge start failed: {e}")))?;
        let port = process.grpc_port();
        let channel = (self.connect_fn)(port).await.map_err(|e| {
            ValidatorError::endpoint(format!("XML bridge channel connect failed: {e}"))
        })?;

        wait_for_health(&channel, Duration::from_secs(10), |ch| {
            let mut client = proto::health_client::HealthClient::new(ch);
            async move {
                let resp = client.check(HealthCheckRequest {}).await?;
                Ok(resp.into_inner().status == "SERVING")
            }
        })
        .await
        .map_err(|e| ValidatorError::endpoint(format!("XML bridge health check failed: {e}")))?;

        Ok((process, channel, port))
    }

    fn is_transport_error(msg: &str) -> bool {
        msg.contains(&Code::Unavailable.to_string())
            || msg.contains(&Code::Unknown.to_string())
            || msg.contains("transport")
    }

    async fn register_with_channel(
        &self,
        channel: Channel,
        schema_id: SchemaId,
        xsd_bytes: Vec<u8>,
    ) -> Result<SchemaId, ValidatorError> {
        let response = self
            .rpc
            .register_schema(
                channel,
                RegisterSchemaRequest {
                    schema_id: schema_id.clone(),
                    schema: xsd_bytes.clone(),
                },
            )
            .await?;

        if let Some(err) = response.error {
            return Err(ValidatorError::from_bridge_error(&err));
        }

        self.schemas.insert(schema_id.clone(), xsd_bytes);
        Ok(schema_id)
    }
}

impl BridgeReconnectHandler for XsdBridgeBackend {
    fn on_reconnect(&self, _port: u16) -> Result<(), BridgeError> {
        let this = self.clone();
        tokio::spawn(async move {
            let Some(channel) = this.channel.read().await.clone() else {
                return;
            };

            let schemas: Vec<(SchemaId, Vec<u8>)> = this
                .schemas
                .iter()
                .map(|entry| (entry.key().clone(), entry.value().clone()))
                .collect();

            for (schema_id, schema_bytes) in schemas {
                if let Err(e) = this
                    .rpc
                    .register_schema(
                        channel.clone(),
                        RegisterSchemaRequest {
                            schema_id: schema_id.clone(),
                            schema: schema_bytes,
                        },
                    )
                    .await
                {
                    warn!(schema_id = %schema_id, error = %e, "re-seed schema failed after reconnect");
                }
            }
        });
        Ok(())
    }
}

#[async_trait]
impl XsdBridge for XsdBridgeBackend {
    async fn register(&self, xsd_bytes: Vec<u8>) -> Result<SchemaId, ValidatorError> {
        let schema_id = Self::schema_id_for(&xsd_bytes);
        if self.schemas.contains_key(&schema_id) {
            return Ok(schema_id);
        }

        let channel = self.ensure_bridge_ready().await?;
        match self
            .register_with_channel(channel.clone(), schema_id.clone(), xsd_bytes.clone())
            .await
        {
            Ok(id) => Ok(id),
            Err(e) if Self::is_transport_error(&e.to_string()) => {
                let restarted = self.restart_bridge().await?;
                self.register_with_channel(restarted, schema_id, xsd_bytes)
                    .await
            }
            Err(e) => Err(e),
        }
    }

    async fn validate(&self, schema_id: &str, doc_bytes: Vec<u8>) -> Result<(), ValidatorError> {
        let channel = self.ensure_bridge_ready().await?;
        let req = ValidateWithRequest {
            schema_id: schema_id.to_string(),
            document: doc_bytes.clone(),
        };

        let response = match self.rpc.validate_with(channel.clone(), req).await {
            Ok(resp) => resp,
            Err(e) if Self::is_transport_error(&e.to_string()) => {
                let restarted = self.restart_bridge().await?;
                self.rpc
                    .validate_with(
                        restarted,
                        ValidateWithRequest {
                            schema_id: schema_id.to_string(),
                            document: doc_bytes,
                        },
                    )
                    .await?
            }
            Err(e) => return Err(e),
        };

        if let Some(err) = response.error {
            return Err(ValidatorError::from_bridge_error(&err));
        }
        if response.valid {
            return Ok(());
        }

        let details = response
            .errors
            .iter()
            .map(|e| format!("{}:{} {}", e.line, e.column, e.message))
            .collect::<Vec<_>>()
            .join("\n");
        Err(ValidatorError::validation(format!(
            "XSD validation failed:\n{details}"
        )))
    }
}

impl Default for XsdBridgeBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tonic::transport::Endpoint;

    #[derive(Debug)]
    struct MockRpc {
        register_calls: Arc<AtomicUsize>,
        validate_ok: bool,
    }

    #[async_trait]
    impl XsdBridgeRpc for MockRpc {
        async fn register_schema(
            &self,
            _channel: Channel,
            request: RegisterSchemaRequest,
        ) -> Result<RegisterSchemaResponse, ValidatorError> {
            self.register_calls.fetch_add(1, Ordering::SeqCst);
            Ok(RegisterSchemaResponse {
                schema_id: request.schema_id,
                error: None,
            })
        }

        async fn validate_with(
            &self,
            _channel: Channel,
            _request: ValidateWithRequest,
        ) -> Result<ValidateResponse, ValidatorError> {
            Ok(ValidateResponse {
                valid: self.validate_ok,
                errors: Vec::new(),
                error: None,
            })
        }
    }

    fn lazy_channel() -> Channel {
        Endpoint::from_static("http://127.0.0.1:65535").connect_lazy()
    }

    #[tokio::test]
    async fn xsd_bridge_reconnect_reseeds() {
        let calls = Arc::new(AtomicUsize::new(0));
        let rpc = Arc::new(MockRpc {
            register_calls: Arc::clone(&calls),
            validate_ok: true,
        });
        let connector =
            Arc::new(|_port| Box::pin(async move { Ok(lazy_channel()) }) as ConnectFuture);
        let backend = XsdBridgeBackend::for_test(rpc, connector, lazy_channel());

        let _id_a = backend.register(b"<xsd:a/>".to_vec()).await.unwrap();
        let _id_b = backend.register(b"<xsd:b/>".to_vec()).await.unwrap();

        backend.on_reconnect(50051).unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;

        assert!(calls.load(Ordering::SeqCst) >= 4);
    }
}
