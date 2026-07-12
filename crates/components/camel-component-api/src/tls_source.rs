//! Shared [`ServerTlsSource`] — builds a validated [`rustls::ServerConfig`] from
//! PEM cert/key files, optionally with mTLS client CA verification.
//!
//! This deduplicates ~40 lines of PEM parsing that each of gRPC, HTTP, and WS
//! components implement separately. The individual components still own ALPN
//! configuration — `build_server_config` intentionally leaves ALPN empty so the
//! caller can set it after construction.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use camel_api::CamelError;
use rustls::ServerConfig;

/// File-based TLS source for rustls server-side configuration.
///
/// Reads PEM-encoded cert+key from the filesystem. When `client_ca_path` is
/// set, the server requires (and verifies) a client certificate (mTLS).
#[derive(Debug, Clone)]
pub struct ServerTlsSource {
    /// Path to the PEM-encoded server certificate chain.
    pub cert_path: PathBuf,
    /// Path to the PEM-encoded private key for the server certificate.
    pub key_path: PathBuf,
    /// Optional path to a PEM-encoded CA certificate for client verification.
    /// When set, mTLS is enabled (clients must present a cert signed by this CA).
    pub client_ca_path: Option<PathBuf>,
}

impl ServerTlsSource {
    /// Build a [`rustls::ServerConfig`] from the PEM files.
    ///
    /// Returns `EndpointCreationFailed` on any I/O, PEM parse, or rustls
    /// configuration error.
    ///
    /// ALPN protocols are intentionally NOT set — callers add their own
    /// (e.g., `b"h2"` for gRPC) after construction.
    pub fn build_server_config(&self) -> Result<ServerConfig, CamelError> {
        // Read cert and key files as raw bytes.
        let cert_pem = std::fs::read(&self.cert_path).map_err(|e| {
            CamelError::EndpointCreationFailed(format!(
                "TLS cert file '{}': {e}",
                self.cert_path.display()
            ))
        })?;
        let key_pem = std::fs::read(&self.key_path).map_err(|e| {
            CamelError::EndpointCreationFailed(format!(
                "TLS key file '{}': {e}",
                self.key_path.display()
            ))
        })?;

        // Parse PEM certs.
        let certs: Vec<_> = rustls_pemfile::certs(&mut cert_pem.as_slice())
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| {
                CamelError::EndpointCreationFailed(format!("TLS cert parse error: {e}"))
            })?;

        // Parse PEM private key.
        let key = rustls_pemfile::private_key(&mut key_pem.as_slice())
            .map_err(|e| CamelError::EndpointCreationFailed(format!("TLS key parse error: {e}")))?
            .ok_or_else(|| {
                CamelError::EndpointCreationFailed("TLS: no private key found in key PEM".into())
            })?;

        // Explicit ring provider — rustls 0.23 requires this (the default
        // `ServerConfig::builder()` panics at runtime without a process-default).
        let provider = Arc::new(rustls::crypto::ring::default_provider());

        // Build the protocol-version-safe builder.
        let builder = rustls::ServerConfig::builder_with_provider(Arc::clone(&provider))
            .with_safe_default_protocol_versions()
            .map_err(|e| {
                CamelError::EndpointCreationFailed(format!("rustls protocol versions: {e}"))
            })?;

        // Branch: mTLS (client cert verification) or plain server auth.
        let config = match &self.client_ca_path {
            Some(ca_path) => {
                let ca_pem = std::fs::read(ca_path).map_err(|e| {
                    CamelError::EndpointCreationFailed(format!(
                        "TLS client CA file '{}': {e}",
                        ca_path.display()
                    ))
                })?;
                let ca_certs: Vec<_> = rustls_pemfile::certs(&mut ca_pem.as_slice())
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|e| {
                        CamelError::EndpointCreationFailed(format!("invalid client CA PEM: {e}"))
                    })?;
                let mut roots = rustls::RootCertStore::empty();
                for cert in ca_certs {
                    roots.add(cert).map_err(|e| {
                        CamelError::EndpointCreationFailed(format!("client CA root add: {e}"))
                    })?;
                }
                let verifier = rustls::server::WebPkiClientVerifier::builder_with_provider(
                    Arc::new(roots),
                    Arc::clone(&provider),
                )
                .build()
                .map_err(|e| {
                    CamelError::EndpointCreationFailed(format!("mTLS client cert verifier: {e}"))
                })?;
                builder
                    .with_client_cert_verifier(verifier)
                    .with_single_cert(certs, key)
                    .map_err(|e| {
                        CamelError::EndpointCreationFailed(format!("rustls ServerConfig: {e}"))
                    })?
            }
            None => builder
                .with_no_client_auth()
                .with_single_cert(certs, key)
                .map_err(|e| {
                    CamelError::EndpointCreationFailed(format!("rustls ServerConfig: {e}"))
                })?,
        };

        Ok(config)
    }
}

// ---------------------------------------------------------------------------
// TlsReloadHandler + TlsReloadRegistry
// ---------------------------------------------------------------------------

/// Implemented by each TLS-terminating component.
/// Registered with [`TlsReloadRegistry::global()`] when a server first spawns.
#[async_trait]
pub trait TlsReloadHandler: Send + Sync {
    /// Returns true if this handler owns the (scheme, host, port).
    fn matches(&self, scheme: &str, host: &str, port: u16) -> bool;
    /// Re-read cert files and swap. Returns Err if cert invalid; old cert stays.
    async fn reload(&self) -> Result<(), CamelError>;
}

/// Process-global singleton registry of TLS reload handlers.
///
/// Provides a singleton via [`TlsReloadRegistry::global()`]. Each TLS-terminating
/// component registers its handler when it first spawns, and unregisters on
/// release/eviction.
#[derive(Default)]
pub struct TlsReloadRegistry {
    handlers: Mutex<Vec<Arc<dyn TlsReloadHandler>>>,
}

impl TlsReloadRegistry {
    /// Returns a reference to the process-global singleton.
    pub fn global() -> &'static TlsReloadRegistry {
        static INSTANCE: std::sync::OnceLock<TlsReloadRegistry> = std::sync::OnceLock::new();
        INSTANCE.get_or_init(TlsReloadRegistry::default)
    }

    /// Register a handler so it can be found later via [`find`](Self::find).
    pub fn register(&self, handler: Arc<dyn TlsReloadHandler>) {
        let mut guard = self
            .handlers
            .lock()
            .expect("TlsReloadRegistry lock poisoned");
        guard.push(handler);
    }

    /// Find the handler that matches (scheme, host, port), if any.
    pub fn find(&self, scheme: &str, host: &str, port: u16) -> Option<Arc<dyn TlsReloadHandler>> {
        let guard = self
            .handlers
            .lock()
            .expect("TlsReloadRegistry lock poisoned");
        guard
            .iter()
            .find(|h| h.matches(scheme, host, port))
            .cloned()
    }

    /// Remove handlers matching (scheme, host, port). Called on server release/eviction.
    pub fn unregister(&self, scheme: &str, host: &str, port: u16) {
        let mut guard = self
            .handlers
            .lock()
            .expect("TlsReloadRegistry lock poisoned");
        guard.retain(|h| !h.matches(scheme, host, port));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: write a PEM string to a temp file and return the path.
    fn write_pem(pem: &str, name: &str) -> PathBuf {
        crate::test_support::tls::write_pem_tmp(name, pem)
    }

    #[test]
    fn build_server_config_valid_cert() {
        let (ca_pem, cert_pem, key_pem) = crate::test_support::tls::gen_server_cert();
        let _ca = write_pem(&ca_pem, "ca.pem");
        let cert = write_pem(&cert_pem, "cert.pem");
        let key = write_pem(&key_pem, "key.pem");

        let source = ServerTlsSource {
            cert_path: cert,
            key_path: key,
            client_ca_path: None,
        };

        let config = source.build_server_config().expect("valid cert+key");
        assert!(config.alpn_protocols.is_empty(), "ALPN should be empty");
    }

    #[test]
    fn build_server_config_mtls() {
        let (ca_pem, cert_pem, key_pem) = crate::test_support::tls::gen_server_cert();
        let ca = write_pem(&ca_pem, "mtls-ca.pem");
        let cert = write_pem(&cert_pem, "cert.pem");
        let key = write_pem(&key_pem, "key.pem");

        let source = ServerTlsSource {
            cert_path: cert,
            key_path: key,
            client_ca_path: Some(ca),
        };

        let config = source.build_server_config().expect("mTLS should work");
        assert!(config.alpn_protocols.is_empty());
        // mTLS is enabled — verifier should require client cert.
        // We can't easily assert the verifier type, but we can verify
        // the config uses a client-cert verifier by checking it was
        // built without errors.
    }

    #[test]
    fn build_server_config_mtls_bad_ca_rejected() {
        let (_, cert_pem, key_pem) = crate::test_support::tls::gen_server_cert();
        let cert = write_pem(&cert_pem, "cert.pem");
        let key = write_pem(&key_pem, "key.pem");

        // Write garbage as the CA PEM.
        let bad_ca = write_pem("not-a-valid-ca-certificate\n", "bad-ca.pem");

        let source = ServerTlsSource {
            cert_path: cert,
            key_path: key,
            client_ca_path: Some(bad_ca),
        };

        let err = source.build_server_config().unwrap_err();
        assert!(
            matches!(&err, CamelError::EndpointCreationFailed(_)),
            "expected EndpointCreationFailed, got {err:?}"
        );
    }

    #[test]
    fn build_server_config_missing_file() {
        let source = ServerTlsSource {
            cert_path: PathBuf::from("/nonexistent/cert.pem"),
            key_path: PathBuf::from("/nonexistent/key.pem"),
            client_ca_path: None,
        };

        let err = source.build_server_config().unwrap_err();
        assert!(
            matches!(&err, CamelError::EndpointCreationFailed(_)),
            "expected EndpointCreationFailed, got {err:?}"
        );
    }

    #[test]
    fn build_server_config_malformed_pem() {
        let cert = write_pem("garbage-content\n", "bad-cert.pem");
        let key = write_pem("also-garbage\n", "bad-key.pem");

        let source = ServerTlsSource {
            cert_path: cert,
            key_path: key,
            client_ca_path: None,
        };

        let err = source.build_server_config().unwrap_err();
        assert!(
            matches!(&err, CamelError::EndpointCreationFailed(_)),
            "expected EndpointCreationFailed, got {err:?}"
        );
    }
}

#[cfg(test)]
mod registry_tests {
    use super::*;

    struct FakeHandler {
        scheme: String,
        host: String,
        port: u16,
    }

    #[async_trait::async_trait]
    impl TlsReloadHandler for FakeHandler {
        fn matches(&self, scheme: &str, host: &str, port: u16) -> bool {
            self.scheme == scheme && self.host == host && self.port == port
        }
        async fn reload(&self) -> Result<(), CamelError> {
            Ok(())
        }
    }

    #[test]
    fn registry_find_returns_matching_handler() {
        let reg = TlsReloadRegistry::default();
        reg.register(Arc::new(FakeHandler {
            scheme: "grpcs".into(),
            host: "0.0.0.0".into(),
            port: 9090,
        }));
        assert!(reg.find("grpcs", "0.0.0.0", 9090).is_some());
        assert!(reg.find("https", "0.0.0.0", 9090).is_none());
    }

    #[test]
    fn registry_find_returns_none_when_empty() {
        let reg = TlsReloadRegistry::default();
        assert!(reg.find("grpcs", "0.0.0.0", 9090).is_none());
    }

    #[test]
    fn registry_unregister_removes_handler() {
        let reg = TlsReloadRegistry::default();
        reg.register(Arc::new(FakeHandler {
            scheme: "grpcs".into(),
            host: "0.0.0.0".into(),
            port: 9090,
        }));
        assert!(reg.find("grpcs", "0.0.0.0", 9090).is_some());
        reg.unregister("grpcs", "0.0.0.0", 9090);
        assert!(reg.find("grpcs", "0.0.0.0", 9090).is_none());
    }
}
