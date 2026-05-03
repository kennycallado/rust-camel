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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_grpc_mode_variants_exist() {
        let _ = GrpcMode::Unary;
        let _ = GrpcMode::ServerStreaming;
        let _ = GrpcMode::ClientStreaming;
        let _ = GrpcMode::Bidi;
    }

    #[test]
    fn test_grpc_mode_equality() {
        assert_eq!(GrpcMode::Unary, GrpcMode::Unary);
        assert_ne!(GrpcMode::Unary, GrpcMode::Bidi);
        assert_eq!(GrpcMode::ServerStreaming, GrpcMode::ServerStreaming);
        assert_ne!(GrpcMode::ClientStreaming, GrpcMode::ServerStreaming);
    }

    #[test]
    fn test_grpc_mode_copy_and_clone() {
        let mode = GrpcMode::Unary;
        let copied = mode;
        let cloned = mode.clone();
        assert_eq!(mode, copied);
        assert_eq!(mode, cloned);
    }

    #[test]
    fn test_grpc_mode_debug() {
        let debug_unary = format!("{:?}", GrpcMode::Unary);
        assert!(debug_unary.contains("Unary"));

        let debug_bidi = format!("{:?}", GrpcMode::Bidi);
        assert!(debug_bidi.contains("Bidi"));
    }

    #[test]
    fn test_grpc_mode_all_variants_distinct() {
        let modes = [
            GrpcMode::Unary,
            GrpcMode::ServerStreaming,
            GrpcMode::ClientStreaming,
            GrpcMode::Bidi,
        ];
        for (i, a) in modes.iter().enumerate() {
            for (j, b) in modes.iter().enumerate() {
                if i == j {
                    assert_eq!(a, b);
                } else {
                    assert_ne!(a, b);
                }
            }
        }
    }
}
