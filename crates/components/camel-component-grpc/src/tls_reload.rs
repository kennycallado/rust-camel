use std::sync::Arc;

use arc_swap::ArcSwap;
use async_trait::async_trait;
use camel_api::CamelError;
use camel_component_api::tls_source::{ServerTlsSource, TlsReloadHandler};
use tokio_rustls::TlsAcceptor;

pub(crate) struct GrpcReloadHandler {
    pub(crate) acceptor: Arc<ArcSwap<TlsAcceptor>>,
    pub(crate) source: ServerTlsSource,
    scheme: String,
    host: String,
    port: u16,
}

impl GrpcReloadHandler {
    pub(crate) fn new(
        acceptor: Arc<ArcSwap<TlsAcceptor>>,
        source: ServerTlsSource,
        host: String,
        port: u16,
    ) -> Self {
        Self {
            acceptor,
            source,
            scheme: "grpcs".into(),
            host,
            port,
        }
    }
}

#[async_trait]
impl TlsReloadHandler for GrpcReloadHandler {
    fn matches(&self, scheme: &str, host: &str, port: u16) -> bool {
        self.scheme == scheme && self.host == host && self.port == port
    }

    async fn reload(&self) -> Result<(), CamelError> {
        match self.source.build_server_config() {
            Ok(mut server_cfg) => {
                server_cfg.alpn_protocols = vec![b"h2".to_vec()];
                let new_acceptor = TlsAcceptor::from(Arc::new(server_cfg));
                self.acceptor.store(Arc::new(new_acceptor));
                tracing::info!(
                    scheme = %self.scheme,
                    host = %self.host,
                    port = self.port,
                    "gRPC TLS cert reloaded"
                );
                Ok(())
            }
            Err(e) => {
                tracing::warn!(
                    scheme = %self.scheme,
                    host = %self.host,
                    port = self.port,
                    error = %e,
                    "gRPC TLS cert reload failed — keeping old cert"
                );
                Err(e)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_component_api::test_support::tls;

    #[tokio::test]
    async fn grpc_tls_reload_handler_swaps_valid_cert() {
        let (cert_pem, key_pem) = {
            let (_ca, c, k) = tls::gen_server_cert();
            (c, k)
        };
        let cert_path = tls::write_pem_tmp("grpc-reload-cert-1.pem", &cert_pem);
        let key_path = tls::write_pem_tmp("grpc-reload-key-1.pem", &key_pem);

        let source = ServerTlsSource {
            cert_path: cert_path.clone(),
            key_path: key_path.clone(),
            client_ca_path: None,
        };
        let mut cfg = source.build_server_config().expect("initial cert");
        cfg.alpn_protocols = vec![b"h2".to_vec()];
        let acceptor = Arc::new(ArcSwap::from_pointee(TlsAcceptor::from(Arc::new(cfg))));

        let handler = GrpcReloadHandler::new(acceptor.clone(), source, "0.0.0.0".into(), 9999);

        // Before reload: capture the old acceptor identity
        let old_ptr = Arc::as_ptr(&handler.acceptor.load_full());

        // Generate new cert, overwrite files
        let (_ca2, cert2, key2) = tls::gen_server_cert();
        std::fs::write(&handler.source.cert_path, &cert2).unwrap();
        std::fs::write(&handler.source.key_path, &key2).unwrap();

        handler.reload().await.expect("reload should succeed");

        // After reload: verify the ArcSwap stored a new pointer
        let new_ptr = Arc::as_ptr(&handler.acceptor.load_full());
        assert_ne!(
            old_ptr, new_ptr,
            "ArcSwap must have swapped to the new cert"
        );
    }

    #[tokio::test]
    async fn grpc_tls_reload_handler_preserves_old_cert_on_error() {
        let (cert_pem, key_pem) = {
            let (_ca, c, k) = tls::gen_server_cert();
            (c, k)
        };
        let cert_path = tls::write_pem_tmp("grpc-reload-bad-cert.pem", &cert_pem);
        let key_path = tls::write_pem_tmp("grpc-reload-bad-key.pem", &key_pem);

        let source = ServerTlsSource {
            cert_path: cert_path.clone(),
            key_path: key_path.clone(),
            client_ca_path: None,
        };
        let mut cfg = source.build_server_config().unwrap();
        cfg.alpn_protocols = vec![b"h2".to_vec()];
        let acceptor = Arc::new(ArcSwap::from_pointee(TlsAcceptor::from(Arc::new(cfg))));

        // Overwrite with garbage
        std::fs::write(&cert_path, "GARBAGE").unwrap();

        let handler = GrpcReloadHandler::new(acceptor, source, "0.0.0.0".into(), 9998);
        let old_ptr = Arc::as_ptr(&handler.acceptor.load_full());
        let result = handler.reload().await;
        assert!(result.is_err(), "bad cert should fail reload");

        // ArcSwap must NOT have been swapped on error — pointer identity unchanged.
        let new_ptr = Arc::as_ptr(&handler.acceptor.load_full());
        assert_eq!(old_ptr, new_ptr, "ArcSwap must NOT swap when reload fails");
    }

    #[test]
    fn grpc_tls_reload_handler_matches_correctly() {
        let (cert_pem, key_pem) = {
            let (_ca, c, k) = tls::gen_server_cert();
            (c, k)
        };
        let cert_path = tls::write_pem_tmp("grpc-match-cert.pem", &cert_pem);
        let key_path = tls::write_pem_tmp("grpc-match-key.pem", &key_pem);

        let source = ServerTlsSource {
            cert_path,
            key_path,
            client_ca_path: None,
        };
        let mut cfg = source.build_server_config().unwrap();
        cfg.alpn_protocols = vec![b"h2".to_vec()];
        let acceptor = Arc::new(ArcSwap::from_pointee(TlsAcceptor::from(Arc::new(cfg))));

        let handler = GrpcReloadHandler::new(acceptor, source, "0.0.0.0".into(), 9090);
        assert!(handler.matches("grpcs", "0.0.0.0", 9090));
        assert!(!handler.matches("grpc", "0.0.0.0", 9090));
        assert!(!handler.matches("grpcs", "127.0.0.1", 9090));
        assert!(!handler.matches("grpcs", "0.0.0.0", 8080));
    }
}
