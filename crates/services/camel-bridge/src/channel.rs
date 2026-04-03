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
    #[test]
    fn uri_format_is_correct() {
        let port: u16 = 50051;
        let uri = format!("http://127.0.0.1:{port}");
        assert_eq!(uri, "http://127.0.0.1:50051");
    }
}
