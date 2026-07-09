use tonic::transport::{Certificate, Channel, ClientTlsConfig, Endpoint, Identity};

use crate::process::BridgeError;
use crate::tls::BridgeTlsMaterial;

/// Connect a tonic channel to localhost:{port} with mTLS.
///
/// Retries the TLS handshake a few times because the Quarkus PortAnnouncer
/// fires on `StartupEvent`, which can precede full SSL listener readiness
/// by a few hundred milliseconds in native images.
pub(crate) async fn connect_channel(
    port: u16,
    tls: &BridgeTlsMaterial,
) -> Result<Channel, BridgeError> {
    let uri = format!("https://127.0.0.1:{port}");
    let tls_config = ClientTlsConfig::new()
        .ca_certificate(Certificate::from_pem(&tls.ca_pem))
        .identity(Identity::from_pem(&tls.client_pem, &tls.client_key_pem))
        .domain_name("127.0.0.1");

    let endpoint = Endpoint::from_shared(uri)
        .map_err(|e| BridgeError::Transport(e.to_string()))?
        .tls_config(tls_config)
        .map_err(|e| BridgeError::Transport(e.to_string()))?;

    /// Retries because Quarkus PortAnnouncer fires on StartupEvent, which can
    /// precede full SSL listener readiness in native images by ~100ms.
    const MAX_CONNECT_RETRIES: u32 = 10;
    const RETRY_INTERVAL: std::time::Duration = std::time::Duration::from_millis(100);

    let mut last_err = None;
    for _ in 0..MAX_CONNECT_RETRIES {
        match endpoint.connect().await {
            Ok(channel) => return Ok(channel),
            Err(e) => {
                last_err = Some(e);
                tokio::time::sleep(RETRY_INTERVAL).await;
            }
        }
    }
    Err(BridgeError::Transport(
        last_err.map(|e| e.to_string()).unwrap_or_default(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tls_config_builds_with_valid_material() {
        let tls = BridgeTlsMaterial::generate().expect("generate");
        let _config = ClientTlsConfig::new()
            .ca_certificate(Certificate::from_pem(&tls.ca_pem))
            .identity(Identity::from_pem(&tls.client_pem, &tls.client_key_pem))
            .domain_name("127.0.0.1");
    }
}
