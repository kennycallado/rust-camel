//! Private-CA TLS fixture for fake servers (test support only).

use std::path::PathBuf;
use std::sync::Arc;
use tokio_rustls::rustls;

use camel_component_api::test_support::tls::{gen_server_cert, write_pem_tmp};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};

pub struct TlsFixture {
    pub ca_path: PathBuf,
    pub server_config: Arc<rustls::ServerConfig>,
}

pub fn tls_fixture() -> TlsFixture {
    let (ca, cert, key) = gen_server_cert();
    let ca_path = write_pem_tmp("ca.pem", &ca);
    let certs: Vec<CertificateDer<'static>> = CertificateDer::pem_slice_iter(cert.as_bytes())
        .collect::<Result<_, _>>()
        .expect("server cert pem");
    let key = PrivateKeyDer::from_pem_slice(key.as_bytes()).expect("server key pem");
    let server_config = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .expect("protocol versions")
    .with_no_client_auth()
    .with_single_cert(certs, key)
    .expect("server config");
    TlsFixture {
        ca_path,
        server_config: Arc::new(server_config),
    }
}
