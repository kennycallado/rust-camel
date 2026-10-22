use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use camel_component_api::{
    Body, CamelError, ConcurrencyModel, Consumer, ConsumerContext, ConsumerStartupMode, Exchange,
    Message, Value,
};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::sync::CancellationToken;
use tonic::transport::Channel;
use tracing::{error, info, warn};

use crate::pool::{BridgeState, ConsumerProfileGuard, CxfBridgePool, cxf_bridge_client};
use crate::proto::{ConsumerRequest, ConsumerResponse};

/// gRPC metadata key carrying the consumer's profile on `OpenConsumerStream`.
/// Read by the bridge handler `CxfBridgeService.openConsumerStream`.
pub const CXF_PROFILE_METADATA_KEY: &str = "x-cxf-profile";

/// Bridge-owned Cxf* message headers. An inbound HTTP header with any of
/// these names (case-insensitive) is dropped so a client cannot shadow the
/// values the bridge derived from the request.
const RESERVED_CXF_HTTP_HEADERS: [&str; 4] = [
    "cxfrequestid",
    "cxfoperation",
    "cxfsoapaction",
    "cxfsecurityprofile",
];

pub struct CxfConsumer {
    pool: Arc<CxfBridgePool>,
    /// The profile this consumer serves; sent as `x-cxf-profile` metadata on
    /// every `OpenConsumerStream` so the bridge registers the stream under
    /// this profile and dispatches to it only requests addressed to this
    /// profile's path; also the key of the duplicate-consumer registration.
    profile_name: String,
    cancel_token: Option<CancellationToken>,
    task_handle: Option<JoinHandle<Result<(), CamelError>>>,
    /// Registration of `profile_name` in the pool. Owned by the consumer
    /// struct, never by the spawned task, so a late exit of an old task
    /// cannot release a newer registration. Dropping the consumer (the
    /// runtime abort path in `camel-core` `consumer_management.rs`, which
    /// aborts the outer task after the stop budget without calling `stop()`)
    /// releases it through `Drop`.
    profile_guard: Option<ConsumerProfileGuard>,
    /// Observability handle: `metrics()` powers the retained
    /// `b-prime:cxf:response-marshalling` increment and
    /// `component_metrics()` the uniform `cxf:consume` emission
    /// (dashboard-observability 4.2).
    runtime: Arc<dyn camel_component_api::RuntimeObservability>,
}

impl CxfConsumer {
    pub fn new(
        pool: Arc<CxfBridgePool>,
        profile_name: String,
        runtime: Arc<dyn camel_component_api::RuntimeObservability>,
    ) -> Self {
        Self {
            pool,
            profile_name,
            cancel_token: None,
            task_handle: None,
            profile_guard: None,
            runtime,
        }
    }
}

fn build_exchange(req: &ConsumerRequest) -> Result<Exchange, CamelError> {
    // Validate the optional signer FIRST: the whole descriptor passes or
    // nothing is written (atomic metadata, no partial exchange on failure).
    let signer = match &req.signer {
        Some(raw) => Some(crate::signer::validate_signer(raw)?),
        None => None,
    };

    let body = if req.payload.is_empty() {
        Body::Empty
    } else {
        match std::str::from_utf8(&req.payload) {
            Ok(s) => Body::Text(s.to_string()),
            Err(_) => Body::Bytes(bytes::Bytes::from(req.payload.clone())),
        }
    };

    let mut exchange = Exchange::new(Message::new(body));

    if !req.request_id.is_empty() {
        exchange
            .input
            .set_header("CxfRequestId", Value::String(req.request_id.clone()));
    }
    if !req.operation.is_empty() {
        exchange
            .input
            .set_header("CxfOperation", Value::String(req.operation.clone()));
    }
    if !req.soap_action.is_empty() {
        exchange
            .input
            .set_header("CxfSoapAction", Value::String(req.soap_action.clone()));
    }
    if !req.security_profile.is_empty() {
        exchange.input.set_header(
            "CxfSecurityProfile",
            Value::String(req.security_profile.clone()),
        );
    }
    // HTTP headers are mapped before the SOAP-header field so a client key
    // can never overwrite a bridge-owned value. Only the dedicated
    // `soap_headers` proto field may produce `CxfSoapHeader*` keys.
    for (k, v) in &req.headers {
        let lower = k.to_ascii_lowercase();
        if lower.starts_with("cxfsoapheader") || RESERVED_CXF_HTTP_HEADERS.contains(&lower.as_str())
        {
            continue;
        }
        exchange.input.set_header(k, Value::String(v.clone()));
    }

    for h in &req.soap_headers {
        if h.local_name.is_empty() {
            continue;
        }
        exchange.input.set_header(
            format!("CxfSoapHeader.{}", h.local_name),
            Value::String(h.xml.clone()),
        );
        exchange.input.set_header(
            format!("CxfSoapHeaderSigned.{}", h.local_name),
            Value::String(if h.signed { "true" } else { "false" }.to_string()),
        );
    }

    // Signer metadata lands as the four dedicated exchange properties ONLY.
    // Never the principal-property helper, never an authenticated identity,
    // never any auth-scoped property key (ADR-0032 / design D). Request
    // headers stay message headers and cannot smuggle these properties in.
    if let Some(validated) = signer {
        exchange.set_property(
            crate::signer::SIGNER_SUBJECT,
            Value::String(validated.subject),
        );
        exchange.set_property(
            crate::signer::SIGNER_ISSUER,
            Value::String(validated.issuer),
        );
        exchange.set_property(
            crate::signer::SIGNER_SERIAL,
            Value::String(validated.serial),
        );
        exchange.set_property(
            crate::signer::SIGNER_FINGERPRINT,
            Value::String(validated.fingerprint_sha256),
        );
    }

    Ok(exchange)
}

fn build_response_body(exchange: &Exchange) -> Result<Vec<u8>, CamelError> {
    let body = exchange
        .output
        .as_ref()
        .map(|m| &m.body)
        .unwrap_or(&exchange.input.body);
    match body {
        Body::Text(s) => Ok(s.as_bytes().to_vec()),
        Body::Xml(s) => Ok(s.as_bytes().to_vec()),
        Body::Bytes(b) => Ok(b.to_vec()),
        Body::Json(v) => serde_json::to_vec(v).map_err(|e| {
            CamelError::ProcessorError(format!("JSON serialization error in CXF response: {e}"))
        }),
        Body::Empty => Ok(vec![]),
        Body::Stream(_) => Err(CamelError::ProcessorError(
            "streaming body not supported by CXF component".into(),
        )),
        _ => Err(CamelError::ProcessorError(
            "unsupported body type for CXF component".into(),
        )),
    }
}

fn is_bridge_transport_error(err: &CamelError) -> bool {
    let msg = err.to_string().to_lowercase();
    msg.contains("transport") || msg.contains("connection") || msg.contains("unavailable")
}

async fn await_ready_channel(pool: Arc<CxfBridgePool>) -> Result<Channel, CamelError> {
    let key = CxfBridgePool::slot_key();
    let slot = pool
        .get_or_create_slot(&key)
        .await
        .map_err(|e| CamelError::ProcessorError(format!("CXF slot error: {e}")))?;
    let mut rx = slot.state_rx.clone();

    loop {
        match &*rx.borrow() {
            BridgeState::Ready { channel } => return Ok(channel.clone()),
            BridgeState::Stopped => {
                return Err(CamelError::ProcessorError(format!(
                    "CXF bridge '{}' is stopped",
                    slot.key
                )));
            }
            _ => {}
        }

        if rx.changed().await.is_err() {
            return Err(CamelError::ProcessorError(format!(
                "CXF bridge '{}' state channel closed",
                slot.key
            )));
        }
    }
}

#[async_trait]
impl Consumer for CxfConsumer {
    async fn start(&mut self, ctx: ConsumerContext) -> Result<(), CamelError> {
        if let Some(mut stale) = self.profile_guard.take() {
            stale.release();
        }
        // Local until the end: every early return drops it and releases.
        let guard = self.pool.register_consumer_profile(&self.profile_name)?;
        let profile_header: tonic::metadata::AsciiMetadataValue =
            self.profile_name.parse().map_err(|e| {
                CamelError::Config(format!(
                    "cxf consumer profile '{}' is not a valid gRPC metadata value: {e}",
                    self.profile_name
                ))
            })?;
        let pool = Arc::clone(&self.pool);
        let cancel = ctx.cancel_token();
        self.cancel_token = Some(cancel.clone());

        // Eagerly create/probe the bridge slot BEFORE spawning the consumer
        // task so a missing bridge binary or an aborted bridge process is
        // reported as a startup error instead of hanging the route controller.
        // Mirrors the JMS consumer's pre-flight (camel-jms/src/consumer.rs).
        {
            let key = CxfBridgePool::slot_key();
            let slot = pool
                .get_or_create_slot(&key)
                .await
                .map_err(|e| CamelError::ProcessorError(format!("CXF slot error: {e}")))?;
            match &*slot.state_rx.borrow() {
                BridgeState::Ready { .. } | BridgeState::Starting => {} // bridge up or coming up
                BridgeState::Degraded(reason) => {
                    return Err(CamelError::ProcessorError(format!(
                        "CXF bridge not available: {reason}"
                    )));
                }
                other => {
                    return Err(CamelError::ProcessorError(format!(
                        "CXF bridge not available: {other:?}"
                    )));
                }
            }
        }

        let runtime = Arc::clone(&self.runtime);
        let ctx_task = ctx.clone();
        let profile_name = self.profile_name.clone();
        let handle: JoinHandle<Result<(), CamelError>> = tokio::spawn(async move {
            let ctx = ctx_task;
            let mut consecutive_transport_failures: u32 = 0;
            let mut reconnect_attempt: u32 = 0;
            // Manual retry loop (not retry_async) because:
            // - Retries are embedded inside `tokio::select!` with cancellation
            //   tokens (ctx.cancelled(), cancel.cancelled()); retry_async's
            //   tight loop cannot interleave cancellation checks between
            //   delay and retry.
            // - Inter-attempt side effects (channel refresh via
            //   pool.refresh_slot_channel, transport failure counting,
            //   reconnect_attempt reset on success) must run between retries.
            loop {
                let channel = tokio::select! {
                    _ = cancel.cancelled() => {
                        info!("CXF consumer cancelled");
                        break;
                    }
                    _ = ctx.cancelled() => {
                        info!("CXF consumer context cancelled");
                        break;
                    }
                    result = await_ready_channel(Arc::clone(&pool)) => {
                        match result {
                            Ok(channel) => channel,
                            Err(e) => {
                                warn!(error = %e, "CXF consumer waiting for ready bridge failed");
                                if pool.reconnect.should_retry(reconnect_attempt + 1) {
                                    let delay = pool.reconnect.delay_for(reconnect_attempt);
                                    reconnect_attempt = reconnect_attempt.saturating_add(1);
                                    tokio::select! {
                                        _ = cancel.cancelled() => break,
                                        _ = ctx.cancelled() => break,
                                        _ = tokio::time::sleep(delay) => {}
                                    }
                                    continue;
                                }
                                warn!("CXF consumer reconnect attempts exhausted");
                                break;
                            }
                        }
                    }
                };

                let mut client = cxf_bridge_client(channel);
                let (response_tx, response_rx) = mpsc::channel::<ConsumerResponse>(32);
                let response_stream = ReceiverStream::new(response_rx);

                let mut open_request = tonic::Request::new(response_stream);
                open_request
                    .metadata_mut()
                    .insert(CXF_PROFILE_METADATA_KEY, profile_header.clone());
                let stream_result = client.open_consumer_stream(open_request).await;

                let mut stream = match stream_result {
                    Ok(resp) => {
                        consecutive_transport_failures = 0;
                        reconnect_attempt = 0;
                        // Readiness is signalled from start() (see ctx.mark_ready()
                        // after spawn), NOT here: open_consumer_stream is a
                        // bidirectional-streaming RPC whose `.await` only resolves
                        // once the bridge flushes response headers, which it defers
                        // until the first inbound SOAP request is dispatched. Gating
                        // route startup on this would deadlock (no request can arrive
                        // until the route is started).
                        info!("CXF consumer stream opened successfully");
                        resp.into_inner()
                    }
                    Err(e) => {
                        if e.code() == tonic::Code::InvalidArgument {
                            // log-policy: system-broken
                            error!(profile = %profile_name, "CXF bridge rejected consumer profile with INVALID_ARGUMENT; consumer stops without reconnecting: {}", e.message());
                            return Ok(());
                        }
                        let err = CamelError::ProcessorError(format!("CXF gRPC stream error: {e}"));
                        if is_bridge_transport_error(&err) {
                            consecutive_transport_failures += 1;
                            if consecutive_transport_failures >= 2 {
                                warn!(
                                    failures = consecutive_transport_failures,
                                    "CXF stream transport failures exceeded threshold; refreshing channel"
                                );
                                let key = CxfBridgePool::slot_key();
                                if let Err(refresh_err) = pool.refresh_slot_channel(&key).await {
                                    warn!(error = %refresh_err, "CXF channel refresh failed; requesting bridge restart");
                                    pool.restart_slot(&key);
                                }
                                consecutive_transport_failures = 0;
                            }
                        } else {
                            consecutive_transport_failures = 0;
                        }
                        warn!(error = %e, "CXF consumer stream open failed; retrying");
                        if pool.reconnect.should_retry(reconnect_attempt + 1) {
                            let delay = pool.reconnect.delay_for(reconnect_attempt);
                            reconnect_attempt = reconnect_attempt.saturating_add(1);
                            tokio::select! {
                                _ = cancel.cancelled() => break,
                                _ = ctx.cancelled() => break,
                                _ = tokio::time::sleep(delay) => {}
                            }
                            continue;
                        }
                        warn!("CXF consumer reconnect attempts exhausted");
                        break;
                    }
                };

                loop {
                    tokio::select! {
                        _ = cancel.cancelled() => {
                            info!("CXF consumer cancelled");
                            return Ok(());
                        }
                        _ = ctx.cancelled() => {
                            info!("CXF consumer context cancelled");
                            return Ok(());
                        }
                        msg = stream.message() => {
                            match msg {
                                Ok(Some(req)) => {
                                    let request_id = req.request_id.clone();
                                    let security_profile = req.security_profile.clone();

                                    // Assembly can reject the request (invalid signer
                                    // descriptor). The route is then never dispatched and
                                    // the caller gets a correlated non-WSS Receiver fault
                                    // (`soap:Server` → HTTP 500 on the publisher); the
                                    // profile's WSS-only `fault_status` override cannot
                                    // rewrite it.
                                    let response = match build_exchange(&req) {
                                        Ok(exchange) => {
                                            let result = ctx.send_and_wait(exchange).await;

                                            match result {
                                                Ok(resp_exchange) => {
                                                    match build_response_body(&resp_exchange) {
                                                        Ok(payload) => ConsumerResponse {
                                                            request_id,
                                                            payload,
                                                            fault: false,
                                                            fault_code: String::new(),
                                                            fault_string: String::new(),
                                                            security_profile,
                                                        },
                                                        Err(e) => {
                                                            runtime.metrics().increment_errors(ctx.route_id(), "b-prime:cxf:response-marshalling");
                                                            // log-policy: outside-contract
                                                            error!("CXF consumer response body error: {e}");
                                                            ConsumerResponse {
                                                                request_id,
                                                                payload: vec![],
                                                                fault: true,
                                                                fault_code: "soap:Server".to_string(),
                                                                fault_string: e.to_string(),
                                                                security_profile,
                                                            }
                                                        }
                                                    }
                                                }
                                                Err(e) => {
                                                    // log-policy: handler-owned
                                                    warn!("CXF consumer route error: {e}");
                                                    ConsumerResponse {
                                                        request_id,
                                                        payload: vec![],
                                                        fault: true,
                                                        fault_code: "soap:Server".to_string(),
                                                        fault_string: e.to_string(),
                                                        security_profile,
                                                    }
                                                }
                                            }
                                        }
                                        Err(e) => {
                                            // log-policy: handler-owned
                                            warn!("CXF consumer rejected request before dispatch: {e}");
                                            ConsumerResponse {
                                                request_id,
                                                payload: vec![],
                                                fault: true,
                                                fault_code: "soap:Server".to_string(),
                                                fault_string: e.to_string(),
                                                security_profile,
                                            }
                                        }
                                    };

                                    // Uniform `cxf:consume` emission at the
                                    // consumer entrypoint: `fault` is the
                                    // honest failure signal — a pipeline
                                    // rejection OR a marshalling failure
                                    // (which also lands on the retained
                                    // b-prime label; the two error labels
                                    // differ, and whole-family dashboard
                                    // sums count that failure twice —
                                    // intended per design D5).
                                    runtime
                                        .component_metrics()
                                        .observe("cxf", "consume", response.fault);

                                    if let Err(send_err) = response_tx.send(response).await {
                                        warn!("Failed to send CXF consumer response: {send_err}");
                                    }
                                }
                                Ok(None) => {
                                    info!("CXF consumer stream ended; reconnecting");
                                    break;
                                }
                                Err(e) => {
                                    let err = CamelError::ProcessorError(format!("CXF gRPC stream error: {e}"));
                                    if is_bridge_transport_error(&err) {
                                        consecutive_transport_failures += 1;
                                        if consecutive_transport_failures >= 2 {
                                            warn!(
                                                failures = consecutive_transport_failures,
                                                "CXF stream transport failures exceeded threshold; refreshing channel"
                                            );
                                            let key = CxfBridgePool::slot_key();
                                            if let Err(refresh_err) = pool.refresh_slot_channel(&key).await {
                                                warn!(error = %refresh_err, "CXF channel refresh failed; requesting bridge restart");
                                                pool.restart_slot(&key);
                                            }
                                            consecutive_transport_failures = 0;
                                        }
                                    } else {
                                        consecutive_transport_failures = 0;
                                    }
                                    warn!(error = %e, "CXF consumer stream error; reconnecting");
                                    break;
                                }
                            }
                        }
                    }
                }

                if pool.reconnect.should_retry(reconnect_attempt + 1) {
                    let delay = pool.reconnect.delay_for(reconnect_attempt);
                    reconnect_attempt = reconnect_attempt.saturating_add(1);
                    tokio::select! {
                        _ = cancel.cancelled() => break,
                        _ = ctx.cancelled() => break,
                        _ = tokio::time::sleep(delay) => {}
                    }
                } else {
                    warn!("CXF consumer reconnect attempts exhausted");
                    break;
                }
            }
            Ok(())
        });

        self.task_handle = Some(handle);
        self.profile_guard = Some(guard);

        // Signal route-startup readiness now. The bridge slot was probed above
        // (fail-fast on Degraded/Stopped), so the bridge process is running.
        // The gRPC consumer stream is opened lazily inside the spawned task and
        // cannot be gated on here without deadlocking (see the note at the
        // open_consumer_stream call site). Mirrors camel-jms's best-effort
        // mark_ready in start().
        ctx.mark_ready();

        Ok(())
    }

    async fn stop(&mut self) -> Result<(), CamelError> {
        if let Some(mut guard) = self.profile_guard.take() {
            guard.release();
        }
        if let Some(cancel) = self.cancel_token.take() {
            cancel.cancel();
        }
        if let Some(mut handle) = self.task_handle.take()
            && tokio::time::timeout(Duration::from_secs(5), &mut handle)
                .await
                .is_err()
        {
            handle.abort();
            let _ = handle.await;
            warn!("CXF consumer task did not stop in 5s; aborted");
        }
        Ok(())
    }

    fn concurrency_model(&self) -> ConcurrencyModel {
        ConcurrencyModel::Sequential
    }

    fn startup_mode(&self) -> ConsumerStartupMode {
        ConsumerStartupMode::Explicit
    }

    fn background_task_handle(
        &mut self,
    ) -> Option<tokio::task::JoinHandle<Result<(), CamelError>>> {
        self.task_handle.take()
    }
}

#[cfg(test)]
mod tests;
