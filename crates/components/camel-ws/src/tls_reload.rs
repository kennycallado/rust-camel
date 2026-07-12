use std::sync::Arc;

use async_trait::async_trait;
use camel_api::CamelError;
use camel_component_api::tls_source::{ServerTlsSource, TlsReloadHandler};

pub(crate) struct WsReloadHandler {
    tls_config: axum_server::tls_rustls::RustlsConfig,
    source: ServerTlsSource,
    port: u16,
}

impl WsReloadHandler {
    pub(crate) fn new(
        tls_config: axum_server::tls_rustls::RustlsConfig,
        source: ServerTlsSource,
        port: u16,
    ) -> Self {
        Self {
            tls_config,
            source,
            port,
        }
    }
}

#[async_trait]
impl TlsReloadHandler for WsReloadHandler {
    fn matches(&self, scheme: &str, _host: &str, port: u16) -> bool {
        // WS registry is port-keyed; host is intentionally ignored.
        scheme == "wss" && self.port == port
    }

    async fn reload(&self) -> Result<(), CamelError> {
        match self.source.build_server_config() {
            Ok(server_cfg) => {
                self.tls_config.reload_from_config(Arc::new(server_cfg));
                tracing::info!(port = self.port, "WebSocket TLS cert reloaded");
                Ok(())
            }
            Err(e) => {
                tracing::warn!(
                    port = self.port,
                    error = %e,
                    "WebSocket TLS cert reload failed — keeping old cert"
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
    async fn ws_tls_reload_handler_swaps_valid_cert() {
        // Note: RustlsConfig (axum-server) does not expose its internal ArcSwap
        // pointer, so we can't directly verify pointer identity here. The swap
        // is verified by the gRPC reload tests (which use the same ArcSwap
        // pattern with an exposed pointer) and by the RuntimeBus dispatch
        // tests (which exercise the end-to-end ReloadTlsCerts command path).
        let (cert_pem, key_pem) = {
            let (_ca, c, k) = tls::gen_server_cert();
            (c, k)
        };
        let cert_path = tls::write_pem_tmp("ws-reload-cert-1.pem", &cert_pem);
        let key_path = tls::write_pem_tmp("ws-reload-key-1.pem", &key_pem);

        let source = ServerTlsSource {
            cert_path: cert_path.clone(),
            key_path: key_path.clone(),
            client_ca_path: None,
        };
        let cfg = source.build_server_config().expect("initial cert");
        let rustls_cfg = axum_server::tls_rustls::RustlsConfig::from_config(Arc::new(cfg));

        let handler = WsReloadHandler::new(rustls_cfg, source, 8443);

        let (_ca2, cert2, key2) = tls::gen_server_cert();
        std::fs::write(&handler.source.cert_path, &cert2).unwrap();
        std::fs::write(&handler.source.key_path, &key2).unwrap();

        handler.reload().await.expect("reload should succeed");
    }

    #[tokio::test]
    async fn ws_tls_reload_handler_fails_on_bad_cert() {
        let (cert_pem, key_pem) = {
            let (_ca, c, k) = tls::gen_server_cert();
            (c, k)
        };
        let cert_path = tls::write_pem_tmp("ws-reload-bad-cert.pem", &cert_pem);
        let key_path = tls::write_pem_tmp("ws-reload-bad-key.pem", &key_pem);

        let source = ServerTlsSource {
            cert_path: cert_path.clone(),
            key_path: key_path.clone(),
            client_ca_path: None,
        };
        let cfg = source.build_server_config().unwrap();
        let rustls_cfg = axum_server::tls_rustls::RustlsConfig::from_config(Arc::new(cfg));

        std::fs::write(&cert_path, "GARBAGE").unwrap();

        let handler = WsReloadHandler::new(rustls_cfg, source, 8444);
        assert!(handler.reload().await.is_err());
    }

    #[test]
    fn ws_tls_reload_handler_matches_host_agnostic() {
        let (cert_pem, key_pem) = {
            let (_ca, c, k) = tls::gen_server_cert();
            (c, k)
        };
        let cert_path = tls::write_pem_tmp("ws-reload-match-cert.pem", &cert_pem);
        let key_path = tls::write_pem_tmp("ws-reload-match-key.pem", &key_pem);
        let source = ServerTlsSource {
            cert_path,
            key_path,
            client_ca_path: None,
        };
        let cfg = source.build_server_config().unwrap();
        let rustls_cfg = axum_server::tls_rustls::RustlsConfig::from_config(Arc::new(cfg));

        let handler = WsReloadHandler::new(rustls_cfg, source, 9090);
        // Host-agnostic: matches on scheme=wss + port regardless of host
        assert!(handler.matches("wss", "0.0.0.0", 9090));
        assert!(handler.matches("wss", "127.0.0.1", 9090)); // different host OK
        assert!(handler.matches("wss", "any-host", 9090));
        assert!(!handler.matches("ws", "0.0.0.0", 9090)); // wrong scheme
        assert!(!handler.matches("wss", "0.0.0.0", 8080)); // wrong port
    }
}
