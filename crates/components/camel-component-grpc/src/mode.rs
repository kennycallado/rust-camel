use prost_reflect::MethodDescriptor;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GrpcMode {
    Unary,
    ServerStreaming,
    ClientStreaming,
    Bidi,
}

impl GrpcMode {
    pub fn from_method(method: &MethodDescriptor) -> Self {
        match (method.is_server_streaming(), method.is_client_streaming()) {
            (false, false) => GrpcMode::Unary,
            (true, false) => GrpcMode::ServerStreaming,
            (false, true) => GrpcMode::ClientStreaming,
            (true, true) => GrpcMode::Bidi,
        }
    }
}
