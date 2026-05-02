use std::path::PathBuf;

use camel_component_api::{
    BoxProcessor, CamelError, Component, ComponentContext, Consumer, Endpoint, ProducerContext,
};

use crate::config::parse_grpc_uri;
use crate::consumer::{resolve_grpc_mode, GrpcConsumer};
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
        _ctx: &dyn ComponentContext,
    ) -> Result<Box<dyn Endpoint>, CamelError> {
        let (host, port, service_name, method_name, cfg) = parse_grpc_uri(uri)?;
        let proto_file = cfg.proto_file.ok_or_else(|| {
            CamelError::EndpointCreationFailed(
                "missing required query parameter: protoFile".to_string(),
            )
        })?;
        let addr = format!("http://{host}:{port}");
        Ok(Box::new(GrpcEndpoint {
            uri: uri.to_string(),
            addr,
            host,
            port,
            proto_path: PathBuf::from(proto_file),
            service_name,
            method_name,
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
}

impl Endpoint for GrpcEndpoint {
    fn uri(&self) -> &str {
        &self.uri
    }

    fn create_consumer(&self) -> Result<Box<dyn Consumer>, CamelError> {
        let path = format!("/{}/{}", self.service_name, self.method_name);
        let mode = resolve_grpc_mode(
            &self.proto_path,
            &self.service_name,
            &self.method_name,
        )?;
        Ok(Box::new(GrpcConsumer::new(
            self.host.clone(),
            self.port,
            path,
            self.proto_path.clone(),
            self.service_name.clone(),
            self.method_name.clone(),
            mode,
        )))
    }

    fn create_producer(&self, _ctx: &ProducerContext) -> Result<BoxProcessor, CamelError> {
        let mode = resolve_grpc_mode(
            &self.proto_path,
            &self.service_name,
            &self.method_name,
        )?;
        let producer = GrpcProducer::new(
            self.addr.clone(),
            self.proto_path.clone(),
            self.service_name.clone(),
            self.method_name.clone(),
            mode,
        )?;
        Ok(BoxProcessor::new(producer))
    }
}
