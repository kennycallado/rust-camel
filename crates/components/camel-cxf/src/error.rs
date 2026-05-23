use thiserror::Error;

#[derive(Error, Debug)]
pub enum CxfError {
    #[error("SOAP fault: {code} — {message} (endpoint: {endpoint})")]
    Fault {
        code: String,
        message: String,
        endpoint: String,
    },
    #[error("WS-Security error: {message} (operation: {operation})")]
    Security { message: String, operation: String },
    #[error("Transport error: {message} (endpoint: {endpoint})")]
    Transport {
        message: String,
        endpoint: String,
        #[source]
        source: Option<tonic::Status>,
    },
    #[error("Bridge error: {message} (slot: {slot_key})")]
    Bridge {
        message: String,
        slot_key: String,
        #[source]
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    },
    #[error("Timeout waiting for SOAP response (operation: {operation}, endpoint: {endpoint})")]
    Timeout { operation: String, endpoint: String },
    #[error("Configuration error: {0}")]
    Config(String),
    #[error("Consumer stream closed (endpoint: {endpoint})")]
    StreamClosed { endpoint: String },
    #[error("Streaming body not supported by CXF component (endpoint: {endpoint})")]
    StreamBody { endpoint: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fault_display_includes_code_and_message() {
        let err = CxfError::Fault {
            code: "soap:Server".to_string(),
            message: "Internal error".to_string(),
            endpoint: "http://localhost:8080/svc".to_string(),
        };
        let msg = err.to_string();
        assert!(msg.contains("soap:Server"));
        assert!(msg.contains("Internal error"));
        assert!(msg.contains("http://localhost:8080/svc"));
    }

    #[test]
    fn security_display() {
        let err = CxfError::Security {
            message: "token expired".to_string(),
            operation: "sayHello".to_string(),
        };
        assert!(err.to_string().contains("token expired"));
        assert!(err.to_string().contains("sayHello"));
    }

    #[test]
    fn transport_display() {
        let err = CxfError::Transport {
            message: "connection refused".to_string(),
            endpoint: "http://localhost:8080/svc".to_string(),
            source: None,
        };
        assert!(err.to_string().contains("connection refused"));
        assert!(err.to_string().contains("http://localhost:8080/svc"));
    }

    #[test]
    fn transport_preserves_source() {
        let status = tonic::Status::unavailable("channel closed");
        let err = CxfError::Transport {
            message: "connection lost".to_string(),
            endpoint: "grpc://127.0.0.1:50051".to_string(),
            source: Some(status),
        };
        let source = std::error::Error::source(&err);
        assert!(
            source.is_some(),
            "Transport error should preserve source chain"
        );
        let source_msg = source.unwrap().to_string();
        assert!(
            source_msg.contains("channel closed"),
            "source chain should contain original message, got: {source_msg}"
        );
    }

    #[test]
    fn bridge_display() {
        let err = CxfError::Bridge {
            message: "process crashed".to_string(),
            slot_key: "cxf".to_string(),
            source: None,
        };
        assert!(err.to_string().contains("process crashed"));
        assert!(err.to_string().contains("cxf"));
    }

    #[test]
    fn bridge_preserves_source() {
        let inner = std::io::Error::new(std::io::ErrorKind::BrokenPipe, "broken pipe");
        let err = CxfError::Bridge {
            message: "start failed".to_string(),
            slot_key: "cxf".to_string(),
            source: Some(Box::new(inner)),
        };
        let source = std::error::Error::source(&err);
        assert!(
            source.is_some(),
            "Bridge error should preserve source chain"
        );
        let source_msg = source.unwrap().to_string();
        assert!(
            source_msg.contains("broken pipe"),
            "source chain should contain original message, got: {source_msg}"
        );
    }

    #[test]
    fn timeout_display() {
        let err = CxfError::Timeout {
            operation: "sayHello".to_string(),
            endpoint: "http://localhost:8080/svc".to_string(),
        };
        let msg = err.to_string().to_lowercase();
        assert!(msg.contains("timeout"));
        assert!(msg.contains("sayhello"));
    }

    #[test]
    fn config_display() {
        let err = CxfError::Config("missing wsdl_path".to_string());
        assert!(err.to_string().contains("missing wsdl_path"));
    }

    #[test]
    fn stream_closed_display() {
        let err = CxfError::StreamClosed {
            endpoint: "http://localhost:8080/svc".to_string(),
        };
        let msg = err.to_string().to_lowercase();
        assert!(msg.contains("stream"));
        assert!(msg.contains("http://localhost:8080/svc"));
    }

    #[test]
    fn stream_body_display() {
        let err = CxfError::StreamBody {
            endpoint: "http://localhost:8080/svc".to_string(),
        };
        let msg = err.to_string().to_lowercase();
        assert!(msg.contains("streaming body not supported"));
        assert!(msg.contains("http://localhost:8080/svc"));
    }

    #[test]
    fn error_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<CxfError>();
    }
}
