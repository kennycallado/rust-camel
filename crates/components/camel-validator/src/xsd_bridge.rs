use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use camel_bridge::download::{default_cache_dir_for_spec, ensure_binary_for_spec};
use camel_bridge::health::wait_for_health;
use camel_bridge::process::{BridgeError, BridgeProcess, BridgeProcessConfig};
use camel_bridge::reconnect::BridgeReconnectHandler;
use camel_bridge::spec::XML_BRIDGE;
use camel_component_api::RuntimeObservability;
use dashmap::DashMap;
use prost::Message;
use sha2::{Digest, Sha256};
use std::sync::OnceLock;
use tokio::sync::{Mutex, RwLock, watch};
use tonic::transport::Channel;
use tracing::error;

use crate::error::ValidatorError;
use crate::proto;
use crate::proto::{
    HealthCheckRequest, RegisterSchemaRequest, RegisterSchemaResponse, ValidateResponse,
    ValidateWithRequest,
};
use crate::schema_set::SchemaSet;

pub type SchemaId = String;

impl RegisterSchemaRequest {
    /// Builds the wire request for a fully discovered schema set.
    ///
    /// This is the single conversion seam for both the first registration and
    /// reconnect replay, so the internal `BTreeMap`s are mapped onto the
    /// generated `HashMap` fields in exactly one place.
    pub(crate) fn new(schema_id: SchemaId, set: &SchemaSet) -> Self {
        Self {
            schema_id,
            schema: set.root_bytes().to_vec(),
            root_system_id: set.root_system_id.clone(),
            resources: set
                .resources
                .iter()
                .map(|(key, bytes)| (key.clone(), bytes.clone()))
                .collect(),
            aliases: set
                .aliases
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
        }
    }

    fn check_encoded_size(&self) -> Result<(), ValidatorError> {
        if self.encoded_len() > XSD_BRIDGE_MAX_REGISTER_REQUEST_SIZE {
            return Err(ValidatorError::Transport {
                message: "SCHEMA_SET_REQUEST_SIZE_CAP: encoded registration exceeds 17 MiB"
                    .to_string(),
                source: None,
            });
        }
        Ok(())
    }
}

// ── Bridge gRPC decode limit ─────────────────────────────────────────────────

/// Maximum inbound gRPC message size accepted when decoding Java bridge
/// responses.
///
/// The xml-bridge uses a 17 MiB ingress ceiling (see
/// `bridges/xml/src/main/resources/application.yml`). The response ceiling
/// also covers the protobuf envelope around a maximal response. Without this,
/// tonic's 4 MiB default rejects legal responses and surfaces a broken or
/// oversized bridge reply as an opaque transport error instead of a domain
/// error.
pub(crate) const XSD_BRIDGE_MAX_DECODING_MESSAGE_SIZE: usize = 17 * 1024 * 1024;

/// Full encoded registration ceiling, including the legacy root mirror and
/// virtual-key metadata. Also bounds full encoded validation requests. Keep
/// synchronized with XML server ingress.
pub(crate) const XSD_BRIDGE_MAX_REGISTER_REQUEST_SIZE: usize = XSD_BRIDGE_MAX_DECODING_MESSAGE_SIZE;

pub(crate) fn xsd_bridge_decode_limit() -> usize {
    XSD_BRIDGE_MAX_DECODING_MESSAGE_SIZE
}

/// Production-path constructor for bridge gRPC clients.
///
/// Applies [`xsd_bridge_decode_limit`] to every RPC issued through this
/// client. All `XsdValidatorClient` construction sites must go through this
/// helper rather than calling `XsdValidatorClient::new` directly, so a
/// channel handed over by the backend always decodes maximal legal messages.
pub(crate) fn xsd_bridge_client(
    channel: Channel,
) -> proto::xsd_validator_client::XsdValidatorClient<Channel> {
    proto::xsd_validator_client::XsdValidatorClient::new(channel)
        .max_decoding_message_size(xsd_bridge_decode_limit())
}

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

#[async_trait]
pub trait XsdBridge: Send + Sync {
    /// Registers a fully discovered schema set under `schema_id`.
    ///
    /// Callers own discovery: the backend performs no filesystem access and
    /// never re-discovers or re-roots the set.
    async fn register(&self, schema_id: &str, set: &SchemaSet) -> Result<SchemaId, ValidatorError>;
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
        let mut client = xsd_bridge_client(channel);
        let response = client.register_schema(request).await.map_err(|e| {
            ValidatorError::transport_with_source("xml-bridge register_schema RPC failed", e)
        })?;
        Ok(response.into_inner())
    }

    async fn validate_with(
        &self,
        channel: Channel,
        request: ValidateWithRequest,
    ) -> Result<ValidateResponse, ValidatorError> {
        let mut client = xsd_bridge_client(channel);
        let response = client.validate_with(request).await.map_err(|e| {
            ValidatorError::transport_with_source("xml-bridge validate_with RPC failed", e)
        })?;
        Ok(response.into_inner())
    }
}

#[async_trait]
trait XsdBridgeLauncher: Send + Sync {
    async fn start(
        &self,
        version: &str,
        cache_dir: &std::path::Path,
        timeout_ms: u64,
    ) -> Result<(Option<BridgeProcess>, Channel), ValidatorError>;

    async fn stop(&self, process: Option<BridgeProcess>) {
        if let Some(process) = process {
            let _ = process.stop().await;
        }
    }
}

struct ProcessXsdBridgeLauncher;

#[async_trait]
impl XsdBridgeLauncher for ProcessXsdBridgeLauncher {
    async fn start(
        &self,
        version: &str,
        cache_dir: &std::path::Path,
        timeout_ms: u64,
    ) -> Result<(Option<BridgeProcess>, Channel), ValidatorError> {
        let binary_path = ensure_binary_for_spec(&XML_BRIDGE, version, cache_dir)
            .await
            .map_err(|e| ValidatorError::endpoint(format!("XML bridge binary unavailable: {e}")))?;

        let process_config = BridgeProcessConfig::xml(binary_path, timeout_ms);
        let (process, channel) = BridgeProcess::start_and_connect(&process_config)
            .await
            .map_err(|e| ValidatorError::endpoint(format!("XML bridge start failed: {e}")))?;

        wait_for_health(&channel, Duration::from_secs(10), |ch| {
            let mut client = proto::health_client::HealthClient::new(ch);
            async move {
                let resp = client.check(HealthCheckRequest {}).await?;
                Ok(resp.into_inner().status == "SERVING")
            }
        })
        .await
        .map_err(|e| ValidatorError::endpoint(format!("XML bridge health check failed: {e}")))?;

        Ok((Some(process), channel))
    }
}

#[derive(Debug, Clone)]
struct ReadyBridge {
    channel: Channel,
    generation: usize,
}

#[derive(Clone)]
pub struct XsdBridgeBackend {
    /// Published only after the entire retained schema set is acknowledged.
    /// Cache acknowledgement insertion holds this lock for reading, fencing it
    /// against generation invalidation before the next replay snapshot.
    channel: Arc<RwLock<Option<ReadyBridge>>>,
    generation: Arc<AtomicUsize>,
    /// Full retained snapshots keyed by schema id: root systemId, resource map,
    /// and alias map, so reconnect replay re-supplies the exact same set.
    schemas: Arc<DashMap<SchemaId, Arc<SchemaSet>>>,
    /// Deterministic refusal memo, keyed by `(schema id, bridge generation)`.
    ///
    /// A refusal is a function of the schema bytes against one bridge process,
    /// so it is memoized only while that generation is published. Entries are
    /// cleared in `launch_and_seed` on a generation bump.
    ///
    /// Eviction policy: `refusals` and `schemas` share one budget of
    /// `schema_cache_max_entries` entries. Inserting a new refusal at or over
    /// that combined bound clears the whole refusal cache (a full clear is
    /// acceptable because every refusal is re-attempted on demand at one RPC
    /// each); if the success cache alone already fills the budget, the new
    /// refusal is not cached at all, so every Exchange pays one RPC (the
    /// status-quo non-regression). Overwrites are size-neutral and bypass the
    /// check. Refusal entries always yield to successes: a new success insert
    /// at or over the combined bound clears `refusals` first. Shrinking the
    /// budget below the refusal count clears the memo immediately.
    refusals: Arc<DashMap<(SchemaId, usize), ValidatorError>>,
    schema_cache_max_entries: Arc<AtomicUsize>,
    slot: Arc<XmlBridgeSlot>,
    rpc: Arc<dyn XsdBridgeRpc>,
    launcher: Arc<dyn XsdBridgeLauncher>,
    start_lock: Arc<Mutex<()>>,
    bridge_version: String,
    bridge_cache_dir: std::path::PathBuf,
    bridge_start_timeout_ms: u64,
    observability: Arc<OnceLock<(Arc<dyn RuntimeObservability>, String)>>,
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
            generation: Arc::new(AtomicUsize::new(0)),
            schemas: Arc::new(DashMap::new()),
            refusals: Arc::new(DashMap::new()),
            schema_cache_max_entries: Arc::new(AtomicUsize::new(
                crate::config::DEFAULT_SCHEMA_CACHE_MAX_ENTRIES,
            )),
            slot,
            rpc: Arc::new(GrpcXsdBridgeRpc),
            launcher: Arc::new(ProcessXsdBridgeLauncher),
            start_lock: Arc::new(Mutex::new(())),
            bridge_version: crate::BRIDGE_VERSION.to_string(),
            bridge_cache_dir: default_cache_dir_for_spec(&XML_BRIDGE),
            bridge_start_timeout_ms: 30_000,
            observability: Arc::new(OnceLock::new()),
        }
    }

    #[cfg(test)]
    fn for_test(rpc: Arc<dyn XsdBridgeRpc>, channel: Channel) -> Self {
        let (state_tx, state_rx) = watch::channel(BridgeState::Ready {
            channel: channel.clone(),
        });
        let slot = Arc::new(XmlBridgeSlot {
            state_rx,
            state_tx,
            process: Arc::new(Mutex::new(None)),
        });
        Self {
            channel: Arc::new(RwLock::new(Some(ReadyBridge {
                channel,
                generation: 1,
            }))),
            generation: Arc::new(AtomicUsize::new(1)),
            schemas: Arc::new(DashMap::new()),
            refusals: Arc::new(DashMap::new()),
            schema_cache_max_entries: Arc::new(AtomicUsize::new(
                crate::config::DEFAULT_SCHEMA_CACHE_MAX_ENTRIES,
            )),
            slot,
            rpc,
            launcher: Arc::new(ProcessXsdBridgeLauncher),
            start_lock: Arc::new(Mutex::new(())),
            bridge_version: crate::BRIDGE_VERSION.to_string(),
            bridge_cache_dir: default_cache_dir_for_spec(&XML_BRIDGE),
            bridge_start_timeout_ms: 30_000,
            observability: Arc::new(OnceLock::new()),
        }
    }

    pub fn set_observability(&self, runtime: Arc<dyn RuntimeObservability>, route_id: String) {
        self.observability.set((runtime, route_id)).ok();
    }

    pub fn schema_id_for(xsd_bytes: &[u8]) -> SchemaId {
        let mut hasher = Sha256::new();
        hasher.update(xsd_bytes);
        format!("xsd-{}", hex::encode(hasher.finalize()))
    }

    /// Update the maximum number of entries allowed in the schema cache.
    /// If the cache already exceeds the new limit, it is cleared immediately.
    pub fn set_schema_cache_max_entries(&self, max_entries: usize) {
        if self.schemas.len() > max_entries {
            self.schemas.clear();
        }
        if self.refusals.len() > max_entries {
            self.refusals.clear();
        }
        self.schema_cache_max_entries
            .store(max_entries, Ordering::Relaxed);
    }

    async fn ensure_bridge_ready(&self) -> Result<ReadyBridge, ValidatorError> {
        if let Some(ch) = self.channel.read().await.clone() {
            return Ok(ch);
        }

        let _guard = self.start_lock.lock().await;
        if let Some(ch) = self.channel.read().await.clone() {
            return Ok(ch);
        }

        let _ = self.slot.state_tx.send(BridgeState::Starting);
        self.launch_and_seed().await
    }

    async fn restart_bridge(
        &self,
        failed_generation: usize,
    ) -> Result<ReadyBridge, ValidatorError> {
        let _guard = self.start_lock.lock().await;
        if let Some(ready) = self.channel.read().await.as_ref()
            && ready.generation > failed_generation
        {
            return Ok(ready.clone());
        }

        let _ = self.slot.state_tx.send(BridgeState::Restarting {
            attempt: 0,
            next_at: Instant::now(),
        });
        self.launch_and_seed().await
    }

    /// Caller holds `start_lock`. Invalidate before the cache snapshot so an
    /// acknowledgement either enters that snapshot or is retried on the next
    /// ready generation. Never hold channel/process locks across bridge RPCs.
    async fn launch_and_seed(&self) -> Result<ReadyBridge, ValidatorError> {
        self.channel.write().await.take();
        let old_process = {
            let mut process_guard = self.slot.process.lock().await;
            process_guard.take()
        };
        self.launcher.stop(old_process).await;

        let (process, channel) = match self.start_bridge_process().await {
            Ok(launched) => launched,
            Err(e) => {
                let _ = self
                    .slot
                    .state_tx
                    .send(BridgeState::Degraded(e.to_string()));
                return Err(e);
            }
        };

        if let Err(e) = self.reseed_with_channel(&channel).await {
            self.launcher.stop(process).await;
            let _ = self
                .slot
                .state_tx
                .send(BridgeState::Degraded(e.to_string()));
            return Err(e);
        }
        let ready = ReadyBridge {
            channel: channel.clone(),
            generation: self.generation.fetch_add(1, Ordering::Relaxed) + 1,
        };
        {
            // Lock order: startup, channel, process. Publication is the final
            // step, after all registrations have completed successfully.
            let mut published = self.channel.write().await;
            // A new bridge process may accept what the old one refused, so
            // memoized refusals never survive a generation change.
            // Defense-in-depth alongside the generation-scoped keys and the
            // fenced consult. Clearing under the write guard closes the window
            // where a fenced insert for the retired generation could complete
            // after the clear but before publication: every fenced insert holds
            // the read guard, so it finishes before this write guard is taken.
            self.refusals.clear();
            *self.slot.process.lock().await = process;
            *published = Some(ready.clone());
        }
        let _ = self.slot.state_tx.send(BridgeState::Ready {
            channel: channel.clone(),
        });

        Ok(ready)
    }

    async fn start_bridge_process(
        &self,
    ) -> Result<(Option<BridgeProcess>, Channel), ValidatorError> {
        self.launcher
            .start(
                &self.bridge_version,
                &self.bridge_cache_dir,
                self.bridge_start_timeout_ms,
            )
            .await
    }

    /// Typed transport-retry classification (rc-7owor).
    ///
    /// Restart + full-set replay only for a direct tonic `Unavailable` source.
    /// Other status codes, arbitrary attached causes, non-tonic sources and
    /// source-less domain refusals do not establish connection/process loss.
    /// Diagnostic wording never determines retry policy.
    fn is_transport_retryable(err: &ValidatorError) -> bool {
        matches!(
            err,
            ValidatorError::Transport {
                source: Some(source),
                ..
            } if source.downcast_ref::<tonic::Status>()
                .is_some_and(|status| status.code() == tonic::Code::Unavailable)
        )
    }

    pub async fn shutdown(&self) {
        let mut guard = self.slot.process.lock().await;
        if let Some(p) = guard.take()
            && let Err(e) = p.stop().await
        {
            tracing::warn!("Failed to stop XSD bridge process: {}", e);
        }
    }

    async fn register_with_channel(
        &self,
        ready: &ReadyBridge,
        schema_id: &str,
        set: &SchemaSet,
    ) -> Result<Option<SchemaId>, ValidatorError> {
        let request = RegisterSchemaRequest::new(schema_id.to_string(), set);
        request.check_encoded_size()?;
        let response = self
            .rpc
            .register_schema(ready.channel.clone(), request)
            .await?;

        if let Some(err) = response.error {
            let err = ValidatorError::from_bridge_error(&err);
            // The read guard must stay live across the memo insert: it closes
            // the check-then-insert gap against `launch_and_seed`'s
            // `channel.write().take()`, so a retired generation can never
            // acquire a refusal entry.
            let published = self.channel.read().await;
            let generation_current = published
                .as_ref()
                .is_some_and(|current| current.generation == ready.generation);
            if generation_current && err.is_deterministic_refusal() {
                let key = (schema_id.to_string(), ready.generation);
                if self.refusals.contains_key(&key) {
                    // Overwriting an existing key is size-neutral: bypass the
                    // combined bound.
                    self.refusals.insert(key, err.clone());
                } else {
                    let max_entries = self.schema_cache_max_entries.load(Ordering::Relaxed);
                    if self.schemas.len() + self.refusals.len() >= max_entries {
                        self.refusals.clear();
                    }
                    // A success cache that alone fills the budget leaves no
                    // room for refusals; do not cache, so the status quo (one
                    // RPC per Exchange) is preserved.
                    if self.schemas.len() < max_entries {
                        self.refusals.insert(key, err.clone());
                    }
                }
            }
            return Err(err);
        }

        let published = self.channel.read().await;
        if !published
            .as_ref()
            .is_some_and(|current| current.generation == ready.generation)
        {
            // This acknowledgement belongs to a retired process. It must not
            // enter the retained cache or masquerade as a current local hit.
            return Ok(None);
        }

        // Evict the entire cache if it has reached capacity. A full clear is
        // acceptable because schemas are re-registered on demand and also
        // re-seeded after bridge reconnects.
        let max_entries = self.schema_cache_max_entries.load(Ordering::Relaxed);
        let new_success_key = !self.schemas.contains_key(schema_id);
        if new_success_key && self.schemas.len() + self.refusals.len() >= max_entries {
            // Refusal entries always yield to successes.
            self.refusals.clear();
        }
        if self.schemas.len() >= max_entries && new_success_key {
            self.schemas.clear();
        }

        // Only a bridge-acknowledged set is cached; a local hit never stands in
        // for the RPC acknowledgement.
        self.schemas
            .insert(schema_id.to_string(), Arc::new(set.clone()));
        Ok(Some(schema_id.to_string()))
    }

    /// Re-registers every retained schema set on `channel`, from cache only
    /// (never the filesystem). Single reseed path shared by `restart_bridge`
    /// (awaited for both startup and restart) and the compatibility
    /// `on_reconnect` hook, so the paths cannot drift (rc-bbwfn).
    ///
    /// Returns only after every retained set is acknowledged by the bridge.
    /// The first failure is metered, logged, and returned, so a reseed is
    /// never swallowed into a half-seeded bridge.
    async fn reseed_with_channel(&self, channel: &Channel) -> Result<(), ValidatorError> {
        // Snapshot full (SchemaId, Arc<SchemaSet>) pairs and release the
        // DashMap guards BEFORE any await, so a slow RPC cannot stall
        // writers.
        let schemas: Vec<(SchemaId, Arc<SchemaSet>)> = self
            .schemas
            .iter()
            .map(|entry| (entry.key().clone(), Arc::clone(entry.value())))
            .collect();

        let observability = self
            .observability
            .get()
            .map(|(rt, rid)| (Arc::clone(rt), rid.clone()));

        for (schema_id, set) in schemas {
            let outcome = async {
                let request = RegisterSchemaRequest::new(schema_id.clone(), &set);
                request.check_encoded_size()?;
                let response = self.rpc.register_schema(channel.clone(), request).await?;
                match response.error {
                    // A refusal response is a failed re-registration, not an
                    // acknowledgement: the same contract as
                    // `register_with_channel`.
                    Some(err) => Err(ValidatorError::from_bridge_error(&err)),
                    None => Ok(()),
                }
            }
            .await;
            if let Err(e) = outcome {
                if let Some((ref rt, ref rid)) = observability {
                    rt.metrics()
                        .increment_errors(rid, "e:validator:reconnect-reseed");
                }
                // log-policy: outside-contract
                error!(schema_id = %schema_id, error = %e, "re-seed schema failed after reconnect");
                return Err(e);
            }
        }
        Ok(())
    }
}

impl BridgeReconnectHandler for XsdBridgeBackend {
    fn on_reconnect(&self, channel: &Channel) -> Result<(), BridgeError> {
        let this = self.clone();
        let channel = channel.clone();
        tokio::spawn(async move {
            // One reseed code path with `restart_bridge` (rc-bbwfn). Failures
            // are metered and logged inside `reseed_with_channel`; this hook
            // has no caller to propagate to.
            let _ = this.reseed_with_channel(&channel).await;
        });
        Ok(())
    }
}

#[async_trait]
impl XsdBridge for XsdBridgeBackend {
    async fn register(&self, schema_id: &str, set: &SchemaSet) -> Result<SchemaId, ValidatorError> {
        let mut ready = self.ensure_bridge_ready().await?;
        let mut transport_retried = false;
        loop {
            {
                let published = self.channel.read().await;
                let generation_current = published
                    .as_ref()
                    .is_some_and(|current| current.generation == ready.generation);
                if generation_current && self.schemas.contains_key(schema_id) {
                    return Ok(schema_id.to_string());
                }
                // Same read guard as the local-hit check, so the memo is only
                // consulted while its generation is still published.
                if generation_current
                    && let Some(cached) = self
                        .refusals
                        .get(&(schema_id.to_string(), ready.generation))
                {
                    return Err(cached.clone());
                }
            }
            match self.register_with_channel(&ready, schema_id, set).await {
                Ok(Some(id)) => return Ok(id),
                Ok(None) => ready = self.ensure_bridge_ready().await?,
                Err(e) if !transport_retried && Self::is_transport_retryable(&e) => {
                    transport_retried = true;
                    ready = self.restart_bridge(ready.generation).await?;
                }
                Err(e) => return Err(e),
            }
        }
    }

    async fn validate(&self, schema_id: &str, doc_bytes: Vec<u8>) -> Result<(), ValidatorError> {
        let req = ValidateWithRequest {
            schema_id: schema_id.to_string(),
            document: doc_bytes,
        };
        if req.encoded_len() > XSD_BRIDGE_MAX_REGISTER_REQUEST_SIZE {
            return Err(ValidatorError::Transport {
                message: "VALIDATION_REQUEST_SIZE_CAP: encoded validation exceeds 17 MiB"
                    .to_string(),
                source: None,
            });
        }
        let channel = self.ensure_bridge_ready().await?;

        let response = match self
            .rpc
            .validate_with(channel.channel.clone(), req.clone())
            .await
        {
            Ok(resp) => resp,
            Err(e) if Self::is_transport_retryable(&e) => {
                let restarted = self.restart_bridge(channel.generation).await?;
                self.rpc.validate_with(restarted.channel, req).await?
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
    use std::collections::{BTreeMap, HashMap};
    use std::sync::Mutex as StdMutex;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::Notify;
    use tonic::transport::Endpoint;

    #[derive(Debug)]
    struct MockRpc {
        register_calls: Arc<AtomicUsize>,
        validate_ok: bool,
        captured: Arc<StdMutex<Vec<RegisterSchemaRequest>>>,
        signal: Arc<Notify>,
    }

    impl MockRpc {
        fn new(validate_ok: bool) -> Self {
            Self {
                register_calls: Arc::new(AtomicUsize::new(0)),
                validate_ok,
                captured: Arc::new(StdMutex::new(Vec::new())),
                signal: Arc::new(Notify::new()),
            }
        }
    }

    #[async_trait]
    impl XsdBridgeRpc for MockRpc {
        async fn register_schema(
            &self,
            _channel: Channel,
            request: RegisterSchemaRequest,
        ) -> Result<RegisterSchemaResponse, ValidatorError> {
            self.register_calls.fetch_add(1, Ordering::SeqCst);
            self.captured
                .lock()
                .expect("capture lock")
                .push(request.clone());
            self.signal.notify_one();
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

    fn test_backend(rpc: &Arc<MockRpc>) -> XsdBridgeBackend {
        XsdBridgeBackend::for_test(Arc::clone(rpc) as Arc<dyn XsdBridgeRpc>, lazy_channel())
    }

    /// Deadline-bounded wait for the mock to observe `min` register calls.
    ///
    /// Uses the mock's `Notify` signal instead of a bare sleep, so the test is
    /// deterministic and does not consume the test-sleep ratchet.
    async fn wait_for_register_calls(rpc: &MockRpc, min: usize) {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if rpc.captured.lock().expect("capture lock").len() >= min {
                return;
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            assert!(
                !remaining.is_zero(),
                "timed out waiting for {min} register calls"
            );
            if tokio::time::timeout(remaining, rpc.signal.notified())
                .await
                .is_err()
            {
                assert!(
                    rpc.captured.lock().expect("capture lock").len() >= min,
                    "timed out waiting for {min} register calls"
                );
                return;
            }
        }
    }

    fn sample_set(root_bytes: &[u8]) -> SchemaSet {
        let mut resources: BTreeMap<String, Vec<u8>> = BTreeMap::new();
        resources.insert("root.xsd".to_string(), root_bytes.to_vec());
        SchemaSet {
            root_system_id: "root.xsd".to_string(),
            resources,
            aliases: BTreeMap::new(),
        }
    }

    fn xml_ingress_limit() -> usize {
        include_str!("../../../../bridges/xml/src/main/resources/application.yml")
            .lines()
            .find_map(|line| line.trim().strip_prefix("max-inbound-message-size:"))
            .expect("XML gRPC ingress configuration")
            .trim()
            .parse()
            .expect("numeric ingress limit")
    }

    #[test]
    fn register_request_at_byte_budget_fits_ingress() {
        use crate::schema_set::MAX_SCHEMA_BYTES;

        let prefix = br#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"><!--"#;
        let suffix = b"--></xs:schema>";
        let mut root = prefix.to_vec();
        root.resize(MAX_SCHEMA_BYTES - suffix.len(), b' ');
        root.extend_from_slice(suffix);
        let set = sample_set(&root);
        assert_eq!(set.root_bytes().len(), MAX_SCHEMA_BYTES);
        let request = RegisterSchemaRequest::new(set.cache_key(), &set);
        assert_eq!(request.schema, root, "legacy root must remain populated");
        let encoded = request.encode_to_vec();
        assert_eq!(encoded.len(), request.encoded_len());
        assert!(encoded.len() > 16 * 1024 * 1024);
        assert!(encoded.len() <= XSD_BRIDGE_MAX_REGISTER_REQUEST_SIZE);
        assert!(
            encoded.len() <= xml_ingress_limit(),
            "encoded request {} exceeds configured ingress {}",
            encoded.len(),
            xml_ingress_limit()
        );
    }

    #[test]
    fn xml_ingress_matches_request_limit() {
        assert_eq!(xml_ingress_limit(), XSD_BRIDGE_MAX_REGISTER_REQUEST_SIZE);
        assert_eq!(
            XSD_BRIDGE_MAX_REGISTER_REQUEST_SIZE,
            xsd_bridge_decode_limit()
        );
    }

    mod generations {
        use super::*;
        use std::future::Future;
        use std::pin::Pin;
        use std::task::Poll;

        type Events = Arc<StdMutex<Vec<String>>>;

        struct CountingLauncher {
            launches: AtomicUsize,
            stops: AtomicUsize,
            events: Events,
        }

        #[async_trait]
        impl XsdBridgeLauncher for CountingLauncher {
            async fn start(
                &self,
                _version: &str,
                _cache_dir: &std::path::Path,
                _timeout_ms: u64,
            ) -> Result<(Option<BridgeProcess>, Channel), ValidatorError> {
                let launch = self.launches.fetch_add(1, Ordering::SeqCst) + 1;
                self.events.lock().unwrap().push(format!("launch:{launch}"));
                Ok((None, lazy_channel()))
            }

            async fn stop(&self, process: Option<BridgeProcess>) {
                assert!(process.is_none(), "tests must not launch a real process");
                self.stops.fetch_add(1, Ordering::SeqCst);
                self.events.lock().unwrap().push("stop".to_string());
            }
        }

        struct GatedRpc {
            calls: AtomicUsize,
            gated_calls: Vec<usize>,
            fail_call: usize,
            entered: Notify,
            release: Notify,
            events: Events,
        }

        impl GatedRpc {
            fn new(gated_calls: Vec<usize>, fail_call: usize) -> Arc<Self> {
                Arc::new(Self {
                    calls: AtomicUsize::new(0),
                    gated_calls,
                    fail_call,
                    entered: Notify::new(),
                    release: Notify::new(),
                    events: Arc::new(StdMutex::new(Vec::new())),
                })
            }
        }

        #[async_trait]
        impl XsdBridgeRpc for GatedRpc {
            async fn register_schema(
                &self,
                _channel: Channel,
                request: RegisterSchemaRequest,
            ) -> Result<RegisterSchemaResponse, ValidatorError> {
                let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
                self.events
                    .lock()
                    .unwrap()
                    .push(format!("register:{call}:{}", request.schema_id));
                if self.gated_calls.contains(&call) {
                    self.entered.notify_one();
                    self.release.notified().await;
                }
                if call == self.fail_call {
                    self.events.lock().unwrap().push(format!("fail:{call}"));
                    return Err(ValidatorError::transport_with_source(
                        "test reseed transport failure",
                        tonic::Status::unavailable("test connection lost"),
                    ));
                }
                self.events
                    .lock()
                    .unwrap()
                    .push(format!("ack:{call}:{}", request.schema_id));
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
                unreachable!("generation tests exercise readiness and registration")
            }
        }

        fn fixture(rpc: &Arc<GatedRpc>) -> (XsdBridgeBackend, Arc<CountingLauncher>) {
            let mut backend = XsdBridgeBackend::for_test(rpc.clone(), lazy_channel());
            let launcher = Arc::new(CountingLauncher {
                launches: AtomicUsize::new(0),
                stops: AtomicUsize::new(0),
                events: rpc.events.clone(),
            });
            backend.launcher = launcher.clone();
            (backend, launcher)
        }

        fn retain(backend: &XsdBridgeBackend, id: &str) {
            backend
                .schemas
                .insert(id.to_string(), Arc::new(sample_set(id.as_bytes())));
        }

        async fn poll_once<F: Future>(mut future: Pin<&mut F>) -> Poll<F::Output> {
            std::future::poll_fn(|cx| Poll::Ready(future.as_mut().poll(cx))).await
        }

        mod n1 {
            use super::*;

            #[derive(Clone, Copy, Debug, PartialEq, Eq)]
            enum Operation {
                Register,
                Validate,
            }

            impl Operation {
                async fn run(self, backend: &XsdBridgeBackend) -> Result<(), ValidatorError> {
                    match self {
                        Self::Register => backend
                            .register("xsd-target", &sample_set(b"<target/>"))
                            .await
                            .map(|_| ()),
                        Self::Validate => {
                            backend.validate("xsd-target", b"<target/>".to_vec()).await
                        }
                    }
                }
            }

            struct ScriptedRpc {
                operation: Operation,
                code: tonic::Code,
                failures: usize,
                attempts: AtomicUsize,
                registrations: StdMutex<Vec<RegisterSchemaRequest>>,
                validations: AtomicUsize,
                events: Events,
            }

            impl ScriptedRpc {
                fn target(&self, operation: Operation, id: &str) -> Result<(), ValidatorError> {
                    if operation == self.operation && id == "xsd-target" {
                        let attempt = self.attempts.fetch_add(1, Ordering::SeqCst) + 1;
                        self.events
                            .lock()
                            .unwrap()
                            .push(format!("target:{attempt}"));
                        if attempt <= self.failures {
                            return Err(ValidatorError::transport_with_source(
                                "xml-bridge RPC failed",
                                tonic::Status::new(
                                    self.code,
                                    format!("connection unavailable: attempt {attempt}"),
                                ),
                            ));
                        }
                    }
                    Ok(())
                }
            }

            #[async_trait]
            impl XsdBridgeRpc for ScriptedRpc {
                async fn register_schema(
                    &self,
                    _channel: Channel,
                    request: RegisterSchemaRequest,
                ) -> Result<RegisterSchemaResponse, ValidatorError> {
                    self.registrations.lock().unwrap().push(request.clone());
                    self.target(Operation::Register, &request.schema_id)?;
                    self.events
                        .lock()
                        .unwrap()
                        .push(format!("ack:{}", request.schema_id));
                    Ok(RegisterSchemaResponse {
                        schema_id: request.schema_id,
                        error: None,
                    })
                }

                async fn validate_with(
                    &self,
                    _channel: Channel,
                    request: ValidateWithRequest,
                ) -> Result<ValidateResponse, ValidatorError> {
                    self.validations.fetch_add(1, Ordering::SeqCst);
                    self.target(Operation::Validate, &request.schema_id)?;
                    Ok(ValidateResponse {
                        valid: true,
                        errors: vec![],
                        error: None,
                    })
                }
            }

            fn fixture(
                operation: Operation,
                code: tonic::Code,
                failures: usize,
            ) -> (XsdBridgeBackend, Arc<ScriptedRpc>, Arc<CountingLauncher>) {
                let events = Arc::new(StdMutex::new(vec![]));
                let rpc = Arc::new(ScriptedRpc {
                    operation,
                    code,
                    failures,
                    attempts: AtomicUsize::new(0),
                    registrations: StdMutex::new(vec![]),
                    validations: AtomicUsize::new(0),
                    events: events.clone(),
                });
                let launcher = Arc::new(CountingLauncher {
                    launches: AtomicUsize::new(0),
                    stops: AtomicUsize::new(0),
                    events,
                });
                let mut backend = XsdBridgeBackend::for_test(rpc.clone(), lazy_channel());
                backend.launcher = launcher.clone();
                (backend, rpc, launcher)
            }

            fn assert_size_refusal(error: &ValidatorError) {
                assert!(matches!(
                    error,
                    ValidatorError::Transport { source: None, .. }
                ));
                assert!(!XsdBridgeBackend::is_transport_retryable(error));
                let message = error.to_string();
                assert!(message.len() <= 512);
                assert!(!message.contains("private-schema-id"));
                assert!(!message.contains("PRIVATE_DOCUMENT"));
            }

            fn assert_status(error: &ValidatorError, code: tonic::Code, attempt: usize) {
                let ValidatorError::Transport {
                    source: Some(source),
                    ..
                } = error
                else {
                    panic!("expected typed RPC status: {error:?}");
                };
                let status = source
                    .downcast_ref::<tonic::Status>()
                    .expect("tonic source");
                assert_eq!(status.code(), code);
                assert_eq!(
                    status.message(),
                    format!("connection unavailable: attempt {attempt}")
                );
            }

            #[tokio::test]
            async fn oversized_validation_is_refused_before_readiness_or_rpc() {
                tokio::time::timeout(Duration::from_secs(2), async {
                    let mut outcomes = vec![];
                    for cold in [true, false] {
                        let (backend, rpc, launcher) = fixture(Operation::Validate, tonic::Code::Ok, 0);
                        if cold {
                            backend.channel.write().await.take();
                            backend.slot.state_tx.send(BridgeState::Stopped).unwrap();
                        } else {
                            backend.register("private-schema-id", &sample_set(b"<first/>")).await.unwrap();
                            backend.register("xsd-other", &sample_set(b"<other/>")).await.unwrap();
                            rpc.registrations.lock().unwrap().clear();
                        }
                        let retained: Vec<_> = backend.schemas.iter()
                            .map(|entry| (entry.key().clone(), entry.value().clone())).collect();
                        let mut state = backend.slot.state_rx.clone();
                        state.borrow_and_update();
                        let mut document = vec![b'x'; XSD_BRIDGE_MAX_REGISTER_REQUEST_SIZE + 1];
                        document[..16].copy_from_slice(b"PRIVATE_DOCUMENT");
                        let result = backend.validate("private-schema-id", document).await;
                        let validations = rpc.validations.load(Ordering::SeqCst);
                        let unchanged = !state.has_changed().unwrap()
                            && backend.generation.load(Ordering::SeqCst) == 1
                            && backend.channel.read().await.as_ref().map(|ready| ready.generation)
                                == if cold { None } else { Some(1) }
                            && backend.schemas.len() == retained.len()
                            && retained.iter().all(|(id, set)| Arc::ptr_eq(&backend.schemas.get(id).unwrap(), set));
                        if !cold {
                            backend.validate("xsd-other", b"<other/>".to_vec()).await.unwrap();
                        }
                        outcomes.push((cold, result, validations, unchanged,
                            launcher.launches.load(Ordering::SeqCst),
                            launcher.stops.load(Ordering::SeqCst),
                            rpc.registrations.lock().unwrap().len()));
                    }
                    for (cold, result, validations, unchanged, launches, stops, registrations) in outcomes {
                        assert_size_refusal(&result.expect_err("oversized validation must be refused"));
                        assert_eq!((validations, launches, stops, registrations), (0, 0, 0, 0), "cold={cold}");
                        assert!(unchanged, "refusal must preserve channel, generation, state and cache; cold={cold}");
                    }
                }).await.expect("validation size preflight must finish within deadline");
            }

            #[tokio::test]
            async fn validation_encoded_limit_includes_id_and_overhead() {
                tokio::time::timeout(Duration::from_secs(2), async {
                    let limit = XSD_BRIDGE_MAX_REGISTER_REQUEST_SIZE;
                    assert_eq!(limit, xml_ingress_limit());
                    assert_eq!(limit, xsd_bridge_decode_limit());
                    let (backend, rpc, launcher) = fixture(Operation::Validate, tonic::Code::Ok, 0);
                    for extra in [0, 1] {
                        let mut request = ValidateWithRequest {
                            schema_id: "private-schema-id".to_string(),
                            document: vec![b'x'; limit - 128],
                        };
                        let overhead = request.encoded_len() - request.document.len();
                        request.document.resize(limit - overhead + extra, b'x');
                        assert!(request.document.len() < limit);
                        assert_eq!(request.encoded_len(), limit + extra);
                        let result = backend.validate(&request.schema_id, request.document).await;
                        if extra == 0 {
                            result.expect("exact encoded ceiling is admitted");
                        } else {
                            assert_size_refusal(
                                &result.expect_err("ID and protobuf overhead cross ceiling"),
                            );
                        }
                    }
                    assert_eq!(rpc.validations.load(Ordering::SeqCst), 1);
                    assert_eq!(launcher.launches.load(Ordering::SeqCst), 0);
                    assert_eq!(launcher.stops.load(Ordering::SeqCst), 0);
                })
                .await
                .expect("encoded validation boundary must finish within deadline");
            }

            #[tokio::test]
            async fn limit_statuses_do_not_restart_register_or_validate() {
                tokio::time::timeout(Duration::from_secs(2), async {
                    let mut outcomes = vec![];
                    for operation in [Operation::Register, Operation::Validate] {
                        for code in [
                            tonic::Code::ResourceExhausted,
                            tonic::Code::OutOfRange,
                            tonic::Code::InvalidArgument,
                            tonic::Code::FailedPrecondition,
                        ] {
                            let (backend, rpc, launcher) = fixture(operation, code, 2);
                            retain(&backend, "xsd-other");
                            if operation == Operation::Validate {
                                retain(&backend, "xsd-target");
                            }
                            let mut state = backend.slot.state_rx.clone();
                            state.borrow_and_update();
                            let result = operation.run(&backend).await;
                            backend
                                .validate("xsd-other", b"<other/>".to_vec())
                                .await
                                .unwrap();
                            let registrations = rpc.registrations.lock().unwrap().len();
                            outcomes.push((
                                operation,
                                code,
                                result,
                                rpc.attempts.load(Ordering::SeqCst),
                                launcher.launches.load(Ordering::SeqCst),
                                launcher.stops.load(Ordering::SeqCst),
                                registrations,
                                backend.channel.read().await.as_ref().unwrap().generation,
                                !state.has_changed().unwrap(),
                            ));
                        }
                    }
                    for (
                        operation,
                        code,
                        result,
                        attempts,
                        launches,
                        stops,
                        registrations,
                        generation,
                        unchanged,
                    ) in outcomes
                    {
                        assert_status(&result.expect_err("limit status must surface"), code, 1);
                        assert_eq!(
                            (attempts, launches, stops, generation),
                            (1, 0, 0, 1),
                            "{operation:?} {code:?}"
                        );
                        assert_eq!(registrations, usize::from(operation == Operation::Register));
                        assert!(unchanged);
                    }
                })
                .await
                .expect("limit statuses must finish within deadline");
            }

            #[test]
            fn typed_retry_classifier_only_accepts_unavailable() {
                let mut mismatches = vec![];
                for value in 0..=16 {
                    let code = tonic::Code::from_i32(value);
                    for cause in [false, true] {
                        for message in ["connection unavailable", "document size limit exceeded"] {
                            let mut status = tonic::Status::new(code, message);
                            if cause {
                                status.set_source(Arc::new(std::io::Error::other(
                                    std::io::Error::new(
                                        std::io::ErrorKind::TimedOut,
                                        "h2 cancelled: connection unavailable",
                                    ),
                                )));
                            }
                            let error = ValidatorError::transport_with_source("RPC failed", status);
                            if XsdBridgeBackend::is_transport_retryable(&error)
                                != (code == tonic::Code::Unavailable)
                            {
                                mismatches.push((code, cause, message));
                            }
                        }
                    }
                }
                let non_tonic = ValidatorError::transport_with_source(
                    "connection unavailable",
                    std::io::Error::other(tonic::Status::unavailable(
                        "nested status is not direct source",
                    )),
                );
                let domain = ValidatorError::Transport {
                    message: "connection unavailable".into(),
                    source: None,
                };
                assert!(
                    !XsdBridgeBackend::is_transport_retryable(&non_tonic),
                    "non-tonic source is not process loss"
                );
                assert!(!XsdBridgeBackend::is_transport_retryable(&domain));
                assert!(
                    mismatches.is_empty(),
                    "only Unavailable may restart: {mismatches:?}"
                );
            }

            async fn assert_unavailable_retry(failures: usize) {
                for operation in [Operation::Register, Operation::Validate] {
                    let (backend, rpc, launcher) =
                        fixture(operation, tonic::Code::Unavailable, failures);
                    let mut set = sample_set(b"<root/>");
                    set.resources
                        .insert("child.xsd".into(), b"<child/>".to_vec());
                    set.aliases.insert("urn:child".into(), "child.xsd".into());
                    backend.schemas.insert("xsd-other".into(), Arc::new(set));
                    retain(
                        &backend,
                        if operation == Operation::Validate {
                            "xsd-target"
                        } else {
                            "xsd-cached"
                        },
                    );
                    let retained: Vec<_> = backend
                        .schemas
                        .iter()
                        .map(|entry| RegisterSchemaRequest::new(entry.key().clone(), entry.value()))
                        .collect();
                    let result = operation.run(&backend).await;
                    if failures == 1 {
                        result.unwrap();
                    } else {
                        assert_status(
                            &result.expect_err("second failure must surface"),
                            tonic::Code::Unavailable,
                            2,
                        );
                    }
                    assert_eq!(
                        rpc.attempts.load(Ordering::SeqCst),
                        2,
                        "{operation:?}: no third target attempt"
                    );
                    assert_eq!(launcher.launches.load(Ordering::SeqCst), 1);
                    assert_eq!(launcher.stops.load(Ordering::SeqCst), 1);
                    assert_eq!(backend.generation.load(Ordering::SeqCst), 2);
                    assert_eq!(backend.channel.read().await.as_ref().unwrap().generation, 2);
                    let events = rpc.events.lock().unwrap();
                    assert_eq!(&events[..3], ["target:1", "stop", "launch:1"]);
                    assert_eq!(
                        &events[3..5]
                            .iter()
                            .cloned()
                            .collect::<std::collections::BTreeSet<_>>(),
                        &retained
                            .iter()
                            .map(|request| format!("ack:{}", request.schema_id))
                            .collect()
                    );
                    assert_eq!(events[5], "target:2", "all full-set replays precede retry");
                    let registrations = rpc.registrations.lock().unwrap();
                    let replay = if operation == Operation::Register {
                        &registrations[1..3]
                    } else {
                        &registrations[..]
                    };
                    assert_eq!(
                        registrations.len(),
                        if operation == Operation::Register {
                            4
                        } else {
                            2
                        }
                    );
                    for expected in retained {
                        assert!(replay.contains(&expected), "replay preserves full set");
                    }
                }
            }

            #[tokio::test]
            async fn unavailable_retries_register_and_validate_once() {
                tokio::time::timeout(Duration::from_secs(2), assert_unavailable_retry(1))
                    .await
                    .expect("single retry must finish within deadline");
            }

            #[tokio::test]
            async fn second_unavailable_is_propagated_without_another_restart() {
                tokio::time::timeout(Duration::from_secs(2), assert_unavailable_retry(2))
                    .await
                    .expect("second failure must finish within deadline");
            }
        }

        #[tokio::test]
        async fn oversized_encoded_request_is_non_retryable() {
            tokio::time::timeout(Duration::from_secs(5), async {
                let rpc = GatedRpc::new(vec![], 0);
                let (backend, launcher) = fixture(&rpc);
                let mut set = sample_set(b"<xs:schema/>");
                let long_key = "k".repeat(XSD_BRIDGE_MAX_REGISTER_REQUEST_SIZE);
                set.resources.insert(long_key.clone(), vec![1]);
                assert!(
                    RegisterSchemaRequest::new("xsd-large".to_string(), &set).encoded_len()
                        > XSD_BRIDGE_MAX_REGISTER_REQUEST_SIZE
                );
                let initial = backend.register("xsd-large", &set).await.err();
                let initial_launches = launcher.launches.load(Ordering::SeqCst);
                let initial_stops = launcher.stops.load(Ordering::SeqCst);
                // Seed the retained snapshot directly to exercise restart replay
                // even though initial registration must refuse this request.
                backend
                    .schemas
                    .insert("xsd-large".to_string(), Arc::new(set));
                let replay = backend.restart_bridge(1).await.err();
                assert_eq!(rpc.calls.load(Ordering::SeqCst), 0, "no registration RPCs");
                assert_eq!(initial_launches, 0, "refusal must not restart the bridge");
                assert_eq!(initial_stops, 0);
                assert_eq!(launcher.launches.load(Ordering::SeqCst), 1);
                assert_eq!(launcher.stops.load(Ordering::SeqCst), 2);
                assert!(backend.channel.read().await.is_none());
                for error in [initial, replay] {
                    let error = error.expect("oversized registration must be refused");
                    assert!(matches!(
                        error,
                        ValidatorError::Transport { source: None, .. }
                    ));
                    assert!(!XsdBridgeBackend::is_transport_retryable(&error));
                    let message = error.to_string();
                    assert!(message.len() <= 512);
                    assert!(message.contains("SCHEMA_SET_REQUEST_SIZE_CAP"));
                    assert!(!message.contains(&long_key));
                }
            })
            .await
            .expect("oversized registration and replay must finish within deadline");
        }

        #[tokio::test]
        async fn concurrent_caller_waits_for_restart_reseed() {
            tokio::time::timeout(Duration::from_secs(2), async {
                let rpc = GatedRpc::new(vec![1], 0);
                let (backend, launcher) = fixture(&rpc);
                retain(&backend, "xsd-a");
                retain(&backend, "xsd-b");
                let mut restart = Box::pin(backend.restart_bridge(1));
                assert!(poll_once(restart.as_mut()).await.is_pending());
                rpc.entered.notified().await;

                let mut caller = Box::pin(backend.ensure_bridge_ready());
                assert!(
                    poll_once(caller.as_mut()).await.is_pending(),
                    "readiness must stay pending until every reseed acknowledgement"
                );
                rpc.release.notify_one();
                let (restarted, ready) = tokio::join!(restart, caller);
                assert_eq!(restarted.unwrap().generation, 2);
                assert_eq!(ready.unwrap().generation, 2);
                assert_eq!(launcher.launches.load(Ordering::SeqCst), 1);
                assert_eq!(rpc.calls.load(Ordering::SeqCst), 2);
                assert_eq!(
                    rpc.events
                        .lock()
                        .unwrap()
                        .iter()
                        .filter(|e| e.starts_with("ack:"))
                        .count(),
                    2
                );
            })
            .await
            .expect("restart/readiness must finish within deadline");
        }

        #[tokio::test]
        async fn failed_reseed_unpublishes_and_next_call_reseeds() {
            tokio::time::timeout(Duration::from_secs(2), async {
                let rpc = GatedRpc::new(vec![], 1);
                let (backend, launcher) = fixture(&rpc);
                retain(&backend, "xsd-a");
                retain(&backend, "xsd-b");
                let err = backend
                    .restart_bridge(1)
                    .await
                    .expect_err("first reseed fails");
                assert!(XsdBridgeBackend::is_transport_retryable(&err));
                assert!(matches!(
                    *backend.slot.state_rx.borrow(),
                    BridgeState::Degraded(_)
                ));
                assert!(
                    backend.channel.read().await.is_none(),
                    "failed generation must not remain published"
                );
                assert!(backend.slot.process.lock().await.is_none());
                assert_eq!(
                    launcher.stops.load(Ordering::SeqCst),
                    2,
                    "old and failed replacement must be stopped"
                );
                assert_eq!(
                    backend.schemas.len(),
                    2,
                    "failure must retain the full replay set"
                );

                let ready = backend.ensure_bridge_ready().await.unwrap();
                assert!(ready.generation > 1);
                assert_eq!(
                    backend.channel.read().await.as_ref().unwrap().generation,
                    ready.generation
                );
                assert!(matches!(
                    *backend.slot.state_rx.borrow(),
                    BridgeState::Ready { .. }
                ));
                assert_eq!(launcher.launches.load(Ordering::SeqCst), 2);
                assert_eq!(rpc.calls.load(Ordering::SeqCst), 3);
                let mut acknowledged: Vec<_> = rpc
                    .events
                    .lock()
                    .unwrap()
                    .iter()
                    .filter(|e| e.starts_with("ack:"))
                    .map(|e| e.rsplit(':').next().unwrap().to_string())
                    .collect();
                acknowledged.sort();
                assert_eq!(acknowledged, ["xsd-a", "xsd-b"]);
            })
            .await
            .expect("failed replay and subsequent startup must finish within deadline");
        }

        #[tokio::test]
        async fn concurrent_transport_retries_restart_once() {
            tokio::time::timeout(Duration::from_secs(2), async {
                let rpc = GatedRpc::new(vec![1], 0);
                let (backend, launcher) = fixture(&rpc);
                retain(&backend, "xsd-a");
                let mut first = Box::pin(backend.restart_bridge(1));
                assert!(poll_once(first.as_mut()).await.is_pending());
                rpc.entered.notified().await;
                let mut second = Box::pin(backend.restart_bridge(1));
                assert!(poll_once(second.as_mut()).await.is_pending());
                rpc.release.notify_one();
                let (first, second) = tokio::join!(first, second);
                assert_eq!(
                    launcher.launches.load(Ordering::SeqCst),
                    1,
                    "two failures from one generation need one replacement"
                );
                assert_eq!(first.unwrap().generation, 2);
                assert_eq!(second.unwrap().generation, 2);
                assert_eq!(rpc.calls.load(Ordering::SeqCst), 1);
            })
            .await
            .expect("concurrent restarts must finish within deadline");
        }

        #[tokio::test]
        async fn stale_generation_restart_reuses_current() {
            tokio::time::timeout(Duration::from_secs(2), async {
                let rpc = GatedRpc::new(vec![], 0);
                let (backend, launcher) = fixture(&rpc);
                // Establish generation 2 through the real restart path.
                assert_eq!(backend.restart_bridge(1).await.unwrap().generation, 2);
                launcher.launches.store(0, Ordering::SeqCst);
                launcher.stops.store(0, Ordering::SeqCst);
                rpc.events.lock().unwrap().clear();

                let ready = backend.restart_bridge(1).await.unwrap();
                assert_eq!(
                    launcher.launches.load(Ordering::SeqCst),
                    0,
                    "a late generation-1 failure must reuse generation 2"
                );
                assert_eq!(launcher.stops.load(Ordering::SeqCst), 0);
                assert_eq!(ready.generation, 2);
                assert!(rpc.events.lock().unwrap().is_empty());
            })
            .await
            .expect("stale restart must finish within deadline");
        }

        #[tokio::test]
        async fn stale_generation_ack_is_reregistered() {
            tokio::time::timeout(Duration::from_secs(2), async {
                let rpc = GatedRpc::new(vec![1, 2], 0);
                let (backend, launcher) = fixture(&rpc);
                let set = sample_set(b"<stale/>");
                let mut registration = Box::pin(backend.register("xsd-stale", &set));
                assert!(poll_once(registration.as_mut()).await.is_pending());
                rpc.entered.notified().await;
                assert_eq!(backend.restart_bridge(1).await.unwrap().generation, 2);
                assert!(!backend.schemas.contains_key("xsd-stale"));

                rpc.release.notify_one();
                assert!(
                    poll_once(registration.as_mut()).await.is_pending(),
                    "old acknowledgement must not complete registration on the current generation"
                );
                rpc.entered.notified().await;
                assert!(
                    !backend.schemas.contains_key("xsd-stale"),
                    "old acknowledgement must not become a local hit"
                );
                assert_eq!(
                    *rpc.events.lock().unwrap(),
                    [
                        "register:1:xsd-stale",
                        "stop",
                        "launch:1",
                        "ack:1:xsd-stale",
                        "register:2:xsd-stale",
                    ]
                );
                rpc.release.notify_one();
                assert_eq!(registration.await.unwrap(), "xsd-stale");
                assert_eq!(
                    backend.register("xsd-stale", &set).await.unwrap(),
                    "xsd-stale"
                );
                assert_eq!(
                    rpc.calls.load(Ordering::SeqCst),
                    2,
                    "local reuse is allowed only after the current acknowledgement"
                );
                assert_eq!(launcher.launches.load(Ordering::SeqCst), 1);
                assert_eq!(
                    *rpc.events.lock().unwrap(),
                    [
                        "register:1:xsd-stale",
                        "stop",
                        "launch:1",
                        "ack:1:xsd-stale",
                        "register:2:xsd-stale",
                        "ack:2:xsd-stale",
                    ]
                );
            })
            .await
            .expect("stale acknowledgement recovery must finish within deadline");
        }

        #[tokio::test]
        async fn cached_register_waits_for_successful_generation() {
            tokio::time::timeout(Duration::from_secs(2), async {
                let mut outcomes = Vec::new();
                for failed_reseed in [false, true] {
                    let rpc = if failed_reseed {
                        GatedRpc::new(vec![2], 1)
                    } else {
                        GatedRpc::new(vec![1], 0)
                    };
                    let (backend, launcher) = fixture(&rpc);
                    retain(&backend, "xsd-cached");
                    let set = sample_set(b"xsd-cached");
                    let mut restart = Box::pin(backend.restart_bridge(1));
                    if failed_reseed {
                        let err = restart.as_mut().await.expect_err("first reseed fails");
                        assert!(XsdBridgeBackend::is_transport_retryable(&err));
                        assert!(matches!(
                            *backend.slot.state_rx.borrow(),
                            BridgeState::Degraded(_)
                        ));
                        assert!(backend.channel.read().await.is_none());
                        assert_eq!(launcher.stops.load(Ordering::SeqCst), 2);
                    } else {
                        assert!(poll_once(restart.as_mut()).await.is_pending());
                        rpc.entered.notified().await;
                    }

                    let mut cached = Box::pin(backend.register("xsd-cached", &set));
                    let observed = poll_once(cached.as_mut()).await;
                    let pending = observed.is_pending();
                    if failed_reseed && pending {
                        rpc.entered.notified().await;
                    }
                    let launches = launcher.launches.load(Ordering::SeqCst);
                    let registrations = rpc.calls.load(Ordering::SeqCst);
                    assert!(backend.channel.read().await.is_none());
                    rpc.release.notify_one();

                    if !failed_reseed {
                        assert_eq!(restart.await.unwrap().generation, 2);
                    }
                    let registered = match observed {
                        Poll::Ready(result) => result,
                        Poll::Pending => cached.await,
                    };
                    assert_eq!(registered.unwrap(), "xsd-cached");
                    let current_acknowledged = backend
                        .channel
                        .read()
                        .await
                        .as_ref()
                        .is_some_and(|ready| ready.generation > 1);
                    outcomes.push((
                        failed_reseed,
                        pending,
                        launches,
                        registrations,
                        current_acknowledged,
                    ));
                    assert_eq!(rpc.calls.load(Ordering::SeqCst), registrations);
                }
                // Check both cases together so an early local hit cannot hide
                // the separate recovery failure behind the first assertion.
                assert_eq!(
                    outcomes,
                    [(false, true, 1, 1, true), (true, true, 2, 2, true)],
                    "cached register must wait for reseed and recover after failed reseed: \
                     (failed reseed, pending, launches, registrations, current acknowledged)"
                );
            })
            .await
            .expect("cached registration and recovery must finish within deadline");
        }
    }

    mod refusal_cache {
        use super::*;
        use std::collections::VecDeque;
        use std::future::Future;
        use std::pin::Pin;
        use std::task::Poll;

        /// Scripted bridge outcome. Retryability is source-keyed:
        /// `TransportFailed` is a source-less transport error (non-retryable),
        /// while `TransportUnavailable` carries a tonic `Unavailable` source
        /// (retryable); `Refused` and `Acknowledged` complete the four shapes.
        #[derive(Clone, Debug)]
        enum Outcome {
            Refused(proto::BridgeError),
            TransportFailed,
            TransportUnavailable,
            Acknowledged,
        }

        fn bridge_error(kind: proto::bridge_error::Kind, message: &str) -> proto::BridgeError {
            proto::BridgeError {
                kind: kind as i32,
                message: message.to_string(),
                line: 0,
                column: 0,
                detail: String::new(),
            }
        }

        struct RefusalScriptRpc {
            scripted: StdMutex<HashMap<String, VecDeque<Outcome>>>,
            register_calls: Arc<AtomicUsize>,
            captured: Arc<StdMutex<Vec<String>>>,
            calls: AtomicUsize,
            gated_calls: Vec<usize>,
            entered: Notify,
            release: Notify,
        }

        #[async_trait]
        impl XsdBridgeRpc for RefusalScriptRpc {
            async fn register_schema(
                &self,
                _channel: Channel,
                request: RegisterSchemaRequest,
            ) -> Result<RegisterSchemaResponse, ValidatorError> {
                let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
                if self.gated_calls.contains(&call) {
                    self.entered.notify_one();
                    self.release.notified().await;
                }
                let outcome = self
                    .scripted
                    .lock()
                    .expect("script lock")
                    .get_mut(&request.schema_id)
                    .and_then(VecDeque::pop_front)
                    .unwrap_or(Outcome::Acknowledged);
                self.register_calls.fetch_add(1, Ordering::SeqCst);
                self.captured
                    .lock()
                    .expect("capture lock")
                    .push(request.schema_id.clone());
                match outcome {
                    Outcome::Refused(error) => Ok(RegisterSchemaResponse {
                        schema_id: request.schema_id,
                        error: Some(error),
                    }),
                    Outcome::TransportFailed => Err(ValidatorError::Transport {
                        message: "scripted transport failure".to_string(),
                        source: None,
                    }),
                    Outcome::TransportUnavailable => Err(ValidatorError::transport_with_source(
                        "scripted transport failure",
                        tonic::Status::unavailable("scripted bridge unavailable"),
                    )),
                    Outcome::Acknowledged => Ok(RegisterSchemaResponse {
                        schema_id: request.schema_id,
                        error: None,
                    }),
                }
            }

            async fn validate_with(
                &self,
                _channel: Channel,
                _request: ValidateWithRequest,
            ) -> Result<ValidateResponse, ValidatorError> {
                Ok(ValidateResponse {
                    valid: true,
                    errors: Vec::new(),
                    error: None,
                })
            }
        }

        fn refusal_rpc_with(
            scripted: &[(&str, Outcome)],
            gated_calls: Vec<usize>,
        ) -> Arc<RefusalScriptRpc> {
            let mut queues: HashMap<String, VecDeque<Outcome>> = HashMap::new();
            for (schema_id, outcome) in scripted {
                queues
                    .entry((*schema_id).to_string())
                    .or_default()
                    .push_back(outcome.clone());
            }
            Arc::new(RefusalScriptRpc {
                scripted: StdMutex::new(queues),
                register_calls: Arc::new(AtomicUsize::new(0)),
                captured: Arc::new(StdMutex::new(Vec::new())),
                calls: AtomicUsize::new(0),
                gated_calls,
                entered: Notify::new(),
                release: Notify::new(),
            })
        }

        fn refusal_rpc(scripted: &[(&str, Outcome)]) -> Arc<RefusalScriptRpc> {
            refusal_rpc_with(scripted, Vec::new())
        }

        fn refusal_rpc_gated(
            scripted: &[(&str, Outcome)],
            gated_calls: Vec<usize>,
        ) -> Arc<RefusalScriptRpc> {
            refusal_rpc_with(scripted, gated_calls)
        }

        fn scripted_backend(rpc: Arc<RefusalScriptRpc>) -> XsdBridgeBackend {
            XsdBridgeBackend::for_test(rpc as Arc<dyn XsdBridgeRpc>, lazy_channel())
        }

        struct CountingLauncher {
            launches: AtomicUsize,
            stops: AtomicUsize,
        }

        #[async_trait]
        impl XsdBridgeLauncher for CountingLauncher {
            async fn start(
                &self,
                _version: &str,
                _cache_dir: &std::path::Path,
                _timeout_ms: u64,
            ) -> Result<(Option<BridgeProcess>, Channel), ValidatorError> {
                self.launches.fetch_add(1, Ordering::SeqCst);
                Ok((None, lazy_channel()))
            }

            async fn stop(&self, process: Option<BridgeProcess>) {
                assert!(process.is_none(), "tests must not launch a real process");
                self.stops.fetch_add(1, Ordering::SeqCst);
            }
        }

        fn gated_fixture(rpc: &Arc<RefusalScriptRpc>) -> (XsdBridgeBackend, Arc<CountingLauncher>) {
            let mut backend = XsdBridgeBackend::for_test(rpc.clone(), lazy_channel());
            let launcher = Arc::new(CountingLauncher {
                launches: AtomicUsize::new(0),
                stops: AtomicUsize::new(0),
            });
            backend.launcher = launcher.clone();
            (backend, launcher)
        }

        async fn poll_once<F: Future>(mut future: Pin<&mut F>) -> Poll<F::Output> {
            std::future::poll_fn(|cx| Poll::Ready(future.as_mut().poll(cx))).await
        }

        #[tokio::test]
        async fn first_refusal_costs_one_rpc_subsequent_cost_none() {
            let rpc = refusal_rpc(&[(
                "xsd-a",
                Outcome::Refused(bridge_error(
                    proto::bridge_error::Kind::CompilationFailed,
                    "bad schema",
                )),
            )]);
            let backend = scripted_backend(rpc.clone());
            let set_a = sample_set(b"<xsd:a/>");

            let mut errors = Vec::new();
            for _ in 0..3 {
                errors.push(
                    backend
                        .register("xsd-a", &set_a)
                        .await
                        .expect_err("refusal must surface as an error"),
                );
            }

            assert_eq!(
                rpc.register_calls.load(Ordering::SeqCst),
                1,
                "one refusal RPC covers every subsequent registration"
            );
            for error in &errors {
                match error {
                    ValidatorError::CompilationFailed { message, .. } => {
                        assert_eq!(message, "bad schema");
                    }
                    other => panic!("expected CompilationFailed, got {other:?}"),
                }
            }
            assert!(
                backend.refusals.contains_key(&("xsd-a".to_string(), 1)),
                "the refusal must be memoized under the current generation"
            );
        }

        #[tokio::test]
        async fn transient_transport_error_is_not_cached_and_retries() {
            let rpc = refusal_rpc(&[
                ("xsd-t", Outcome::TransportFailed),
                (
                    "xsd-t",
                    Outcome::Refused(bridge_error(
                        proto::bridge_error::Kind::CompilationFailed,
                        "now refused",
                    )),
                ),
            ]);
            let backend = scripted_backend(rpc.clone());
            let set_t = sample_set(b"<xsd:t/>");

            let first = backend
                .register("xsd-t", &set_t)
                .await
                .expect_err("transport failure must surface");
            assert!(matches!(first, ValidatorError::Transport { .. }));
            assert_eq!(rpc.register_calls.load(Ordering::SeqCst), 1);
            assert!(
                backend.refusals.is_empty(),
                "a transport failure must not be memoized"
            );

            let second = backend
                .register("xsd-t", &set_t)
                .await
                .expect_err("the refusal must surface");
            assert!(matches!(second, ValidatorError::CompilationFailed { .. }));
            assert_eq!(
                rpc.register_calls.load(Ordering::SeqCst),
                2,
                "the second attempt must perform a fresh RPC, not short-circuit"
            );
            assert!(
                backend.refusals.contains_key(&("xsd-t".to_string(), 1)),
                "only the deterministic refusal is memoized"
            );
        }

        #[tokio::test]
        async fn non_refusal_response_error_is_not_memoized() {
            let rpc = refusal_rpc(&[
                (
                    "xsd-i",
                    Outcome::Refused(bridge_error(proto::bridge_error::Kind::Internal, "boom")),
                ),
                (
                    "xsd-i",
                    Outcome::Refused(bridge_error(proto::bridge_error::Kind::Internal, "boom")),
                ),
            ]);
            let backend = scripted_backend(rpc.clone());
            let set_i = sample_set(b"<xsd:i/>");

            for _ in 0..2 {
                let error = backend
                    .register("xsd-i", &set_i)
                    .await
                    .expect_err("INTERNAL must surface as an error");
                assert!(
                    matches!(error, ValidatorError::Transport { source: None, .. }),
                    "INTERNAL maps to a source-less transport error"
                );
            }

            assert_eq!(
                rpc.register_calls.load(Ordering::SeqCst),
                2,
                "every attempt must re-RPC; nothing is memoized"
            );
            assert!(backend.refusals.is_empty());
        }

        #[tokio::test]
        async fn different_schema_ids_do_not_collide() {
            let rpc = refusal_rpc(&[(
                "xsd-a",
                Outcome::Refused(bridge_error(
                    proto::bridge_error::Kind::CompilationFailed,
                    "bad a",
                )),
            )]);
            let backend = scripted_backend(rpc.clone());
            let set_a = sample_set(b"<xsd:a/>");
            let set_b = sample_set(b"<xsd:b/>");

            let refused = backend
                .register("xsd-a", &set_a)
                .await
                .expect_err("xsd-a must be refused");
            assert!(matches!(refused, ValidatorError::CompilationFailed { .. }));
            assert_eq!(
                backend
                    .register("xsd-b", &set_b)
                    .await
                    .expect("xsd-b is acknowledged"),
                "xsd-b"
            );
            let refused_again = backend
                .register("xsd-a", &set_a)
                .await
                .expect_err("xsd-a refusal must be memoized");
            assert!(matches!(
                refused_again,
                ValidatorError::CompilationFailed { .. }
            ));

            let captured = rpc.captured.lock().expect("capture lock").clone();
            assert!(captured.contains(&"xsd-a".to_string()));
            assert!(captured.contains(&"xsd-b".to_string()));
            assert_eq!(
                captured.iter().filter(|id| id.as_str() == "xsd-a").count(),
                1,
                "the third xsd-a registration must consult-hit the memo"
            );
            assert_eq!(
                rpc.register_calls.load(Ordering::SeqCst),
                2,
                "only the first xsd-a and the xsd-b registration may RPC"
            );
            assert!(
                backend.refusals.contains_key(&("xsd-a".to_string(), 1)),
                "xsd-a refusal belongs to the refusal cache"
            );
            assert!(
                !backend.refusals.contains_key(&("xsd-b".to_string(), 1)),
                "an acknowledged set must not enter the refusal cache"
            );
            assert!(backend.schemas.contains_key("xsd-b"));
            assert!(!backend.schemas.contains_key("xsd-a"));
        }

        #[tokio::test]
        async fn stale_generation_refusal_is_not_memoized() {
            tokio::time::timeout(Duration::from_secs(2), async {
                let rpc = refusal_rpc_gated(
                    &[
                        (
                            "xsd-s",
                            Outcome::Refused(bridge_error(
                                proto::bridge_error::Kind::CompilationFailed,
                                "late refusal",
                            )),
                        ),
                        ("xsd-kick", Outcome::TransportUnavailable),
                    ],
                    vec![1],
                );
                let (backend, launcher) = gated_fixture(&rpc);
                let set_s = sample_set(b"<xsd:s/>");
                let set_kick = sample_set(b"<xsd:kick/>");

                let mut registration = Box::pin(backend.register("xsd-s", &set_s));
                assert!(poll_once(registration.as_mut()).await.is_pending());
                rpc.entered.notified().await;

                backend
                    .register("xsd-kick", &set_kick)
                    .await
                    .expect("a retryable kick must restart and acknowledge");
                assert_eq!(
                    backend
                        .channel
                        .read()
                        .await
                        .as_ref()
                        .map(|ready| ready.generation),
                    Some(2),
                    "the kick must publish a new generation"
                );

                rpc.release.notify_one();
                let refusal = registration
                    .await
                    .expect_err("the late refusal must still surface");
                assert!(matches!(refusal, ValidatorError::CompilationFailed { .. }));
                assert!(
                    backend.refusals.is_empty(),
                    "a refusal from a retired generation must not be memoized"
                );

                assert_eq!(
                    backend
                        .register("xsd-s", &set_s)
                        .await
                        .expect("fresh registration on the new generation"),
                    "xsd-s"
                );
                let captured = rpc.captured.lock().expect("capture lock").clone();
                assert_eq!(
                    captured.iter().filter(|id| id.as_str() == "xsd-s").count(),
                    2,
                    "the third xsd-s registration must perform a fresh RPC"
                );
                assert_eq!(launcher.launches.load(Ordering::SeqCst), 1);
                assert_eq!(launcher.stops.load(Ordering::SeqCst), 1);
            })
            .await
            .expect("stale refusal isolation must finish within deadline");
        }

        fn refusal_outcome(message: &str) -> Outcome {
            Outcome::Refused(bridge_error(
                proto::bridge_error::Kind::CompilationFailed,
                message,
            ))
        }

        #[tokio::test]
        async fn restart_clears_negative_cache_and_retries() {
            tokio::time::timeout(Duration::from_secs(2), async {
                let rpc = refusal_rpc(&[
                    ("xsd-r", refusal_outcome("bad")),
                    ("xsd-r", refusal_outcome("still bad")),
                    ("xsd-a", Outcome::TransportUnavailable),
                ]);
                let (backend, launcher) = gated_fixture(&rpc);
                let set_r = sample_set(b"<xsd:r/>");
                let set_a = sample_set(b"<xsd:a/>");

                let first = backend
                    .register("xsd-r", &set_r)
                    .await
                    .expect_err("the generation-1 registration must be refused");
                match &first {
                    ValidatorError::CompilationFailed { message, .. } => assert_eq!(message, "bad"),
                    other => panic!("expected CompilationFailed, got {other:?}"),
                }
                assert!(
                    backend.refusals.contains_key(&("xsd-r".to_string(), 1)),
                    "the generation-1 refusal must be memoized"
                );

                assert_eq!(
                    backend
                        .register("xsd-a", &set_a)
                        .await
                        .expect("a retryable kick must restart and acknowledge"),
                    "xsd-a"
                );
                assert_eq!(
                    backend
                        .channel
                        .read()
                        .await
                        .as_ref()
                        .map(|ready| ready.generation),
                    Some(2),
                    "the retryable failure must publish a new generation"
                );
                assert!(
                    backend.refusals.is_empty(),
                    "a generation change must clear the generation-1 refusal memo"
                );
                assert_eq!(launcher.launches.load(Ordering::SeqCst), 1);

                let second = backend
                    .register("xsd-r", &set_r)
                    .await
                    .expect_err("the generation-2 registration must be refused");
                match &second {
                    ValidatorError::CompilationFailed { message, .. } => {
                        assert_eq!(message, "still bad")
                    }
                    other => panic!("expected CompilationFailed, got {other:?}"),
                }
                assert_eq!(
                    backend.refusals.len(),
                    1,
                    "the restart-cleared memo must hold only the generation-2 refusal"
                );
                assert!(
                    backend.refusals.contains_key(&("xsd-r".to_string(), 2)),
                    "the generation-2 refusal must be memoized"
                );

                let calls_before_memo_hit = rpc.register_calls.load(Ordering::SeqCst);
                let third = backend
                    .register("xsd-r", &set_r)
                    .await
                    .expect_err("the memoized generation-2 refusal must surface");
                match &third {
                    ValidatorError::CompilationFailed { message, .. } => {
                        assert_eq!(message, "still bad")
                    }
                    other => panic!("expected CompilationFailed, got {other:?}"),
                }
                assert_eq!(
                    rpc.register_calls.load(Ordering::SeqCst),
                    calls_before_memo_hit,
                    "the third generation-2 registration must be a memo hit, not an RPC"
                );

                let captured = rpc.captured.lock().expect("capture lock").clone();
                assert_eq!(
                    captured.iter().filter(|id| id.as_str() == "xsd-r").count(),
                    2,
                    "xsd-r must RPC once per generation: the gen-1 memo did not survive the restart"
                );
            })
            .await
            .expect("restart cache clearing must finish within deadline");
        }

        #[tokio::test]
        async fn refusal_insert_full_clears_at_combined_bound() {
            let rpc = refusal_rpc(&[
                ("xsd-a", refusal_outcome("bad a")),
                ("xsd-a", refusal_outcome("bad a")),
                ("xsd-b", refusal_outcome("bad b")),
                ("xsd-b", refusal_outcome("bad b")),
                ("xsd-c", refusal_outcome("bad c")),
            ]);
            let backend = scripted_backend(rpc.clone());
            backend.set_schema_cache_max_entries(2);
            let set_a = sample_set(b"<xsd:a/>");
            let set_b = sample_set(b"<xsd:b/>");
            let set_c = sample_set(b"<xsd:c/>");

            // Explicit walk: a1, b1, c1, a2, c2, b2.
            let walk = [
                ("xsd-a", &set_a),
                ("xsd-b", &set_b),
                ("xsd-c", &set_c),
                ("xsd-a", &set_a),
                ("xsd-c", &set_c),
                ("xsd-b", &set_b),
            ];
            for (step, (id, set)) in walk.into_iter().enumerate() {
                let error = backend
                    .register(id, set)
                    .await
                    .expect_err("every scripted call must be refused");
                assert!(
                    matches!(error, ValidatorError::CompilationFailed { .. }),
                    "step {step} ({id}) must surface CompilationFailed, got {error:?}"
                );
                match step {
                    2 => {
                        assert!(
                            backend.refusals.contains_key(&("xsd-c".to_string(), 1)),
                            "xsd-c must be cached after its refusal"
                        );
                        assert_eq!(
                            backend.refusals.len(),
                            1,
                            "the combined bound clears the cache before caching xsd-c"
                        );
                    }
                    3 => {
                        assert!(backend.refusals.contains_key(&("xsd-c".to_string(), 1)));
                        assert!(backend.refusals.contains_key(&("xsd-a".to_string(), 1)));
                        assert_eq!(
                            backend.refusals.len(),
                            2,
                            "xsd-a joins xsd-c without eviction below the bound"
                        );
                    }
                    5 => {
                        assert!(
                            backend.refusals.contains_key(&("xsd-b".to_string(), 1)),
                            "xsd-b must be cached after its refusal"
                        );
                        assert_eq!(
                            backend.refusals.len(),
                            1,
                            "reaching the combined bound clears the cache before caching xsd-b"
                        );
                    }
                    _ => {}
                }
            }
            assert_eq!(
                rpc.register_calls.load(Ordering::SeqCst),
                5,
                "c2 must consult-hit the memo instead of performing an RPC"
            );
        }

        #[tokio::test]
        async fn success_insert_clears_refusals_at_combined_bound() {
            let rpc = refusal_rpc(&[
                ("xsd-b", refusal_outcome("bad b")),
                ("xsd-b", refusal_outcome("bad b")),
            ]);
            let backend = scripted_backend(rpc.clone());
            backend.set_schema_cache_max_entries(2);
            let set_a = sample_set(b"<xsd:a/>");
            let set_b = sample_set(b"<xsd:b/>");
            let set_c = sample_set(b"<xsd:c/>");

            assert_eq!(
                backend
                    .register("xsd-a", &set_a)
                    .await
                    .expect("xsd-a is acknowledged"),
                "xsd-a"
            );
            let refused = backend
                .register("xsd-b", &set_b)
                .await
                .expect_err("xsd-b must be refused");
            assert!(matches!(refused, ValidatorError::CompilationFailed { .. }));
            assert_eq!(
                backend.schemas.len() + backend.refusals.len(),
                2,
                "xsd-a plus the xsd-b refusal fill the combined budget"
            );
            assert_eq!(
                backend
                    .register("xsd-c", &set_c)
                    .await
                    .expect("xsd-c is acknowledged"),
                "xsd-c"
            );

            let refused_again = backend
                .register("xsd-b", &set_b)
                .await
                .expect_err("xsd-b must be refused again after the memo eviction");
            match (&refused, &refused_again) {
                (
                    ValidatorError::CompilationFailed { message: first, .. },
                    ValidatorError::CompilationFailed {
                        message: second, ..
                    },
                ) => assert_eq!(
                    first, second,
                    "the re-attempt must surface the same refusal"
                ),
                other => panic!("expected CompilationFailed pairs, got {other:?}"),
            }
            assert_eq!(
                backend
                    .register("xsd-c", &set_c)
                    .await
                    .expect("xsd-c remains acknowledged"),
                "xsd-c"
            );

            let captured = rpc.captured.lock().expect("capture lock").clone();
            assert_eq!(
                captured.iter().filter(|id| id.as_str() == "xsd-b").count(),
                2,
                "the post-clear xsd-b register must perform a fresh RPC"
            );
            assert_eq!(
                captured.iter().filter(|id| id.as_str() == "xsd-c").count(),
                1,
                "the second xsd-c register must be a local hit"
            );
            assert!(backend.schemas.contains_key("xsd-a"));
            assert!(backend.schemas.contains_key("xsd-c"));
            assert!(
                !backend.refusals.contains_key(&("xsd-b".to_string(), 1)),
                "the refusal memo must be evicted by the success insert"
            );
        }

        #[tokio::test]
        async fn set_max_entries_shrink_clears_refusals() {
            let rpc = refusal_rpc(&[
                ("xsd-a", refusal_outcome("bad a")),
                ("xsd-b", refusal_outcome("bad b")),
            ]);
            let backend = scripted_backend(rpc.clone());
            let set_a = sample_set(b"<xsd:a/>");
            let set_b = sample_set(b"<xsd:b/>");

            backend
                .register("xsd-a", &set_a)
                .await
                .expect_err("xsd-a must be refused");
            backend
                .register("xsd-b", &set_b)
                .await
                .expect_err("xsd-b must be refused");
            assert_eq!(backend.refusals.len(), 2);

            backend.set_schema_cache_max_entries(1);
            assert!(
                backend.refusals.is_empty(),
                "shrinking below the refusal count clears the memo"
            );

            backend
                .register("xsd-a", &set_a)
                .await
                .expect("xsd-a re-attempts after the shrink");
            backend
                .register("xsd-b", &set_b)
                .await
                .expect("xsd-b re-attempts after the shrink");

            assert_eq!(
                rpc.register_calls.load(Ordering::SeqCst),
                4,
                "both re-attempts must perform fresh RPCs after the shrink"
            );
        }
    }

    #[tokio::test]
    async fn xsd_bridge_reconnect_reseeds() {
        let rpc = Arc::new(MockRpc::new(true));
        let calls = Arc::clone(&rpc.register_calls);
        let backend = test_backend(&rpc);

        let set_a = sample_set(b"<xsd:a/>");
        let set_b = sample_set(b"<xsd:b/>");
        let _id_a = backend.register("xsd-a", &set_a).await.unwrap();
        let _id_b = backend.register("xsd-b", &set_b).await.unwrap();

        backend.on_reconnect(&lazy_channel()).unwrap();
        wait_for_register_calls(&rpc, 4).await;

        assert!(calls.load(Ordering::SeqCst) >= 4);
    }

    #[tokio::test]
    async fn register_request_carries_root_system_id_and_resources() {
        let rpc = Arc::new(MockRpc::new(true));
        let backend = test_backend(&rpc);

        let mut resources: BTreeMap<String, Vec<u8>> = BTreeMap::new();
        resources.insert("root.xsd".to_string(), b"<xs:schema/>".to_vec());
        resources.insert("a/child.xsd".to_string(), b"<xs:schema/>".to_vec());
        let set = SchemaSet {
            root_system_id: "root.xsd".to_string(),
            resources,
            aliases: BTreeMap::new(),
        };

        backend.register("xsd-root", &set).await.unwrap();

        let captured = rpc.captured.lock().unwrap();
        assert_eq!(captured.len(), 1);
        let req = &captured[0];
        assert_eq!(req.schema_id, "xsd-root");
        assert_eq!(req.root_system_id, "root.xsd");
        assert_eq!(req.schema, b"<xs:schema/>".to_vec());
        assert_eq!(req.resources.len(), 2);
        assert_eq!(
            req.resources.get("a/child.xsd").map(Vec::as_slice),
            Some(b"<xs:schema/>".as_slice())
        );
    }

    #[tokio::test]
    async fn register_request_carries_alias_map() {
        let rpc = Arc::new(MockRpc::new(true));
        let backend = test_backend(&rpc);

        let mut resources: BTreeMap<String, Vec<u8>> = BTreeMap::new();
        resources.insert("root.xsd".to_string(), b"<xs:schema/>".to_vec());
        resources.insert("a/parent.xsd".to_string(), b"<xs:schema/>".to_vec());
        // Redundant URI-keyed entry, wire-compat with the normalized path.
        resources.insert(
            "http://example.com/parent.xsd".to_string(),
            b"<xs:schema/>".to_vec(),
        );
        let mut aliases: BTreeMap<String, String> = BTreeMap::new();
        aliases.insert(
            "http://example.com/parent.xsd".to_string(),
            "a/parent.xsd".to_string(),
        );
        let set = SchemaSet {
            root_system_id: "root.xsd".to_string(),
            resources,
            aliases,
        };

        backend.register("xsd-alias", &set).await.unwrap();

        let captured = rpc.captured.lock().unwrap();
        let req = &captured[0];
        assert_eq!(
            req.aliases
                .get("http://example.com/parent.xsd")
                .map(String::as_str),
            Some("a/parent.xsd")
        );
        // The redundant URI-keyed resource still carries its bytes.
        assert!(req.resources.contains_key("http://example.com/parent.xsd"));
    }

    #[tokio::test]
    async fn register_preserves_schema_aliases() {
        let rpc = Arc::new(MockRpc::new(true));
        let backend = test_backend(&rpc);

        // Root bytes are DISTINCT from the child bytes, and the redundant
        // alias-URI resource carries the SAME bytes as its mapped child.
        let mut resources: BTreeMap<String, Vec<u8>> = BTreeMap::new();
        resources.insert("root.xsd".to_string(), b"<root/>".to_vec());
        resources.insert("a/parent.xsd".to_string(), b"<child/>".to_vec());
        resources.insert(
            "http://example.com/parent.xsd".to_string(),
            b"<child/>".to_vec(),
        );
        let mut aliases: BTreeMap<String, String> = BTreeMap::new();
        aliases.insert(
            "http://example.com/parent.xsd".to_string(),
            "a/parent.xsd".to_string(),
        );
        let set = SchemaSet {
            root_system_id: "root.xsd".to_string(),
            resources,
            aliases,
        };

        let expected_resources: HashMap<String, Vec<u8>> = [
            ("root.xsd".to_string(), b"<root/>".to_vec()),
            ("a/parent.xsd".to_string(), b"<child/>".to_vec()),
            (
                "http://example.com/parent.xsd".to_string(),
                b"<child/>".to_vec(),
            ),
        ]
        .into_iter()
        .collect();
        let expected_aliases: HashMap<String, String> = [(
            "http://example.com/parent.xsd".to_string(),
            "a/parent.xsd".to_string(),
        )]
        .into_iter()
        .collect();

        backend.register("xsd-keep", &set).await.unwrap();

        backend.on_reconnect(&lazy_channel()).unwrap();
        wait_for_register_calls(&rpc, 2).await;

        let captured = rpc.captured.lock().unwrap();
        assert_eq!(captured.len(), 2, "exactly one register and one replay");

        // Both the initial registration and the reconnect replay must carry the
        // whole set, not just a count.
        for (label, req) in [("initial", &captured[0]), ("replay", &captured[1])] {
            assert_eq!(req.schema_id, "xsd-keep", "{label}: schema_id");
            assert_eq!(req.root_system_id, "root.xsd", "{label}: root_system_id");
            assert_eq!(
                req.schema,
                b"<root/>".to_vec(),
                "{label}: legacy schema field must carry the root bytes"
            );
            assert_eq!(
                req.resources, expected_resources,
                "{label}: full resources map must be preserved"
            );
            assert_eq!(
                req.aliases, expected_aliases,
                "{label}: full alias map must be preserved"
            );
            assert_eq!(
                req.resources.get("http://example.com/parent.xsd"),
                req.resources.get("a/parent.xsd"),
                "{label}: alias-URI resource must duplicate its mapped child bytes"
            );
        }
    }

    #[tokio::test]
    async fn register_does_not_read_filesystem() {
        let rpc = Arc::new(MockRpc::new(true));
        let backend = test_backend(&rpc);

        // Discover against a real tempdir, then DROP it before registering.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root.xsd");
        std::fs::write(
            &root,
            br#"<?xml version="1.0"?><xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"/>"#,
        )
        .unwrap();
        let set = crate::schema_set::discover_schema_set(&root, &BTreeMap::new()).unwrap();
        drop(dir); // The root on disk is gone; the backend must not touch the FS.

        backend.register("xsd-nofs", &set).await.unwrap();
        backend.on_reconnect(&lazy_channel()).unwrap();
        wait_for_register_calls(&rpc, 2).await;

        let captured = rpc.captured.lock().unwrap();
        assert!(
            captured.len() >= 2,
            "register and replay must succeed without the files on disk"
        );
        for req in captured.iter() {
            assert!(
                req.resources.contains_key(&req.root_system_id),
                "root bytes must be retained from the discovered snapshot"
            );
        }
    }

    /// Writes `root.xsd` importing `a/child.xsd` under `dir` and returns the
    /// root path. `child` bytes are written separately so a caller can mutate
    /// only the child.
    fn write_root_with_child(dir: &std::path::Path, child_bytes: &[u8]) -> std::path::PathBuf {
        let root = dir.join("root.xsd");
        let child = dir.join("a").join("child.xsd");
        std::fs::create_dir_all(child.parent().expect("child parent")).unwrap();
        std::fs::write(
            &root,
            br#"<?xml version="1.0"?><xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"><xs:include schemaLocation="a/child.xsd"/></xs:schema>"#,
        )
        .unwrap();
        std::fs::write(&child, child_bytes).unwrap();
        root
    }

    #[tokio::test]
    async fn changed_child_yields_new_cache_key() {
        let dir = tempfile::tempdir().unwrap();
        let root = write_root_with_child(
            dir.path(),
            br#"<?xml version="1.0"?><xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"><xs:element name="a" type="xs:string"/></xs:schema>"#,
        );

        let no_aliases = BTreeMap::new();
        let set1 = crate::schema_set::discover_schema_set(&root, &no_aliases).unwrap();
        let id1 = set1.cache_key();
        let root_bytes = set1.root_bytes().to_vec();

        // Change ONLY the child's bytes, then rediscover.
        std::fs::write(
            dir.path().join("a").join("child.xsd"),
            br#"<?xml version="1.0"?><xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"><xs:element name="b" type="xs:int"/></xs:schema>"#,
        )
        .unwrap();
        let set2 = crate::schema_set::discover_schema_set(&root, &no_aliases).unwrap();
        let id2 = set2.cache_key();

        assert_ne!(id1, id2, "a changed child must rotate the cache key");
        assert_eq!(
            root_bytes.as_slice(),
            set2.root_bytes(),
            "only the child changed; the legacy root bytes must be unchanged"
        );

        // Both ids are the full set cache keys, not the legacy root-only hash.
        assert_ne!(
            id1,
            XsdBridgeBackend::schema_id_for(&root_bytes),
            "the id must be the full-set cache key, not the root-only helper"
        );

        // Registering both retains two requests keyed by the exact cache key.
        let rpc = Arc::new(MockRpc::new(true));
        let backend = test_backend(&rpc);
        let recorded1 = backend.register(&id1, &set1).await.unwrap();
        let recorded2 = backend.register(&id2, &set2).await.unwrap();
        assert_ne!(recorded1, recorded2);
        {
            let captured = rpc.captured.lock().unwrap();
            assert_eq!(captured.len(), 2, "one request per distinct cache key");
            assert_eq!(captured[0].schema_id, id1);
            assert_eq!(captured[1].schema_id, id2);
        }

        // Alias-map-only change: both alias targets are present with identical
        // bytes, so the resource map is byte-for-byte unchanged; only the alias
        // target mapping differs. The key must still rotate, proving the alias
        // map participates in the digest.
        let mut resources: BTreeMap<String, Vec<u8>> = BTreeMap::new();
        resources.insert("root.xsd".to_string(), b"<root/>".to_vec());
        resources.insert("a/parent.xsd".to_string(), b"<child/>".to_vec());
        resources.insert("b/parent.xsd".to_string(), b"<child/>".to_vec());
        resources.insert(
            "http://example.com/parent.xsd".to_string(),
            b"<child/>".to_vec(),
        );
        let mut aliases_a: BTreeMap<String, String> = BTreeMap::new();
        aliases_a.insert(
            "http://example.com/parent.xsd".to_string(),
            "a/parent.xsd".to_string(),
        );
        let mut aliases_b = aliases_a.clone();
        aliases_b.insert(
            "http://example.com/parent.xsd".to_string(),
            "b/parent.xsd".to_string(),
        );
        let set_alias_a = SchemaSet {
            root_system_id: "root.xsd".to_string(),
            resources: resources.clone(),
            aliases: aliases_a,
        };
        let set_alias_b = SchemaSet {
            root_system_id: "root.xsd".to_string(),
            resources,
            aliases: aliases_b,
        };
        assert_ne!(
            set_alias_a.cache_key(),
            set_alias_b.cache_key(),
            "an alias-mapping change alone must rotate the cache key"
        );
    }

    #[test]
    fn cache_key_is_stable_for_unchanged_set() {
        let mut resources: BTreeMap<String, Vec<u8>> = BTreeMap::new();
        resources.insert("root.xsd".to_string(), b"<root/>".to_vec());
        resources.insert("a/child.xsd".to_string(), b"<child/>".to_vec());
        let mut aliases: BTreeMap<String, String> = BTreeMap::new();
        aliases.insert(
            "http://example.com/child.xsd".to_string(),
            "a/child.xsd".to_string(),
        );
        let set = SchemaSet {
            root_system_id: "root.xsd".to_string(),
            resources,
            aliases,
        };
        assert_eq!(set.cache_key(), set.cache_key(), "key is deterministic");

        // Same content, different insertion order: BTreeMap iteration is sorted,
        // so the key must not depend on insertion order.
        let mut reordered: BTreeMap<String, Vec<u8>> = BTreeMap::new();
        reordered.insert("a/child.xsd".to_string(), b"<child/>".to_vec());
        reordered.insert("root.xsd".to_string(), b"<root/>".to_vec());
        let clone = SchemaSet {
            root_system_id: "root.xsd".to_string(),
            resources: reordered,
            aliases: set.aliases.clone(),
        };
        assert_eq!(
            set.cache_key(),
            clone.cache_key(),
            "insertion order must not affect the key"
        );
    }

    #[tokio::test]
    async fn reconnect_reseeds_every_virtual_path() {
        let dir = tempfile::tempdir().unwrap();
        let root = write_root_with_child(
            dir.path(),
            br#"<?xml version="1.0"?><xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"/>"#,
        );

        let mut aliases: BTreeMap<String, String> = BTreeMap::new();
        aliases.insert(
            "http://example.com/child.xsd".to_string(),
            "a/child.xsd".to_string(),
        );
        let set = crate::schema_set::discover_schema_set(&root, &aliases).unwrap();

        // Two real relative entries (root + child) plus the redundant
        // alias-URI entry: three virtual keys.
        assert_eq!(
            set.resources.len(),
            3,
            "expected root, child, and alias-URI keys, got {:?}",
            set.resources.keys().collect::<Vec<_>>()
        );
        let expected_resources: HashMap<String, Vec<u8>> = set
            .resources
            .iter()
            .map(|(key, bytes)| (key.clone(), bytes.clone()))
            .collect();
        let expected_aliases: HashMap<String, String> = set
            .aliases
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        let root_system_id = set.root_system_id.clone();
        let root_bytes = set.root_bytes().to_vec();
        let schema_id = set.cache_key();

        let rpc = Arc::new(MockRpc::new(true));
        let backend = test_backend(&rpc);
        backend.register(&schema_id, &set).await.unwrap();

        // Drop the files: replay must come from the retained snapshot, not the FS.
        drop(dir);
        backend.on_reconnect(&lazy_channel()).unwrap();
        wait_for_register_calls(&rpc, 2).await;

        let captured = rpc.captured.lock().unwrap();
        assert_eq!(captured.len(), 2, "exactly one register and one replay");
        let replay = &captured[1];
        assert_eq!(replay.schema_id, schema_id, "schema id");
        assert_eq!(replay.root_system_id, root_system_id, "root systemId");
        assert_eq!(
            replay.schema, root_bytes,
            "legacy schema field must carry the root bytes"
        );
        assert_eq!(
            replay.resources, expected_resources,
            "replay must carry every virtual path, not just a count"
        );
        assert_eq!(
            replay.aliases, expected_aliases,
            "replay must carry the alias map"
        );
    }

    #[tokio::test]
    async fn root_without_imports_registers_one_entry() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root.xsd");
        std::fs::write(
            &root,
            br#"<?xml version="1.0"?><xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"/>"#,
        )
        .unwrap();

        let set = crate::schema_set::discover_schema_set(&root, &BTreeMap::new()).unwrap();
        assert_eq!(set.resources.len(), 1, "a single file is a one-entry set");
        assert_eq!(set.root_system_id, "root.xsd");
        assert!(set.aliases.is_empty());

        let rpc = Arc::new(MockRpc::new(true));
        let backend = test_backend(&rpc);
        let schema_id = set.cache_key();
        backend.register(&schema_id, &set).await.unwrap();

        let captured = rpc.captured.lock().unwrap();
        assert_eq!(captured.len(), 1);
        let req = &captured[0];
        assert_eq!(req.schema_id, schema_id);
        assert_eq!(req.root_system_id, "root.xsd");
        assert_eq!(req.resources.len(), 1);
        assert_eq!(
            req.resources.get("root.xsd").map(Vec::as_slice),
            Some(set.root_bytes()),
            "the one resource must be the root's bytes"
        );
        assert!(req.aliases.is_empty());
        // Legacy wire-compat field: the root entry bytes.
        assert_eq!(
            req.schema,
            set.root_bytes().to_vec(),
            "legacy schema field must stay the root bytes"
        );
    }

    // ── transport-retry classification (rc-7owor) ───────────────────────────

    /// A bridge transport failure carries the tonic RPC status in `source`.
    /// That is the error class that must trigger restart + full-set replay, so
    /// it must classify retryable even though its Display string never shows
    /// the tonic code.
    #[test]
    fn transport_error_with_tonic_source_classifies_retryable() {
        let err = ValidatorError::transport_with_source(
            "xml-bridge validate_with RPC failed",
            tonic::Status::unavailable("tcp connect error: ConnectionRefused"),
        );
        assert!(
            XsdBridgeBackend::is_transport_retryable(&err),
            "a Transport wrapping a tonic Status must classify retryable: {err}"
        );
    }

    /// A source-less `Transport` is a domain refusal mapped by
    /// `ValidatorError::from_bridge_error`, not a transport failure. It must
    /// never classify retryable — even when its wording would collide with a
    /// string heuristic — so a domain error never triggers restart/replay.
    #[test]
    fn sourceless_domain_transport_is_not_retryable() {
        let domain = crate::proto::BridgeError {
            kind: crate::proto::bridge_error::Kind::Internal as i32,
            message: "upstream transport unavailable".to_string(),
            line: 0,
            column: 0,
            detail: String::new(),
        };
        let err = ValidatorError::from_bridge_error(&domain);
        assert!(
            !XsdBridgeBackend::is_transport_retryable(&err),
            "a source-less domain Transport must not classify retryable: {err}"
        );
    }

    // ── reseed ordering (rc-bbwfn) ──────────────────────────────────────────

    /// Register RPC that always fails at the transport level, for reseed
    /// failure paths.
    struct RejectingRpc;

    #[async_trait]
    impl XsdBridgeRpc for RejectingRpc {
        async fn register_schema(
            &self,
            _channel: Channel,
            _request: RegisterSchemaRequest,
        ) -> Result<RegisterSchemaResponse, ValidatorError> {
            Err(ValidatorError::transport_with_source(
                "xml-bridge register_schema RPC failed",
                tonic::Status::unavailable("connection refused"),
            ))
        }

        async fn validate_with(
            &self,
            _channel: Channel,
            _request: ValidateWithRequest,
        ) -> Result<ValidateResponse, ValidatorError> {
            Ok(ValidateResponse {
                valid: true,
                errors: Vec::new(),
                error: None,
            })
        }
    }

    /// The shared reseed must complete every retained registration BEFORE it
    /// returns, so `restart_bridge` hands the channel back only over a fully
    /// seeded bridge and the retried RPC can never observe `unknown
    /// schema_id` (rc-bbwfn). A spawn-based reseed would return before the
    /// mock observes any registration and fail this assertion.
    #[tokio::test]
    async fn reseed_completes_all_registrations_before_returning() {
        let rpc = Arc::new(MockRpc::new(true));
        let backend = test_backend(&rpc);

        let set_a = sample_set(b"<xsd:a/>");
        let set_b = sample_set(b"<xsd:b/>");
        let _id_a = backend.register("xsd-a", &set_a).await.unwrap();
        let _id_b = backend.register("xsd-b", &set_b).await.unwrap();
        assert_eq!(rpc.captured.lock().expect("capture lock").len(), 2);

        backend.reseed_with_channel(&lazy_channel()).await.unwrap();

        let captured = rpc.captured.lock().expect("capture lock");
        assert_eq!(
            captured.len(),
            4,
            "both retained sets must be re-registered before the reseed returns"
        );
        let mut replayed: Vec<&str> = captured[2..]
            .iter()
            .map(|req| req.schema_id.as_str())
            .collect();
        replayed.sort_unstable();
        assert_eq!(
            replayed,
            ["xsd-a", "xsd-b"],
            "the replay must carry every retained schema id"
        );
    }

    /// A reseed failure must surface as an error (rc-bbwfn) — never be
    /// swallowed into a half-seeded bridge — and keep its transport
    /// classification so the existing retry behavior still applies.
    #[tokio::test]
    async fn reseed_failure_surfaces_as_error() {
        let rpc = Arc::new(RejectingRpc);
        let backend =
            XsdBridgeBackend::for_test(Arc::clone(&rpc) as Arc<dyn XsdBridgeRpc>, lazy_channel());
        // A retained set, as left behind by a prior acknowledged registration.
        backend
            .schemas
            .insert("xsd-a".to_string(), Arc::new(sample_set(b"<xsd:a/>")));

        let err = backend
            .reseed_with_channel(&lazy_channel())
            .await
            .expect_err("a failed re-registration must surface as an error");
        assert!(
            XsdBridgeBackend::is_transport_retryable(&err),
            "the surfaced error keeps its transport classification: {err}"
        );
    }
}
