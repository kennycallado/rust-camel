//! Ephemeral mTLS certificate material for bridge IPC.
//! Generated per-spawn via rcgen, never persisted beyond process lifetime.

use std::path::PathBuf;

use rcgen::{CertificateParams, DnType, SanType};
use tempfile::TempDir;

use crate::process::BridgeError;

pub(crate) struct BridgeTlsMaterial {
    pub ca_pem: Vec<u8>,
    pub client_pem: Vec<u8>,
    pub client_key_pem: Vec<u8>,
    pub server_pem_path: PathBuf,
    pub server_key_path: PathBuf,
    pub ca_pem_path: PathBuf,
    _temp_dir: TempDir,
}

impl BridgeTlsMaterial {
    pub(crate) fn generate() -> Result<Self, BridgeError> {
        use rcgen::{BasicConstraints, IsCa, KeyPair};
        use std::net::IpAddr;
        use time::{Duration, OffsetDateTime};

        let now = OffsetDateTime::now_utc();
        let not_before = now.checked_sub(Duration::seconds(60)).unwrap_or(now);
        let not_after = now.checked_add(Duration::days(90)).unwrap_or(now);

        // --- CA ---
        let ca_key =
            KeyPair::generate().map_err(|e| BridgeError::Config(format!("CA keygen: {e}")))?;
        let mut ca_params = CertificateParams::default();
        ca_params.not_before = not_before;
        ca_params.not_after = not_after;
        ca_params
            .distinguished_name
            .push(DnType::CommonName, "camel-bridge CA");
        ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        let ca_cert = ca_params
            .self_signed(&ca_key)
            .map_err(|e| BridgeError::Config(format!("CA self-sign: {e}")))?;

        // --- Server cert ---
        let server_key =
            KeyPair::generate().map_err(|e| BridgeError::Config(format!("server keygen: {e}")))?;
        let mut server_params = CertificateParams::default();
        server_params.not_before = not_before;
        server_params.not_after = not_after;
        let localhost = "localhost"
            .try_into()
            .map_err(|e| BridgeError::Config(format!("localhost SAN: {e}")))?;
        server_params.subject_alt_names = vec![
            SanType::DnsName(localhost),
            SanType::IpAddress(IpAddr::V4([127, 0, 0, 1].into())),
            SanType::IpAddress(
                "::1"
                    .parse()
                    .map_err(|e| BridgeError::Config(format!("::1 parse: {e}")))?,
            ),
        ];
        let server_cert = server_params
            .signed_by(&server_key, &ca_cert, &ca_key)
            .map_err(|e| BridgeError::Config(format!("server cert sign: {e}")))?;

        // --- Client cert ---
        let client_key =
            KeyPair::generate().map_err(|e| BridgeError::Config(format!("client keygen: {e}")))?;
        let mut client_params = CertificateParams::default();
        client_params.not_before = not_before;
        client_params.not_after = not_after;
        client_params
            .distinguished_name
            .push(DnType::CommonName, "camel-bridge client");
        let client_cert = client_params
            .signed_by(&client_key, &ca_cert, &ca_key)
            .map_err(|e| BridgeError::Config(format!("client cert sign: {e}")))?;

        // --- Write PEMs to TempDir ---
        #[cfg(unix)]
        let temp_dir = {
            use std::os::unix::fs::PermissionsExt;
            let dir = tempfile::Builder::new()
                .prefix("camel-bridge-tls-")
                .tempdir()
                .map_err(|e| BridgeError::Config(format!("tempdir: {e}")))?;
            std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700))
                .map_err(|e| BridgeError::Config(format!("tempdir chmod: {e}")))?;
            dir
        };
        #[cfg(not(unix))]
        let temp_dir = tempfile::Builder::new()
            .prefix("camel-bridge-tls-")
            .tempdir()
            .map_err(|e| BridgeError::Config(format!("tempdir: {e}")))?;

        let ca_pem = ca_cert.pem().into_bytes();
        let server_pem = server_cert.pem().into_bytes();
        let server_key_pem = server_key.serialize_pem().into_bytes();
        let client_pem = client_cert.pem().into_bytes();
        let client_key_pem = client_key.serialize_pem().into_bytes();

        let ca_pem_path = write_pem(&temp_dir, "ca.pem", &ca_pem)?;
        let server_pem_path = write_pem(&temp_dir, "server.pem", &server_pem)?;
        let server_key_path = write_pem(&temp_dir, "server-key.pem", &server_key_pem)?;

        Ok(Self {
            ca_pem,
            client_pem,
            client_key_pem,
            server_pem_path,
            server_key_path,
            ca_pem_path,
            _temp_dir: temp_dir,
        })
    }
}

#[cfg(unix)]
fn write_pem(dir: &TempDir, name: &str, content: &[u8]) -> Result<PathBuf, BridgeError> {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.path().join(name);
    std::fs::write(&path, content)
        .map_err(|e| BridgeError::Config(format!("write {name}: {e}")))?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
        .map_err(|e| BridgeError::Config(format!("chmod {name}: {e}")))?;
    Ok(path)
}

#[cfg(not(unix))]
fn write_pem(dir: &TempDir, name: &str, content: &[u8]) -> Result<PathBuf, BridgeError> {
    let path = dir.path().join(name);
    std::fs::write(&path, content)
        .map_err(|e| BridgeError::Config(format!("write {name}: {e}")))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_produces_valid_material() {
        let tls = BridgeTlsMaterial::generate().expect("generate");
        assert!(!tls.ca_pem.is_empty());
        assert!(!tls.client_pem.is_empty());
        assert!(!tls.client_key_pem.is_empty());
        assert!(tls.server_pem_path.exists());
        assert!(tls.server_key_path.exists());
        assert!(tls.ca_pem_path.exists());
    }

    #[test]
    fn server_cert_contains_127_0_0_1_san() {
        // Parse the server cert PEM and verify SAN contains 127.0.0.1
        let tls = BridgeTlsMaterial::generate().expect("generate");
        let server_pem = std::fs::read_to_string(&tls.server_pem_path).expect("read server.pem");
        let pem_blocks: Vec<_> = rustls_pemfile::certs(&mut server_pem.as_bytes())
            .collect::<Result<Vec<_>, _>>()
            .expect("parse pem");
        assert!(!pem_blocks.is_empty());
        let cert_der = &pem_blocks[0];

        // Use x509-parser to extract SANs
        let (_, cert) = x509_parser::parse_x509_certificate(cert_der).expect("parse x509");
        let san_ext = cert
            .subject_alternative_name()
            .expect("SAN extension extraction failed")
            .expect("SAN extension present");
        let general_names = san_ext.value.general_names.iter().collect::<Vec<_>>();
        let has_ip = general_names.iter().any(|gn| {
            matches!(gn, x509_parser::extensions::GeneralName::IPAddress(addr)
                if addr.len() == 4 && addr == &[127, 0, 0, 1])
        });
        assert!(has_ip, "server cert SAN must contain 127.0.0.1");
    }

    #[cfg(unix)]
    #[test]
    fn temp_files_are_0600() {
        use std::os::unix::fs::PermissionsExt;
        let tls = BridgeTlsMaterial::generate().expect("generate");
        let perms = std::fs::metadata(&tls.server_key_path)
            .expect("stat")
            .permissions()
            .mode();
        assert_eq!(perms & 0o777, 0o600);
    }

    #[test]
    fn drop_cleans_up_temp_dir() {
        let path;
        {
            let tls = BridgeTlsMaterial::generate().expect("generate");
            path = tls.server_pem_path.clone();
            assert!(path.exists());
        }
        assert!(!path.exists(), "temp file should be cleaned up on Drop");
    }
}
