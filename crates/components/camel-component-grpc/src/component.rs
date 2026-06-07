use std::path::PathBuf;
use std::sync::Arc;

use camel_component_api::{
    BoxProcessor, CamelError, Component, ComponentContext, Consumer, Endpoint, ProducerContext,
    RuntimeObservability,
};

use crate::config::{GrpcConfig, parse_grpc_uri};
use crate::consumer::{GrpcConsumer, resolve_grpc_mode};
use crate::health::GrpcHealthCheck;
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
        let path = format!("/{}/{}", self.service_name, self.method_name);
        let mode = resolve_grpc_mode(&self.proto_path, &self.service_name, &self.method_name)?;
        Ok(Box::new(GrpcConsumer::new(
            self.host.clone(),
            self.port,
            path,
            self.proto_path.clone(),
            self.service_name.clone(),
            self.method_name.clone(),
            mode,
            rt,
        )))
    }

    fn create_producer(
        &self,
        rt: Arc<dyn RuntimeObservability>,
        ctx: &ProducerContext,
    ) -> Result<BoxProcessor, CamelError> {
        let mode = resolve_grpc_mode(&self.proto_path, &self.service_name, &self.method_name)?;
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
