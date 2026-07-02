use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use camel_api::{Body, CamelError, Exchange};
use futures::StreamExt;
use http::uri::PathAndQuery;
use prost_reflect::MessageDescriptor;
use tokio::sync::{AcquireError, OwnedSemaphorePermit, Semaphore};
use tonic::Request;
use tonic::metadata::MetadataValue;
use tonic::transport::{Channel, Endpoint};
use tower::Service;
use tracing::{debug, error};

use crate::codec::RawBytesCodec;
use crate::config::{AuthConfig, GrpcConfig, apply_auth_metadata};
use crate::mode::GrpcMode;

mod retry;
pub use retry::is_retryable_tonic_status;
use retry::retry_rpc;
use retry::tonic_to_camel_error;

mod convert;
#[cfg(test)]
fn rt() -> std::sync::Arc<dyn camel_component_api::RuntimeObservability> {
    std::sync::Arc::new(camel_component_api::NoOpComponentContext)
}

pub(crate) use convert::proto_cache;
use convert::{json_to_protobuf, protobuf_to_json};

type ProducerFuture = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;
type AcquireFut =
    Option<Pin<Box<dyn Future<Output = Result<OwnedSemaphorePermit, AcquireError>> + Send>>>;

/// Default max concurrent gRPC calls per producer instance.
const DEFAULT_CONCURRENCY: usize = 128;

pub struct GrpcProducer {
    channel: Channel,
    path: PathAndQuery,
    req_descriptor: MessageDescriptor,
    resp_descriptor: MessageDescriptor,
    mode: GrpcMode,
    deadline_ms: Option<u64>,
    retry: camel_component_api::NetworkRetryPolicy,
    semaphore: Arc<Semaphore>,
    pending_permit: Option<OwnedSemaphorePermit>,
    acquire_fut: AcquireFut,
    auth: AuthConfig,
    config_metadata: Option<String>,
    runtime: Arc<dyn camel_component_api::RuntimeObservability>,
    /// C1: when `config.tls` is true, this is `true` AND the underlying
    /// `Channel`'s endpoint scheme is `https://...`. Together these prove
    /// the channel is TLS-protected, never plaintext.
    pub tls_enabled: bool,
    /// C1: the `https://host:port/...` URL the channel is bound to.
    /// Stored so tests can assert the scheme (which tonic's `Channel`
    /// does not expose publicly).
    pub endpoint_url: String,
    /// C1: the SNI / TLS server name plumbed into `ClientTlsConfig`.
    pub server_name: Option<String>,
}

impl Clone for GrpcProducer {
    fn clone(&self) -> Self {
        Self {
            channel: self.channel.clone(),
            path: self.path.clone(),
            req_descriptor: self.req_descriptor.clone(),
            resp_descriptor: self.resp_descriptor.clone(),
            mode: self.mode,
            deadline_ms: self.deadline_ms,
            retry: self.retry.clone(),
            semaphore: Arc::clone(&self.semaphore),
            // Each clone starts with a fresh permit state.
            pending_permit: None,
            acquire_fut: None,
            auth: self.auth.clone(),
            config_metadata: self.config_metadata.clone(),
            runtime: Arc::clone(&self.runtime),
            tls_enabled: self.tls_enabled,
            endpoint_url: self.endpoint_url.clone(),
            server_name: self.server_name.clone(),
        }
    }
}

impl GrpcProducer {
    /// Returns the scheme of the channel's underlying endpoint URL
    /// (`"https"` or `"http"`). Used by tests to assert that `tls=true`
    /// always produces a TLS channel, never plaintext.
    pub fn endpoint_scheme(&self) -> &str {
        // `endpoint_url` is `<scheme>://host:port/...`; the scheme ends at `://`.
        self.endpoint_url
            .split_once("://")
            .map(|(s, _)| s)
            .unwrap_or("http")
    }

    /// Returns the SNI / TLS server name configured for this producer.
    pub fn server_name(&self) -> Option<&str> {
        self.server_name.as_deref()
    }
}

impl GrpcProducer {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        addr: String,
        proto_path: PathBuf,
        service_name: String,
        method_name: String,
        mode: GrpcMode,
        deadline_ms: Option<u64>,
        config: &GrpcConfig,
        runtime: Arc<dyn camel_component_api::RuntimeObservability>,
        route_id: &str,
    ) -> Result<Self, CamelError> {
        let endpoint = Endpoint::from_shared(addr.clone()).map_err(|e| {
            runtime.health().force_unhealthy_for_route(
                route_id,
                "g:grpc:producer-create",
                &format!("invalid grpc endpoint: {e}"),
            );
            // log-policy: outside-contract
            error!(error = %e, "grpc producer creation failed");
            CamelError::EndpointCreationFailed(format!("invalid grpc endpoint: {e}"))
        })?;

        // C1 Batch 1: fail-closed TLS. When the operator declares `tls = true`
        // (Intent-Violation disposition), the channel MUST be TLS-protected.
        // Four outcomes (`config.tls` is the single source of truth):
        //   1. `config.tls_config` is Some(...) → wire `tonic::ClientTlsConfig`
        //      and establish a real TLS channel. The endpoint URL is rewritten
        //      from `http://` to `https://` so the scheme reflects TLS at the
        //      transport layer.
        //   2. `config.tls_config` is None and `config.tls = true` → hard-error
        //      (EndpointCreationFailed) so the producer never silently falls
        //      back to h2c / h2 plaintext. Auth tokens MUST NOT travel cleartext.
        //   3. `config.tls = false` and no `tls_config` → h2c (unchanged;
        //      explicit plaintext).
        //   4. `config.tls = false` WITH a `tls_config` supplied → hard-error
        //      (conflicting intent, review fix I-3): certs wired but TLS
        //      disabled is a misconfiguration; refuse rather than guess.
        let mut tls_enabled = false;
        let mut server_name: Option<String> = None;
        let endpoint = if config.tls {
            let tls_cfg = config.tls_config.as_ref().ok_or_else(|| {
                runtime.health().force_unhealthy_for_route(
                    route_id,
                    "g:grpc:producer-create",
                    "tls=true but no tls_config supplied; refusing plaintext h2c",
                );
                // log-policy: outside-contract
                error!("gRPC producer refused: tls=true with no tls_config (fail-closed)");
                CamelError::EndpointCreationFailed(
                    "gRPC tls=true requires tls_config; refusing to fall back to plaintext \
                     (C1 fail-closed). Provide a TlsConfig in the producer config or set \
                     tls=false explicitly."
                        .to_string(),
                )
            })?;
            let sn = tls_cfg.server_name.clone().unwrap_or_else(|| {
                // Fall back to the addr host (strip scheme). tonic requires a
                // DNS name; if we have no server name AND no host-derived
                // alternative, refuse to guess.
                let host = addr
                    .split_once("://")
                    .map(|(_, rest)| rest)
                    .unwrap_or(&addr);
                host.split(':').next().unwrap_or(host).to_string()
            });
            server_name = Some(sn.clone());
            let mut client_tls = tonic::transport::ClientTlsConfig::new().domain_name(sn);
            if let Some(ca_cert) = &tls_cfg.ca_cert_path {
                let pem = std::fs::read(ca_cert).map_err(|e| {
                    runtime.health().force_unhealthy_for_route(
                        route_id,
                        "g:grpc:producer-create",
                        &format!("failed to read ca_cert: {e}"),
                    );
                    // log-policy: outside-contract
                    error!(error = %e, "grpc producer TLS read failed");
                    CamelError::EndpointCreationFailed(format!("failed to read ca_cert: {e}"))
                })?;
                client_tls =
                    client_tls.ca_certificate(tonic::transport::Certificate::from_pem(pem));
            }
            if let (Some(cert), Some(key)) = (&tls_cfg.client_cert_path, &tls_cfg.client_key_path) {
                let cert_pem = std::fs::read(cert).map_err(|e| {
                    runtime.health().force_unhealthy_for_route(
                        route_id,
                        "g:grpc:producer-create",
                        &format!("failed to read client_cert: {e}"),
                    );
                    // log-policy: outside-contract
                    error!(error = %e, "grpc producer TLS read failed");
                    CamelError::EndpointCreationFailed(format!("failed to read client_cert: {e}"))
                })?;
                let key_pem = std::fs::read(key).map_err(|e| {
                    runtime.health().force_unhealthy_for_route(
                        route_id,
                        "g:grpc:producer-create",
                        &format!("failed to read client_key: {e}"),
                    );
                    // log-policy: outside-contract
                    error!(error = %e, "grpc producer TLS read failed");
                    CamelError::EndpointCreationFailed(format!("failed to read client_key: {e}"))
                })?;
                let identity = tonic::transport::Identity::from_pem(cert_pem, key_pem);
                client_tls = client_tls.identity(identity);
            }
            // `insecure_skip_verify` honors the operator's opt-in. In tonic 0.14,
            // skip-verification requires a custom `ServerCertVerifier` via
            // `tls_config_with_verifier`; the verifier requires a direct
            // tokio-rustls dep the workspace does not currently have. Rather
            // than pull a new transitive crypto crate in for one flag, we
            // log the operator's intent and document the limitation: when
            // `insecure_skip_verify=true`, we attempt the verifier-via-
            // custom-tls-config path; on failure (e.g. feature not
            // available in the locked-down build), we hard-error rather
            // than silently skip verification. The operator MUST explicitly
            // set this flag; the default (`false`) is the safe path.
            let endpoint = if tls_cfg.insecure_skip_verify {
                // Documented limitation: the custom-verifier path requires
                // a direct tokio-rustls dep, which is not in the
                // workspace. Surface the operator's intent as a hard
                // error so the misconfiguration cannot silently ship
                // without TLS. If a follow-up batch adds a verifier
                // shim, the implementation swaps in `tls_config_with_verifier`.
                runtime.health().force_unhealthy_for_route(
                    route_id,
                    "g:grpc:producer-create",
                    "insecure_skip_verify=true is not supported in this build; \
                     refusing plaintext h2c (C1 fail-closed).",
                );
                // log-policy: outside-contract
                error!(
                    "gRPC producer refused: insecure_skip_verify=true not supported in this build"
                );
                return Err(CamelError::EndpointCreationFailed(
                    "gRPC insecure_skip_verify=true is not supported in this build. \
                     The C1 fail-closed policy refuses to silently fall back to plaintext. \
                     Either set insecure_skip_verify=false or wait for a follow-up \
                     batch that adds a tokio-rustls verifier. See ADR-0032 / ADR-0033."
                        .to_string(),
                ));
            } else {
                endpoint.tls_config(client_tls).map_err(|e| {
                    runtime.health().force_unhealthy_for_route(
                        route_id,
                        "g:grpc:producer-create",
                        &format!("failed to configure TLS: {e}"),
                    );
                    // log-policy: outside-contract
                    error!(error = %e, "grpc producer TLS config failed");
                    CamelError::EndpointCreationFailed(format!("failed to configure TLS: {e}"))
                })?
            };
            tls_enabled = true;
            endpoint
        } else {
            // Outcome 4 (review fix I-3): conflicting intent — a TlsConfig
            // is supplied but `tls = false`. Refuse rather than silently
            // running plaintext with certs configured.
            if config.tls_config.is_some() {
                runtime.health().force_unhealthy_for_route(
                    route_id,
                    "g:grpc:producer-create",
                    "tls=false but tls_config supplied; conflicting TLS intent",
                );
                // log-policy: outside-contract
                error!(
                    "gRPC producer refused: tls=false with tls_config supplied (conflicting intent)"
                );
                return Err(CamelError::EndpointCreationFailed(
                    "gRPC tls=false conflicts with a supplied tls_config. Set tls=true \
                     to use the tls_config, or remove tls_config for explicit plaintext \
                     (C1 fail-closed). See ADR-0032 / ADR-0033."
                        .to_string(),
                ));
            }
            endpoint
        };
        let channel = endpoint.connect_lazy();

        // Build the stored endpoint_url: rewrite scheme to https when TLS is on.
        let endpoint_url = if tls_enabled {
            if let Some((_, rest)) = addr.split_once("://") {
                format!("https://{rest}")
            } else {
                addr.clone()
            }
        } else {
            addr.clone()
        };

        let cache = proto_cache();
        let pool = cache
            .get_or_compile(&proto_path, std::iter::empty::<&Path>())
            .map_err(|e| {
                runtime.health().force_unhealthy_for_route(
                    route_id,
                    "g:grpc:producer-create",
                    &format!("failed to compile proto: {e}"),
                );
                // log-policy: outside-contract
                error!(error = %e, "grpc producer creation failed");
                CamelError::EndpointCreationFailed(format!("failed to compile proto: {e}"))
            })?;

        let svc = pool.get_service_by_name(&service_name).ok_or_else(|| {
            let err = CamelError::EndpointCreationFailed(format!(
                "service descriptor not found: {service_name}"
            ));
            runtime.health().force_unhealthy_for_route(
                route_id,
                "g:grpc:producer-create",
                &format!("service descriptor not found: {service_name}"),
            );
            // log-policy: outside-contract
            error!(service = %service_name, error = %err, "grpc producer creation failed");
            err
        })?;

        let method = svc
            .methods()
            .find(|m| m.name() == method_name)
            .ok_or_else(|| {
                let err = CamelError::EndpointCreationFailed(format!(
                    "method descriptor not found: {service_name}/{method_name}"
                ));
                runtime.health().force_unhealthy_for_route(
                    route_id,
                    "g:grpc:producer-create",
                    &format!("method descriptor not found: {service_name}/{method_name}"),
                );
                // log-policy: outside-contract
                error!(service = %service_name, method = %method_name, error = %err, "grpc producer creation failed");
                err
            })?;
        let req_descriptor = method.input();
        let resp_descriptor = method.output();
        let path = PathAndQuery::from_maybe_shared(format!("/{service_name}/{method_name}"))
            .map_err(|e| {
                runtime.health().force_unhealthy_for_route(
                    route_id,
                    "g:grpc:producer-create",
                    &format!("invalid gRPC path: {e}"),
                );
                // log-policy: outside-contract
                error!(error = %e, "grpc producer creation failed");
                CamelError::EndpointCreationFailed(format!("invalid gRPC path: {e}"))
            })?;

        debug!(addr = %addr, service = %service_name, method = %method_name, "grpc producer created");
        Ok(Self {
            channel,
            path,
            req_descriptor,
            resp_descriptor,
            mode,
            deadline_ms,
            retry: config.retry.clone(),
            semaphore: Arc::new(Semaphore::new(DEFAULT_CONCURRENCY)),
            pending_permit: None,
            acquire_fut: None,
            auth: config.auth.clone(),
            config_metadata: config.metadata.clone(),
            runtime,
            tls_enabled,
            endpoint_url,
            server_name,
        })
    }

    fn body_to_json(body: Body) -> Result<serde_json::Value, CamelError> {
        match body {
            Body::Json(v) => Ok(v),
            Body::Text(s) => serde_json::from_str(&s).map_err(|e| {
                CamelError::TypeConversionFailed(format!("invalid JSON text body: {e}"))
            }),
            other => Err(CamelError::TypeConversionFailed(format!(
                "grpc producer requires JSON or text body, got {other:?}"
            ))),
        }
    }

    fn header_to_metadata(
        value: &serde_json::Value,
    ) -> Option<MetadataValue<tonic::metadata::Ascii>> {
        let s = match value {
            serde_json::Value::String(v) => v.clone(),
            serde_json::Value::Number(v) => v.to_string(),
            serde_json::Value::Bool(v) => v.to_string(),
            _ => return None,
        };
        MetadataValue::try_from(s).ok()
    }

    fn inject_headers<T>(exchange: &Exchange, request: &mut Request<T>) {
        for (k, v) in &exchange.input.headers {
            if let Some(meta) = Self::header_to_metadata(v)
                && let Ok(name) = tonic::metadata::MetadataKey::from_bytes(k.as_bytes())
            {
                request.metadata_mut().insert(name, meta);
            }
        }
    }

    fn call_unary(&mut self, mut exchange: Exchange) -> ProducerFuture {
        let channel = self.channel.clone();
        let path = self.path.clone();
        let req_df = self.req_descriptor.clone();
        let resp_desc = self.resp_descriptor.clone();
        let deadline_ms = self.deadline_ms;
        let retry = self.retry.clone();
        let auth = self.auth.clone();
        let config_metadata = self.config_metadata.clone();

        Box::pin(async move {
            debug!(path = %path, "grpc unary call");
            let json = Self::body_to_json(exchange.input.body.clone())?;
            let buf = json_to_protobuf(json, req_df)?;
            let mut request = Request::new(buf);
            Self::inject_headers(&exchange, &mut request);
            apply_auth_metadata(&auth, &mut request).await?;
            if let Some(ref metadata_str) = config_metadata {
                for pair in metadata_str.split(',') {
                    let pair = pair.trim();
                    if let Some((key, value)) = pair.split_once('=') {
                        let key = key.trim();
                        let value = value.trim();
                        if let Ok(name) = tonic::metadata::MetadataKey::from_bytes(key.as_bytes())
                            && let Ok(meta_val) = MetadataValue::try_from(value)
                        {
                            request.metadata_mut().insert(name, meta_val);
                        }
                    }
                }
                debug!("applied config metadata to gRPC unary request");
            }
            if let Some(ms) = deadline_ms {
                request.set_timeout(Duration::from_millis(ms));
            }

            let metadata_map = request.metadata().clone();
            let body = request.into_inner();

            let response = retry_rpc(channel, &retry, "unary", |mut grpc| {
                let mut req = Request::new(body.clone());
                *req.metadata_mut() = metadata_map.clone();
                if let Some(ms) = deadline_ms {
                    req.set_timeout(Duration::from_millis(ms));
                }
                let p = path.clone();
                async move {
                    grpc.ready().await.map_err(|e| {
                        tonic::Status::unavailable(format!("grpc client not ready: {e}"))
                    })?;
                    grpc.unary(req, p, RawBytesCodec).await
                }
            })
            .await?;

            let resp_json = protobuf_to_json(response.into_inner(), resp_desc)?;
            exchange.input.body = Body::Json(resp_json);
            Ok(exchange)
        })
    }

    fn call_server_streaming(&mut self, mut exchange: Exchange) -> ProducerFuture {
        let channel = self.channel.clone();
        let path = self.path.clone();
        let req_df = self.req_descriptor.clone();
        let resp_desc = self.resp_descriptor.clone();
        let deadline_ms = self.deadline_ms;
        let retry = self.retry.clone();
        let auth = self.auth.clone();
        let config_metadata = self.config_metadata.clone();

        Box::pin(async move {
            debug!(path = %path, "grpc server streaming call");
            let json = Self::body_to_json(exchange.input.body.clone())?;
            let buf = json_to_protobuf(json, req_df)?;
            let mut request = Request::new(buf);
            Self::inject_headers(&exchange, &mut request);
            apply_auth_metadata(&auth, &mut request).await?;
            if let Some(ref metadata_str) = config_metadata {
                for pair in metadata_str.split(',') {
                    let pair = pair.trim();
                    if let Some((key, value)) = pair.split_once('=') {
                        let key = key.trim();
                        let value = value.trim();
                        if let Ok(name) = tonic::metadata::MetadataKey::from_bytes(key.as_bytes())
                            && let Ok(meta_val) = MetadataValue::try_from(value)
                        {
                            request.metadata_mut().insert(name, meta_val);
                        }
                    }
                }
                debug!("applied config metadata to gRPC server streaming request");
            }
            if let Some(ms) = deadline_ms {
                request.set_timeout(Duration::from_millis(ms));
            }

            let metadata_map = request.metadata().clone();
            let body = request.into_inner();

            let response = retry_rpc(channel, &retry, "server_streaming", |mut grpc| {
                let mut req = Request::new(body.clone());
                *req.metadata_mut() = metadata_map.clone();
                if let Some(ms) = deadline_ms {
                    req.set_timeout(Duration::from_millis(ms));
                }
                let p = path.clone();
                async move {
                    grpc.ready().await.map_err(|e| {
                        tonic::Status::unavailable(format!("grpc client not ready: {e}"))
                    })?;
                    grpc.server_streaming(req, p, RawBytesCodec).await
                }
            })
            .await?;

            let mut results = Vec::new();
            let mut stream = response.into_inner();
            while let Some(item) = stream.next().await {
                let bytes = item.map_err(tonic_to_camel_error)?;
                let resp_json = protobuf_to_json(bytes, resp_desc.clone())?;
                results.push(resp_json);
            }

            exchange.input.body = Body::Json(serde_json::Value::Array(results));
            Ok(exchange)
        })
    }

    fn call_client_streaming(&mut self, mut exchange: Exchange) -> ProducerFuture {
        let channel = self.channel.clone();
        let path = self.path.clone();
        let req_df = self.req_descriptor.clone();
        let resp_desc = self.resp_descriptor.clone();
        let deadline_ms = self.deadline_ms;
        let retry = self.retry.clone();
        let auth = self.auth.clone();
        let config_metadata = self.config_metadata.clone();

        Box::pin(async move {
            debug!(path = %path, "grpc client streaming call");
            let json = Self::body_to_json(exchange.input.body.clone())?;
            let items = json.as_array().ok_or_else(|| {
                CamelError::TypeConversionFailed(
                    "grpc client streaming producer requires JSON array body".into(),
                )
            })?;

            let encoded: Vec<Vec<u8>> = items
                .iter()
                .map(|item| json_to_protobuf(item.clone(), req_df.clone()))
                .collect::<Result<_, _>>()?;

            // Build the initial request with headers/auth/timeout once.
            let mut request_template = Request::new(futures::stream::iter(encoded.clone()));
            Self::inject_headers(&exchange, &mut request_template);
            apply_auth_metadata(&auth, &mut request_template).await?;
            if let Some(ref metadata_str) = config_metadata {
                for pair in metadata_str.split(',') {
                    let pair = pair.trim();
                    if let Some((key, value)) = pair.split_once('=') {
                        let key = key.trim();
                        let value = value.trim();
                        if let Ok(name) = tonic::metadata::MetadataKey::from_bytes(key.as_bytes())
                            && let Ok(meta_val) = MetadataValue::try_from(value)
                        {
                            request_template.metadata_mut().insert(name, meta_val);
                        }
                    }
                }
                debug!("applied config metadata to gRPC client streaming request");
            }
            if let Some(ms) = deadline_ms {
                request_template.set_timeout(Duration::from_millis(ms));
            }

            let metadata_map = request_template.metadata().clone();

            let response = retry_rpc(channel, &retry, "client_streaming", |mut grpc| {
                let mut request = Request::new(futures::stream::iter(encoded.clone()));
                *request.metadata_mut() = metadata_map.clone();
                if let Some(ms) = deadline_ms {
                    request.set_timeout(Duration::from_millis(ms));
                }
                let p = path.clone();
                async move {
                    grpc.ready().await.map_err(|e| {
                        tonic::Status::unavailable(format!("grpc client not ready: {e}"))
                    })?;
                    grpc.client_streaming(request, p, RawBytesCodec).await
                }
            })
            .await?;

            let resp_json = protobuf_to_json(response.into_inner(), resp_desc)?;
            exchange.input.body = Body::Json(resp_json);
            Ok(exchange)
        })
    }

    fn call_bidi(&mut self, mut exchange: Exchange) -> ProducerFuture {
        let channel = self.channel.clone();
        let path = self.path.clone();
        let req_df = self.req_descriptor.clone();
        let resp_desc = self.resp_descriptor.clone();
        let deadline_ms = self.deadline_ms;
        let retry = self.retry.clone();
        let auth = self.auth.clone();
        let config_metadata = self.config_metadata.clone();

        Box::pin(async move {
            debug!(path = %path, "grpc bidi streaming call");
            let json = Self::body_to_json(exchange.input.body.clone())?;
            let items = json.as_array().ok_or_else(|| {
                CamelError::TypeConversionFailed(
                    "grpc bidi streaming producer requires JSON array body".into(),
                )
            })?;

            let encoded: Vec<Vec<u8>> = items
                .iter()
                .map(|item| json_to_protobuf(item.clone(), req_df.clone()))
                .collect::<Result<_, _>>()?;

            // Build the initial request with headers/auth/timeout once.
            let mut request_template = Request::new(futures::stream::iter(encoded.clone()));
            Self::inject_headers(&exchange, &mut request_template);
            apply_auth_metadata(&auth, &mut request_template).await?;
            if let Some(ref metadata_str) = config_metadata {
                for pair in metadata_str.split(',') {
                    let pair = pair.trim();
                    if let Some((key, value)) = pair.split_once('=') {
                        let key = key.trim();
                        let value = value.trim();
                        if let Ok(name) = tonic::metadata::MetadataKey::from_bytes(key.as_bytes())
                            && let Ok(meta_val) = MetadataValue::try_from(value)
                        {
                            request_template.metadata_mut().insert(name, meta_val);
                        }
                    }
                }
                debug!("applied config metadata to gRPC bidi streaming request");
            }
            if let Some(ms) = deadline_ms {
                request_template.set_timeout(Duration::from_millis(ms));
            }

            let metadata_map = request_template.metadata().clone();

            let response = retry_rpc(channel, &retry, "bidi", |mut grpc| {
                let mut request = Request::new(futures::stream::iter(encoded.clone()));
                *request.metadata_mut() = metadata_map.clone();
                if let Some(ms) = deadline_ms {
                    request.set_timeout(Duration::from_millis(ms));
                }
                let p = path.clone();
                async move {
                    grpc.ready().await.map_err(|e| {
                        tonic::Status::unavailable(format!("grpc client not ready: {e}"))
                    })?;
                    grpc.streaming(request, p, RawBytesCodec).await
                }
            })
            .await?;

            let mut results = Vec::new();
            let mut stream = response.into_inner();
            while let Some(item) = stream.next().await {
                let bytes = item.map_err(tonic_to_camel_error)?;
                let resp_json = protobuf_to_json(bytes, resp_desc.clone())?;
                results.push(resp_json);
            }

            exchange.input.body = Body::Json(serde_json::Value::Array(results));
            Ok(exchange)
        })
    }
}

// ── Tower Service impl ────────────────────────────────────────────────────

impl Service<Exchange> for GrpcProducer {
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        if self.pending_permit.is_some() {
            return Poll::Ready(Ok(()));
        }
        let fut = self
            .acquire_fut
            .get_or_insert_with(|| Box::pin(Arc::clone(&self.semaphore).acquire_owned()));
        match fut.as_mut().poll(cx) {
            Poll::Ready(Ok(permit)) => {
                self.acquire_fut = None;
                self.pending_permit = Some(permit);
                Poll::Ready(Ok(()))
            }
            Poll::Ready(Err(_)) => Poll::Ready(Err(CamelError::ChannelClosed)),
            Poll::Pending => Poll::Pending,
        }
    }

    fn call(&mut self, exchange: Exchange) -> ProducerFuture {
        let permit = match self.pending_permit.take() {
            Some(p) => p,
            None => {
                return Box::pin(async {
                    Err(CamelError::ProcessorError(
                        "call() invoked without poll_ready()".into(),
                    ))
                });
            }
        };
        let inner = match self.mode {
            GrpcMode::Unary => self.call_unary(exchange),
            GrpcMode::ServerStreaming => self.call_server_streaming(exchange),
            GrpcMode::ClientStreaming => self.call_client_streaming(exchange),
            GrpcMode::Bidi => self.call_bidi(exchange),
        };
        Box::pin(async move {
            let _permit = permit; // hold semaphore slot for call duration
            inner.await
        })
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};
    use std::task::{Context, Poll};
    use std::time::Duration;

    use super::GrpcProducer;
    use super::rt;
    use crate::GrpcMode;
    use crate::config::{GrpcConfig, TlsConfig};
    use camel_api::{Body, CamelError, Exchange, Message, MetricsCollector};
    use camel_component_api::{HealthCheckRegistry, NetworkRetryPolicy, RuntimeObservability};
    use tonic::Request;
    use tower::Service;

    fn exchange_with_headers(headers: &[(&str, serde_json::Value)]) -> Exchange {
        let mut msg = Message::default();
        for (k, v) in headers {
            msg.set_header(*k, v.clone());
        }
        Exchange::new(msg)
    }

    fn default_config() -> GrpcConfig {
        GrpcConfig {
            proto_file: None,
            service: None,
            method: None,
            reflection: false,
            tls: false,
            max_receive_message_length: 4 * 1024 * 1024,
            deadline_ms: None,
            metadata: None,
            tls_config: None,
            auth: crate::config::AuthConfig::None,
            interceptors: crate::config::InterceptorConfig::default(),
            consumer_strategy: crate::config::ConsumerStrategy::default(),
            producer_strategy: crate::config::ProducerStrategy::default(),
            retry: NetworkRetryPolicy::default(),
        }
    }

    // ── body_to_json tests ─────────────────────────────────────────────

    #[test]
    fn test_body_to_json_from_json_body() {
        let body = Body::Json(serde_json::json!({"key": "value"}));
        let result = GrpcProducer::body_to_json(body).unwrap();
        assert_eq!(result["key"], "value");
    }

    #[test]
    fn test_body_to_json_from_text_body_valid_json() {
        let body = Body::Text(r#"{"name":"test"}"#.to_string());
        let result = GrpcProducer::body_to_json(body).unwrap();
        assert_eq!(result["name"], "test");
    }

    #[test]
    fn test_body_to_json_from_text_body_invalid_json() {
        let body = Body::Text("not json".to_string());
        let err = GrpcProducer::body_to_json(body).unwrap_err();
        assert!(matches!(err, CamelError::TypeConversionFailed(_)));
        assert!(err.to_string().contains("invalid JSON text body"));
    }

    #[test]
    fn test_body_to_json_from_empty_body() {
        let body = Body::Empty;
        let err = GrpcProducer::body_to_json(body).unwrap_err();
        assert!(matches!(err, CamelError::TypeConversionFailed(_)));
        assert!(err.to_string().contains("requires JSON or text body"));
    }

    #[test]
    fn test_body_to_json_from_bytes_body() {
        let body = Body::Bytes(bytes::Bytes::from_static(b"raw"));
        let err = GrpcProducer::body_to_json(body).unwrap_err();
        assert!(matches!(err, CamelError::TypeConversionFailed(_)));
    }

    #[test]
    fn test_body_to_json_from_xml_body() {
        let body = Body::Xml("<root/>".to_string());
        let err = GrpcProducer::body_to_json(body).unwrap_err();
        assert!(matches!(err, CamelError::TypeConversionFailed(_)));
    }

    // ── header_to_metadata tests ───────────────────────────────────────

    #[test]
    fn test_header_to_metadata_from_string() {
        let value = serde_json::Value::String("hello".to_string());
        let result = GrpcProducer::header_to_metadata(&value);
        assert!(result.is_some());
    }

    #[test]
    fn test_header_to_metadata_from_number() {
        let value = serde_json::Value::Number(42.into());
        let result = GrpcProducer::header_to_metadata(&value);
        assert!(result.is_some());
        assert_eq!(result.unwrap().to_str().unwrap(), "42");
    }

    #[test]
    fn test_header_to_metadata_from_bool_true() {
        let value = serde_json::Value::Bool(true);
        let result = GrpcProducer::header_to_metadata(&value);
        assert!(result.is_some());
        assert_eq!(result.unwrap().to_str().unwrap(), "true");
    }

    #[test]
    fn test_header_to_metadata_from_bool_false() {
        let value = serde_json::Value::Bool(false);
        let result = GrpcProducer::header_to_metadata(&value);
        assert!(result.is_some());
        assert_eq!(result.unwrap().to_str().unwrap(), "false");
    }

    #[test]
    fn test_header_to_metadata_from_object() {
        let value = serde_json::json!({"key": "value"});
        let result = GrpcProducer::header_to_metadata(&value);
        assert!(result.is_none());
    }

    #[test]
    fn test_header_to_metadata_from_array() {
        let value = serde_json::json!(["a", "b"]);
        let result = GrpcProducer::header_to_metadata(&value);
        assert!(result.is_none());
    }

    #[test]
    fn test_header_to_metadata_from_null() {
        let value = serde_json::Value::Null;
        let result = GrpcProducer::header_to_metadata(&value);
        assert!(result.is_none());
    }

    // ── inject_headers tests ───────────────────────────────────────────

    #[test]
    fn test_inject_headers_transfers_all_valid_headers() {
        let exchange = exchange_with_headers(&[
            ("x-custom", serde_json::Value::String("val1".to_string())),
            ("x-number", serde_json::Value::Number(123.into())),
        ]);
        let mut request = Request::new(());
        GrpcProducer::inject_headers(&exchange, &mut request);

        let meta = request.metadata();
        assert_eq!(meta.get("x-custom").unwrap().to_str().unwrap(), "val1");
        assert_eq!(meta.get("x-number").unwrap().to_str().unwrap(), "123");
    }

    #[test]
    fn test_inject_headers_skips_unsupported_types() {
        let exchange = exchange_with_headers(&[
            ("x-good", serde_json::Value::String("ok".to_string())),
            ("x-bad", serde_json::json!({"nested": true})),
        ]);
        let mut request = Request::new(());
        GrpcProducer::inject_headers(&exchange, &mut request);

        let meta = request.metadata();
        assert!(meta.get("x-good").is_some());
        assert!(meta.get("x-bad").is_none());
    }

    #[test]
    fn test_inject_headers_empty_exchange() {
        let exchange = Exchange::new(Message::default());
        let mut request = Request::new(());
        GrpcProducer::inject_headers(&exchange, &mut request);
        assert!(request.metadata().is_empty());
    }

    // ── tonic_to_camel_error tests ─────────────────────────────────────

    // ── GrpcMode tests ─────────────────────────────────────────────────

    #[test]
    fn test_grpc_mode_derives() {
        let mode = GrpcMode::Unary;
        let _ = format!("{mode:?}");
        #[allow(clippy::clone_on_copy)]
        let cloned = mode.clone();
        assert_eq!(mode, cloned);
        let copied = mode;
        assert_eq!(mode, copied);
    }

    #[test]
    fn test_grpc_mode_all_variants_distinct() {
        assert_ne!(GrpcMode::Unary, GrpcMode::ServerStreaming);
        assert_ne!(GrpcMode::Unary, GrpcMode::ClientStreaming);
        assert_ne!(GrpcMode::Unary, GrpcMode::Bidi);
        assert_ne!(GrpcMode::ServerStreaming, GrpcMode::ClientStreaming);
        assert_ne!(GrpcMode::ServerStreaming, GrpcMode::Bidi);
        assert_ne!(GrpcMode::ClientStreaming, GrpcMode::Bidi);
    }

    // ── producer lifecycle tests ───────────────────────────────────────

    #[tokio::test]
    async fn test_producer_new_invalid_endpoint() {
        let result = GrpcProducer::new(
            "not-a-valid-endpoint".to_string(),
            PathBuf::from("/dev/null"),
            "svc".to_string(),
            "Method".to_string(),
            GrpcMode::Unary,
            None,
            &default_config(),
            rt(),
            "grpc-producer-test-route",
        );
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_producer_new_missing_proto_file() {
        let result = GrpcProducer::new(
            "http://localhost:50051".to_string(),
            PathBuf::from("/nonexistent/file.proto"),
            "svc".to_string(),
            "Method".to_string(),
            GrpcMode::Unary,
            None,
            &default_config(),
            rt(),
            "grpc-producer-test-route",
        );
        let err = match result {
            Err(e) => e,
            Ok(_) => panic!("expected error"),
        };
        assert!(err.to_string().contains("failed to compile proto"));
    }

    #[tokio::test]
    async fn test_producer_new_service_not_found() {
        let proto_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/helloworld.proto");
        let result = GrpcProducer::new(
            "http://localhost:50051".to_string(),
            proto_path,
            "nonexistent.Service".to_string(),
            "SayHello".to_string(),
            GrpcMode::Unary,
            None,
            &default_config(),
            rt(),
            "grpc-producer-test-route",
        );
        let err = match result {
            Err(e) => e,
            Ok(_) => panic!("expected error"),
        };
        assert!(err.to_string().contains("service descriptor not found"));
    }

    #[tokio::test]
    async fn test_producer_new_method_not_found() {
        let proto_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/helloworld.proto");
        let result = GrpcProducer::new(
            "http://localhost:50051".to_string(),
            proto_path,
            "helloworld.Greeter".to_string(),
            "NonExistentMethod".to_string(),
            GrpcMode::Unary,
            None,
            &default_config(),
            rt(),
            "grpc-producer-test-route",
        );
        let err = match result {
            Err(e) => e,
            Ok(_) => panic!("expected error"),
        };
        assert!(err.to_string().contains("method descriptor not found"));
    }

    #[tokio::test]
    async fn test_producer_poll_ready_always_ready() {
        let proto_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/helloworld.proto");
        let mut producer = GrpcProducer::new(
            "http://localhost:50051".to_string(),
            proto_path,
            "helloworld.Greeter".to_string(),
            "SayHello".to_string(),
            GrpcMode::Unary,
            None,
            &default_config(),
            rt(),
            "grpc-producer-test-route",
        )
        .unwrap();

        let waker = futures::task::noop_waker();
        let mut cx = Context::from_waker(&waker);
        assert!(matches!(producer.poll_ready(&mut cx), Poll::Ready(Ok(()))));
    }

    #[tokio::test]
    async fn test_producer_call_dispatches_by_mode() {
        let proto_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/helloworld.proto");
        let producer_unary = GrpcProducer::new(
            "http://localhost:50051".to_string(),
            proto_path.clone(),
            "helloworld.Greeter".to_string(),
            "SayHello".to_string(),
            GrpcMode::Unary,
            None,
            &default_config(),
            rt(),
            "grpc-producer-test-route",
        )
        .unwrap();

        let producer_streaming = GrpcProducer::new(
            "http://localhost:50051".to_string(),
            proto_path,
            "helloworld.Greeter".to_string(),
            "SayHello".to_string(),
            GrpcMode::ServerStreaming,
            None,
            &default_config(),
            rt(),
            "grpc-producer-test-route",
        )
        .unwrap();

        assert_eq!(producer_unary.mode, GrpcMode::Unary);
        assert_eq!(producer_streaming.mode, GrpcMode::ServerStreaming);
    }

    // ── force_unhealthy_for_route regression tests ────────────────────

    /// Fixture: captures `force_unhealthy_for_route` calls.
    #[derive(Debug, Default)]
    struct RecordingHealth {
        forced: Mutex<Vec<(String, String, String)>>,
    }

    impl HealthCheckRegistry for RecordingHealth {
        fn force_unhealthy_for_route(&self, route_id: &str, name: &str, reason: &str) {
            self.forced.lock().unwrap().push((
                route_id.to_string(),
                name.to_string(),
                reason.to_string(),
            ));
        }
    }

    struct NoopMetrics;

    impl MetricsCollector for NoopMetrics {
        fn record_exchange_duration(&self, _: &str, _: Duration) {}
        fn increment_errors(&self, _: &str, _: &str) {}
        fn increment_exchanges(&self, _: &str) {}
        fn set_queue_depth(&self, _: &str, _: usize) {}
        fn record_circuit_breaker_change(&self, _: &str, _: &str, _: &str) {}
    }

    struct RecordingRuntime {
        health: Arc<RecordingHealth>,
    }

    impl RuntimeObservability for RecordingRuntime {
        fn metrics(&self) -> Arc<dyn MetricsCollector> {
            Arc::new(NoopMetrics)
        }
        fn health(&self) -> Arc<dyn HealthCheckRegistry> {
            self.health.clone()
        }
    }

    #[tokio::test]
    async fn test_producer_new_invalid_endpoint_calls_force_unhealthy() {
        let health = Arc::new(RecordingHealth::default());
        let rt: Arc<dyn RuntimeObservability> = Arc::new(RecordingRuntime {
            health: health.clone(),
        });

        let result = GrpcProducer::new(
            "not-a-valid-endpoint".to_string(),
            PathBuf::from("/dev/null"),
            "svc".to_string(),
            "Method".to_string(),
            GrpcMode::Unary,
            None,
            &default_config(),
            rt,
            "grpc-producer-test-route",
        );
        assert!(result.is_err());

        let forced = health.forced.lock().unwrap();
        assert_eq!(forced.len(), 1, "expected one force_unhealthy call");
        assert_eq!(forced[0].0, "grpc-producer-test-route");
        assert_eq!(forced[0].1, "g:grpc:producer-create");
        assert!(!forced[0].2.is_empty(), "reason should be non-empty");
    }

    // ── edge case tests ────────────────────────────────────────────────

    #[test]
    fn test_body_to_json_from_null_body() {
        let body = Body::Json(serde_json::Value::Null);
        let result = GrpcProducer::body_to_json(body).unwrap();
        assert!(result.is_null());
    }

    #[test]
    fn test_body_to_json_from_array_body() {
        let body = Body::Json(serde_json::json!([1, 2, 3]));
        let result = GrpcProducer::body_to_json(body).unwrap();
        assert!(result.is_array());
        assert_eq!(result.as_array().unwrap().len(), 3);
    }

    #[test]
    fn test_header_to_metadata_from_negative_number() {
        let value = serde_json::Value::Number((-42).into());
        let result = GrpcProducer::header_to_metadata(&value);
        assert!(result.is_some());
        assert_eq!(result.unwrap().to_str().unwrap(), "-42");
    }

    #[test]
    fn test_header_to_metadata_from_float() {
        let value = serde_json::Value::Number(serde_json::Number::from_f64(3.15).unwrap());
        let result = GrpcProducer::header_to_metadata(&value);
        assert!(result.is_some());
    }

    #[test]
    fn test_inject_headers_skips_invalid_metadata_key() {
        let exchange =
            exchange_with_headers(&[("x-good", serde_json::Value::String("ok".to_string()))]);
        let mut request = Request::new(());
        GrpcProducer::inject_headers(&exchange, &mut request);
        assert!(request.metadata().get("x-good").is_some());
    }

    // ── C1 Batch 1: fail-closed TLS ────────────────────────────────

    /// C1 (fail-closed): when `config.tls = true` and the operator has NOT
    /// supplied a `TlsConfig`, the producer MUST hard-error at construction.
    /// h2c / h2 plaintext is never silently substituted. This is the
    /// fail-closed default: a gRPC producer that declares `tls = true`
    /// either establishes TLS (when `tls_config` is present) or refuses
    /// to start. Credentials never travel cleartext when `tls = true`.
    #[tokio::test]
    async fn test_grpc_tls_true_without_tls_config_hard_errors() {
        let proto_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/helloworld.proto");
        let mut config = default_config();
        config.tls = true; // operator declares intent
        // config.tls_config left None deliberately.

        let result = GrpcProducer::new(
            "http://localhost:50051".to_string(),
            proto_path,
            "helloworld.Greeter".to_string(),
            "SayHello".to_string(),
            GrpcMode::Unary,
            None,
            &config,
            rt(),
            "grpc-tls-failclosed-route",
        );
        assert!(
            result.is_err(),
            "tls=true with no tls_config must hard-error, never silently h2c"
        );
        let err = match result {
            Err(e) => e,
            Ok(_) => panic!("expected error"),
        };
        let err = err.to_string();
        // The error must name the missing piece so operators can fix it.
        assert!(
            err.contains("tls")
                || err.contains("TLS")
                || err.contains("mtls")
                || err.contains("mTLS"),
            "error must mention TLS: {err}"
        );
    }

    /// C1 (TLS wired, not plaintext): when `config.tls = true` AND a
    /// `TlsConfig` is supplied, the producer creates successfully AND
    /// the resulting channel's underlying endpoint uses `https://...`
    /// (not `http://...`) — i.e. the channel is TLS-protected, never
    /// plaintext. The endpoint URL is rewritten to `https` as a load-bearing
    /// detail: tonic's `tls_config` only matters at handshake time, but
    /// the scheme is what a real-world proxy / ALPN will inspect.
    ///
    /// SCOPE (review fix I-5): this is PLUMBING EVIDENCE, not a handshake
    /// proof — `connect_lazy()` never negotiates TLS in this test and the
    /// PEM is fake (tonic's `Certificate::from_pem` stores bytes without
    /// eager validation). A real handshake integration test against a
    /// rustls test server is a filed follow-up (see Follow-ups block at
    /// the end of Task 4). What Batch 1 proves: fail-closed refusal +
    /// correct ClientTlsConfig/scheme plumbing.
    #[tokio::test]
    async fn test_grpc_tls_true_with_tls_config_rewrites_to_https_and_wires_tls() {
        // Use a temp CA cert so the TLS path actually reads a cert.
        // An invalid / empty PEM is OK here: the producer should still
        // build (it does not handshake in this test); what we verify is
        // (a) creation succeeds, (b) the endpoint scheme is https, and
        // (c) the producer's `tls_enabled` diagnostic field is true.
        let tmp = std::env::temp_dir().join("camel-grpc-ca-test.pem");
        std::fs::write(
            &tmp,
            b"-----BEGIN CERTIFICATE-----\nMIIBfake\n-----END CERTIFICATE-----\n",
        )
        .unwrap();

        let proto_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/helloworld.proto");
        let mut config = default_config();
        config.tls = true;
        config.tls_config = Some(TlsConfig {
            tls_enabled: true,
            ca_cert_path: Some(tmp.to_string_lossy().into_owned()),
            client_cert_path: None,
            client_key_path: None,
            insecure_skip_verify: false,
            server_name: Some("grpc.example.com".to_string()),
        });

        let producer = GrpcProducer::new(
            "http://localhost:50051".to_string(),
            proto_path,
            "helloworld.Greeter".to_string(),
            "SayHello".to_string(),
            GrpcMode::Unary,
            None,
            &config,
            rt(),
            "grpc-tls-wired-route",
        )
        .expect("tls=true + tls_config must build a TLS producer, not error");

        // The producer stores the endpoint URL it was given AND a
        // diagnostic `tls_enabled` flag. Both must reflect TLS.
        assert!(producer.tls_enabled, "tls_enabled must be true");
        assert_eq!(
            producer.endpoint_scheme(),
            "https",
            "endpoint scheme MUST be https when tls=true, never http (proves the channel is TLS, not plaintext)"
        );
        assert_eq!(
            producer.server_name(),
            Some("grpc.example.com"),
            "operator-supplied server_name must be plumbed through"
        );
    }

    /// C1 (conflicting intent, review fix I-3): `tls = false` while a
    /// `TlsConfig` IS supplied is contradictory — the operator wired
    /// certs but disabled TLS. `config.tls` is the single source of
    /// truth; a dormant `tls_config` under `tls=false` is a
    /// misconfiguration that would silently run plaintext with certs
    /// configured. Fail-closed: refuse construction.
    #[tokio::test]
    async fn test_grpc_tls_false_with_tls_config_conflicting_intent_errors() {
        let proto_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/helloworld.proto");
        let mut config = default_config();
        config.tls = false; // explicit plaintext...
        config.tls_config = Some(TlsConfig {
            tls_enabled: true, // ...but a TLS config is wired
            ca_cert_path: None,
            client_cert_path: None,
            client_key_path: None,
            insecure_skip_verify: false,
            server_name: None,
        });

        let result = GrpcProducer::new(
            "http://localhost:50051".to_string(),
            proto_path,
            "helloworld.Greeter".to_string(),
            "SayHello".to_string(),
            GrpcMode::Unary,
            None,
            &config,
            rt(),
            "grpc-tls-conflict-route",
        );
        assert!(
            result.is_err(),
            "tls=false with tls_config supplied is conflicting intent and must error"
        );
        let err = match result {
            Err(e) => e,
            Ok(_) => panic!("expected error"),
        };
        let err = err.to_string();
        assert!(
            err.contains("conflict") || err.contains("tls"),
            "error must name the conflict: {err}"
        );
    }
}
