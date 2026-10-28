//! TLS trust helpers.

use std::sync::Arc;
use tokio_rustls::rustls;

use rustls::RootCertStore;
use rustls::pki_types::CertificateDer;
use rustls::pki_types::pem::PemObject;

use crate::error::MailError;

/// Read every certificate from a PEM `caFile`.
pub(crate) fn load_ca_file(
    path: &std::path::Path,
) -> Result<Vec<CertificateDer<'static>>, MailError> {
    let iter = CertificateDer::pem_file_iter(path)
        .map_err(|e| MailError::Tls(format!("caFile {}: {e}", path.display())))?;
    let certs = iter
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| MailError::Tls(format!("caFile {}: {e}", path.display())))?;
    if certs.is_empty() {
        return Err(MailError::Tls(format!(
            "caFile {} contains no certificate",
            path.display()
        )));
    }
    Ok(certs)
}

/// Build a ring-backed TLS client config: bundled webpki roots plus any
/// extra roots. The provider is explicit because the process-default is
/// ambiguous when the workspace also links aws-lc-rs.
pub(crate) fn client_config(
    extra_roots: &[CertificateDer<'static>],
) -> Result<Arc<rustls::ClientConfig>, MailError> {
    let mut store = RootCertStore::empty();
    store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    for root in extra_roots {
        store
            .add(root.clone())
            .map_err(|e| MailError::Tls(format!("extra root: {e}")))?;
    }
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map_err(|e| MailError::Tls(format!("protocol versions: {e}")))?
    .with_root_certificates(store)
    .with_no_client_auth();
    Ok(Arc::new(config))
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_component_api::test_support::tls::{gen_server_cert, write_pem_tmp};

    #[test]
    fn load_ca_file_reads_test_ca() {
        let path = write_pem_tmp("ca.pem", &gen_server_cert().0);
        let certs = load_ca_file(&path).expect("ca loads");
        assert_eq!(certs.len(), 1);
    }

    #[test]
    fn load_ca_file_rejects_empty() {
        let path = write_pem_tmp("bad.pem", "not a pem");
        assert!(matches!(load_ca_file(&path), Err(MailError::Tls(_))));
    }

    #[test]
    fn client_config_accepts_test_ca() {
        let path = write_pem_tmp("ca2.pem", &gen_server_cert().0);
        let roots = load_ca_file(&path).expect("ca loads");
        assert!(client_config(&roots).is_ok());
    }

    #[test]
    fn client_config_without_extra_roots() {
        assert!(client_config(&[]).is_ok());
    }
}
