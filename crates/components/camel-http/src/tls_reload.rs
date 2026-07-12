use std::sync::Arc;

use async_trait::async_trait;
use camel_api::CamelError;
use camel_component_api::tls_source::{ServerTlsSource, TlsReloadHandler};

/// Handles TLS cert hot-reload for HTTP servers.
///
/// Registered when a TLS HTTP server spawns. On reload, reads the PEM files
/// via [`ServerTlsSource`] and calls [`RustlsConfig::reload_from_config`] to
/// atomically swap the internal config. Old connections keep their snapshot;
/// new connections use the updated cert.
///
/// # Lifecycle
///
/// 1. Created in [`ServerRegistry::get_or_spawn`] when a TLS endpoint starts.
/// 2. Registered with [`TlsReloadRegistry::global()`] for external trigger.
/// 3. Process-lifetime: no unregister call site — HTTP servers are not
///    released/evicted, so the handler stays registered for the lifetime of
///    the process.
pub(crate) struct HttpReloadHandler {
    tls_config: axum_server::tls_rustls::RustlsConfig,
    source: ServerTlsSource,
    scheme: String,
    host: String,
    port: u16,
}

impl HttpReloadHandler {
    pub(crate) fn new(
        tls_config: axum_server::tls_rustls::RustlsConfig,
        source: ServerTlsSource,
        host: String,
        port: u16,
    ) -> Self {
        Self {
            tls_config,
            source,
            scheme: "https".into(),
            host,
            port,
        }
    }
}

#[async_trait]
impl TlsReloadHandler for HttpReloadHandler {
    fn matches(&self, scheme: &str, host: &str, port: u16) -> bool {
        self.scheme == scheme && self.host == host && self.port == port
    }

    async fn reload(&self) -> Result<(), CamelError> {
        match self.source.build_server_config() {
            Ok(server_cfg) => {
                // reload_from_config is sync and infallible — it replaces
                // the internal ArcSwap. Old connections keep their snapshot;
                // new connections use the new cert.
                self.tls_config.reload_from_config(Arc::new(server_cfg));
                tracing::info!(
                    scheme = %self.scheme,
                    host = %self.host,
                    port = self.port,
                    "HTTP TLS cert reloaded"
                );
                Ok(())
            }
            Err(e) => {
                tracing::warn!(
                    scheme = %self.scheme,
                    host = %self.host,
                    port = self.port,
                    error = %e,
                    "HTTP TLS cert reload failed — keeping old cert"
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
    async fn http_tls_reload_handler_swaps_valid_cert() {
        // Note: RustlsConfig (axum-server) does not expose its internal ArcSwap
        // pointer, so we can't directly verify pointer identity here. The swap
        // is verified by the gRPC reload tests (which use the same ArcSwap
        // pattern with an exposed pointer) and by the RuntimeBus dispatch
        // tests (which exercise the end-to-end ReloadTlsCerts command path).
        let (cert_pem, key_pem) = {
            let (_ca, c, k) = tls::gen_server_cert();
            (c, k)
        };
        let cert_path = tls::write_pem_tmp("http-reload-cert-1.pem", &cert_pem);
        let key_path = tls::write_pem_tmp("http-reload-key-1.pem", &key_pem);

        let source = ServerTlsSource {
            cert_path: cert_path.clone(),
            key_path: key_path.clone(),
            client_ca_path: None,
        };
        let cfg = source.build_server_config().expect("initial cert");
        // HTTP doesn't use ALPN h2 — leave default
        let rustls_cfg = axum_server::tls_rustls::RustlsConfig::from_config(Arc::new(cfg));

        let handler = HttpReloadHandler::new(rustls_cfg, source, "0.0.0.0".into(), 8080);

        // Overwrite with new cert
        let (_ca2, cert2, key2) = tls::gen_server_cert();
        std::fs::write(&handler.source.cert_path, &cert2).unwrap();
        std::fs::write(&handler.source.key_path, &key2).unwrap();

        handler.reload().await.expect("reload should succeed");
    }

    #[tokio::test]
    async fn http_tls_reload_handler_fails_on_bad_cert() {
        // Note: pointer-identity verification for the on-error path is done
        // in the gRPC reload tests (which use an exposed ArcSwap pointer).
        let (cert_pem, key_pem) = {
            let (_ca, c, k) = tls::gen_server_cert();
            (c, k)
        };
        let cert_path = tls::write_pem_tmp("http-reload-bad-cert.pem", &cert_pem);
        let key_path = tls::write_pem_tmp("http-reload-bad-key.pem", &key_pem);

        let source = ServerTlsSource {
            cert_path: cert_path.clone(),
            key_path: key_path.clone(),
            client_ca_path: None,
        };
        let cfg = source.build_server_config().unwrap();
        let rustls_cfg = axum_server::tls_rustls::RustlsConfig::from_config(Arc::new(cfg));

        // Overwrite with garbage
        std::fs::write(&cert_path, "GARBAGE").unwrap();

        let handler = HttpReloadHandler::new(rustls_cfg, source, "0.0.0.0".into(), 8081);
        let result = handler.reload().await;
        assert!(result.is_err(), "bad cert should fail reload");
    }

    #[test]
    fn http_tls_reload_handler_matches_correctly() {
        let (cert_pem, key_pem) = {
            let (_ca, c, k) = tls::gen_server_cert();
            (c, k)
        };
        let cert_path = tls::write_pem_tmp("http-reload-match-cert.pem", &cert_pem);
        let key_path = tls::write_pem_tmp("http-reload-match-key.pem", &key_pem);
        let source = ServerTlsSource {
            cert_path,
            key_path,
            client_ca_path: None,
        };
        let cfg = source.build_server_config().unwrap();
        let rustls_cfg = axum_server::tls_rustls::RustlsConfig::from_config(Arc::new(cfg));

        let handler = HttpReloadHandler::new(rustls_cfg, source, "0.0.0.0".into(), 9090);
        assert!(handler.matches("https", "0.0.0.0", 9090));
        assert!(!handler.matches("http", "0.0.0.0", 9090));
        assert!(!handler.matches("https", "127.0.0.1", 9090));
        assert!(!handler.matches("https", "0.0.0.0", 8080));
    }
}
