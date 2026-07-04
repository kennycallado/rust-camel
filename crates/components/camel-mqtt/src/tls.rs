// TLS configuration for MQTT connections (rustls).
// This module is feature-gated in `lib.rs` via `#[cfg(feature = "tls")]`.

use camel_api::CamelError;

/// Build a rumqttc TLS configuration.
///
/// - `Some(path)`: load the CA cert from the file and use `TlsConfiguration::Simple`.
/// - `None`: use `TlsConfiguration::default()` (platform/native root certs via rustls).
///
/// Only available with the `tls` feature (the module itself is feature-gated in `lib.rs`).
pub fn build_tls_config(
    ca_cert_path: Option<&str>,
) -> Result<rumqttc::TlsConfiguration, CamelError> {
    use rumqttc::TlsConfiguration;

    if let Some(path) = ca_cert_path {
        let ca = std::fs::read(path).map_err(|e| {
            CamelError::Config(format!("mqtt tls: cannot read ca_cert {path}: {e}"))
        })?;
        Ok(TlsConfiguration::Simple {
            ca,
            alpn: None,
            client_auth: None,
        })
    } else {
        // Use native/webpki roots via rustls.
        Ok(TlsConfiguration::default())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    #[cfg(feature = "tls")]
    fn build_tls_config_with_no_ca_cert() {
        // rustls 0.23 requires a process-level CryptoProvider.
        let _ = rustls::crypto::ring::default_provider().install_default();
        // Relies on a readable host cert store: `TlsConfiguration::default()` calls
        // `load_native_certs().expect(...)` internally, so this panics (not fails
        // cleanly) on a minimal CI image lacking ca-certificates.
        let result = super::build_tls_config(None);
        assert!(result.is_ok());
    }

    #[test]
    #[cfg(feature = "tls")]
    fn build_tls_config_rejects_missing_ca_file() {
        let result = super::build_tls_config(Some("/nonexistent/ca.pem"));
        assert!(result.is_err());
    }
}
