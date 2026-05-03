use tonic::transport::{Channel, Endpoint};

use crate::process::BridgeError;

/// Connect a tonic channel to localhost:{port}.
pub async fn connect_channel(port: u16) -> Result<Channel, BridgeError> {
    let uri = format!("http://127.0.0.1:{port}");
    let channel = Endpoint::from_shared(uri)
        .map_err(|e| BridgeError::Transport(e.to_string()))?
        .connect()
        .await
        .map_err(|e| BridgeError::Transport(e.to_string()))?;
    Ok(channel)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uri_format_is_correct() {
        let port: u16 = 50051;
        let uri = format!("http://127.0.0.1:{port}");
        assert_eq!(uri, "http://127.0.0.1:50051");
    }

    #[tokio::test]
    async fn connect_channel_invalid_address_returns_error() {
        let result = connect_channel(1).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            BridgeError::Transport(_) => {}
            other => panic!("expected Transport error, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn connect_channel_unreachable_port_returns_error() {
        let result = connect_channel(65535).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            BridgeError::Transport(_) => {}
            other => panic!("expected Transport error, got: {other:?}"),
        }
    }

    #[test]
    fn connect_channel_valid_port_zero_uri() {
        let port: u16 = 0;
        let uri = format!("http://127.0.0.1:{port}");
        assert_eq!(uri, "http://127.0.0.1:0");
    }
}
