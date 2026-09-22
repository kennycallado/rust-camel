use std::path::PathBuf;
use std::sync::Arc;

use camel_api::component_metadata::ComponentMetadata;
use camel_component_api::{
    BoxProcessor, CamelError, Component, ComponentContext, Consumer, Endpoint, ProducerContext,
    RuntimeObservability,
};

use crate::config::{
    GrpcConfig, GrpcServerConfig, ServerTransport, TransportIntent, parse_grpc_uri,
};
use crate::consumer::{GrpcConsumer, resolve_grpc_mode};
use crate::health::GrpcHealthCheck;
use crate::metadata::GrpcMetadataDescriptor;
use crate::producer::GrpcProducer;

pub struct GrpcComponent;

impl GrpcComponent {
    pub fn new() -> Self {
        Self
    }
}

impl Default for GrpcComponent {
    fn default() -> Self {
        Self::new()
    }
}

impl Component for GrpcComponent {
    fn scheme(&self) -> &str {
        "grpc"
    }

    fn metadata(&self) -> ComponentMetadata {
        GrpcMetadataDescriptor::metadata()
    }

    fn create_endpoint(
        &self,
        uri: &str,
        ctx: &dyn ComponentContext,
    ) -> Result<Box<dyn Endpoint>, CamelError> {
        let (host, port, service_name, method_name, cfg) = parse_grpc_uri(uri)?;
        let proto_file = cfg.proto_file.clone().ok_or_else(|| {
            CamelError::EndpointCreationFailed(
                "missing required query parameter: protoFile".to_string(),
            )
        })?;
        let addr = format!("http://{host}:{port}");

        let health_check = GrpcHealthCheck::new(host.clone(), port);
        ctx.register_current_route_health_check(Arc::new(health_check));

        Ok(Box::new(GrpcEndpoint {
            uri: uri.to_string(),
            addr,
            host,
            port,
            proto_path: PathBuf::from(proto_file),
            service_name,
            method_name,
            deadline_ms: cfg.deadline_ms,
            config: cfg,
        }))
    }
}

struct GrpcEndpoint {
    uri: String,
    addr: String,
    host: String,
    port: u16,
    proto_path: PathBuf,
    service_name: String,
    method_name: String,
    deadline_ms: Option<u64>,
    config: GrpcConfig,
}

impl Endpoint for GrpcEndpoint {
    fn uri(&self) -> &str {
        &self.uri
    }

    fn create_consumer(
        &self,
        rt: Arc<dyn RuntimeObservability>,
    ) -> Result<Box<dyn Consumer>, CamelError> {
        // C2: direction-aware fail-closed. transport=tls declared but the
        // server has no certs → refuse (no silent plaintext on a TLS port).
        // Check this BEFORE proto compilation to fail fast.
        if self.config.transport_intent == TransportIntent::Tls
            && !matches!(self.config.server_transport, ServerTransport::Tls(_))
        {
            return Err(CamelError::EndpointCreationFailed(
                "gRPC from grpc://...?transport=tls requires serverCertPath + serverKeyPath \
                 (inbound TLS termination). Refusing to serve plaintext on a TLS-intent endpoint."
                    .to_string(),
            ));
        }

        let path = format!("/{}/{}", self.service_name, self.method_name);
        let mode = resolve_grpc_mode(&self.proto_path, &self.service_name, &self.method_name)?;

        // rc-9kgtm: reject oversized consumerConcurrency fail-closed BEFORE any
        // shared-server registry mutation, envelope-channel construction, or
        // dispatcher-semaphore construction (bd rc-9kgtm). The channel==
        // semaphore invariant derives both primitives from this one value.
        let consumer_concurrency =
            crate::consumer::consumer_concurrency_limit(self.config.consumer_concurrency)?;

        let server_config = GrpcServerConfig {
            max_receive_message_len: Some(self.config.max_receive_message_length),
            transport: self.config.server_transport.clone(),
        };

        Ok(Box::new(GrpcConsumer::new(
            self.host.clone(),
            self.port,
            path,
            self.proto_path.clone(),
            self.service_name.clone(),
            self.method_name.clone(),
            mode,
            rt,
            server_config,
            consumer_concurrency,
        )))
    }

    fn create_producer(
        &self,
        rt: Arc<dyn RuntimeObservability>,
        ctx: &ProducerContext,
    ) -> Result<BoxProcessor, CamelError> {
        let mode = resolve_grpc_mode(&self.proto_path, &self.service_name, &self.method_name)?;
        // NOTE: direction-aware outbound intent is already enforced at parse time
        // (parse_grpc_query_params rejects conflicting transport+cert combos) and
        // in GrpcProducer::new (ClientTransport::Tls hard-errors on bad config).
        // No extra validation needed here.
        // route_id may not be set in test scenarios (e.g., standalone endpoint tests)
        let route_id = ctx.route_id().unwrap_or("unknown");
        let producer = GrpcProducer::new(
            self.addr.clone(),
            self.proto_path.clone(),
            self.service_name.clone(),
            self.method_name.clone(),
            mode,
            self.deadline_ms,
            &self.config,
            rt,
            route_id,
        )?;
        Ok(BoxProcessor::new(producer))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use camel_component_api::{NoOpComponentContext, RuntimeObservability};

    use super::*;

    fn test_runtime() -> Arc<dyn RuntimeObservability> {
        Arc::new(NoOpComponentContext)
    }

    /// Regression: C2 — inbound transport=tls without server certs must error at create_consumer.
    /// Parse succeeds (defers to create_consumer), but create_consumer must hard-error with
    /// "serverCertPath" in the message.
    #[test]
    fn test_c2_inbound_tls_without_server_certs_errors() {
        // Construct a GrpcEndpoint directly with transport_intent=Tls but server_transport=Plaintext
        // This simulates the state after parsing "grpc://...?transport=tls" without server certs
        let proto_path =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/helloworld.proto");

        let config = GrpcConfig {
            proto_file: None,
            service: None,
            method: None,
            reflection: false,
            transport_intent: TransportIntent::Tls,
            client_transport: crate::config::ClientTransport::Plaintext,
            server_transport: ServerTransport::Plaintext, // No server certs
            max_receive_message_length: 4 * 1024 * 1024,
            deadline_ms: None,
            metadata: None,
            connect_timeout_ms: 10_000,
            default_deadline_ms: 30_000,
            auth: crate::config::AuthConfig::None,
            interceptors: crate::config::InterceptorConfig::default(),
            consumer_strategy: crate::config::ConsumerStrategy::default(),
            producer_strategy: crate::config::ProducerStrategy::default(),
            consumer_concurrency: 64,
            retry: camel_component_api::NetworkRetryPolicy::default(),
        };

        let endpoint = GrpcEndpoint {
            uri: "grpc://127.0.0.1:0/helloworld.Greeter/SayHello?transport=tls".to_string(),
            addr: "http://127.0.0.1:0".to_string(),
            host: "127.0.0.1".to_string(),
            port: 0,
            proto_path,
            service_name: "helloworld.Greeter".to_string(),
            method_name: "SayHello".to_string(),
            deadline_ms: None,
            config,
        };

        // Now call create_consumer — this MUST fail because transport_intent=Tls but
        // server_transport=Plaintext means no server certs were provided (C2 violation)
        let result = endpoint.create_consumer(test_runtime());

        assert!(
            result.is_err(),
            "C2: transport=tls without serverCertPath/serverKeyPath must fail at create_consumer"
        );
        match result {
            Err(e) => {
                let err = e.to_string();
                assert!(
                    err.contains("serverCertPath") || err.contains("serverKeyPath"),
                    "error must mention the missing server certs: {err}"
                );
            }
            Ok(_) => panic!("expected error, got Ok"),
        }
    }

    /// Regression (bd rc-9kgtm): an oversized consumerConcurrency must be rejected at
    /// create_consumer even when the endpoint is constructed directly (not via
    /// URI parse). Mirrors the C2 fixture pattern: direct GrpcEndpoint with
    /// proto_path from CARGO_MANIFEST_DIR, plaintext transport shape.
    #[test]
    fn test_create_consumer_rejects_oversized_direct_endpoint() {
        let l = tokio::sync::Semaphore::MAX_PERMITS;
        let proto_path =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/helloworld.proto");

        let config = GrpcConfig {
            proto_file: None,
            service: None,
            method: None,
            reflection: false,
            transport_intent: TransportIntent::Plaintext,
            client_transport: crate::config::ClientTransport::Plaintext,
            server_transport: ServerTransport::Plaintext,
            max_receive_message_length: 4 * 1024 * 1024,
            deadline_ms: None,
            metadata: None,
            connect_timeout_ms: 10_000,
            default_deadline_ms: 30_000,
            auth: crate::config::AuthConfig::None,
            interceptors: crate::config::InterceptorConfig::default(),
            consumer_strategy: crate::config::ConsumerStrategy::default(),
            producer_strategy: crate::config::ProducerStrategy::default(),
            consumer_concurrency: l + 1,
            retry: camel_component_api::NetworkRetryPolicy::default(),
        };

        let endpoint = GrpcEndpoint {
            uri: "grpc://127.0.0.1:0/helloworld.Greeter/SayHello".to_string(),
            addr: "http://127.0.0.1:0".to_string(),
            host: "127.0.0.1".to_string(),
            port: 0,
            proto_path,
            service_name: "helloworld.Greeter".to_string(),
            method_name: "SayHello".to_string(),
            deadline_ms: None,
            config,
        };

        // ACT: create_consumer must reject fail-closed with a typed config
        // error naming the parameter, the configured value, and the limit.
        let result = endpoint.create_consumer(test_runtime());

        match result {
            Err(CamelError::Config(msg)) => {
                assert!(
                    msg.contains("consumerConcurrency"),
                    "error must name the parameter: {msg}"
                );
                assert!(
                    msg.contains(&format!("{}", l + 1)),
                    "error must name the configured value: {msg}"
                );
                assert!(
                    msg.contains(&format!("{l}")),
                    "error must name the limit: {msg}"
                );
            }
            Err(other) => panic!("expected CamelError::Config, got {other:?}"),
            Ok(_) => panic!("expected error, got Ok"),
        }
    }
}
