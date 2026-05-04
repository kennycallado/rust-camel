use thiserror::Error;

#[derive(Error, Debug)]
pub enum CxfError {
    #[error("SOAP fault: {code} — {message}")]
    Fault { code: String, message: String },
    #[error("WS-Security error: {0}")]
    Security(String),
    #[error("Transport error: {0}")]
    Transport(String),
    #[error("Bridge error: {0}")]
    Bridge(String),
    #[error("Timeout waiting for SOAP response")]
    Timeout,
    #[error("Configuration error: {0}")]
    Config(String),
    #[error("Consumer stream closed")]
    StreamClosed,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fault_display_includes_code_and_message() {
        let err = CxfError::Fault {
            code: "soap:Server".to_string(),
            message: "Internal error".to_string(),
        };
        let msg = err.to_string();
        assert!(msg.contains("soap:Server"));
        assert!(msg.contains("Internal error"));
    }

    #[test]
    fn security_display() {
        let err = CxfError::Security("token expired".to_string());
        assert!(err.to_string().contains("token expired"));
    }

    #[test]
    fn transport_display() {
        let err = CxfError::Transport("connection refused".to_string());
        assert!(err.to_string().contains("connection refused"));
    }

    #[test]
    fn bridge_display() {
        let err = CxfError::Bridge("process crashed".to_string());
        assert!(err.to_string().contains("process crashed"));
    }

    #[test]
    fn timeout_display() {
        let err = CxfError::Timeout;
        let msg = err.to_string().to_lowercase();
        assert!(msg.contains("timeout"));
    }

    #[test]
    fn config_display() {
        let err = CxfError::Config("missing wsdl_path".to_string());
        assert!(err.to_string().contains("missing wsdl_path"));
    }

    #[test]
    fn stream_closed_display() {
        let err = CxfError::StreamClosed;
        let msg = err.to_string().to_lowercase();
        assert!(msg.contains("stream"));
    }

    #[test]
    fn error_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<CxfError>();
    }
}
