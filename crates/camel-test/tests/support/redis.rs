#![allow(dead_code)]

use std::net::IpAddr;
use std::path::PathBuf;

use testcontainers::core::{ContainerPort, WaitFor};
use testcontainers::{ContainerAsync, GenericImage, ImageExt, runners::AsyncRunner};
use testcontainers_modules::redis::Redis;
use tokio::sync::OnceCell;

static REDIS: OnceCell<(ContainerAsync<Redis>, String)> = OnceCell::const_new();

/// Shared Redis container, started once and reused across all tests.
/// Returns the `host:port` (e.g. `127.0.0.1:<mapped>`) the component should
/// connect to.
pub async fn shared_redis() -> &'static str {
    REDIS
        .get_or_init(|| async {
            super::init_tracing();
            super::install_crypto_provider();

            let container = Redis::default()
                .start()
                .await
                .expect("Redis container failed to start");
            let port = container
                .get_host_port_ipv4(6379)
                .await
                .expect("Redis port not available");
            let conn_str = format!("127.0.0.1:{port}");
            eprintln!("Redis ready at: {conn_str}");
            (container, conn_str)
        })
        .await
        .1
        .as_str()
}

/// Shared Redis server that only accepts TLS connections, started once and
/// reused across all tests. The fixture owns the CA tempdir and the container
/// for the whole process lifetime: dropping the tempdir would delete
/// `ca_file`, and dropping the container would stop Redis while tests still
/// use it.
pub(crate) struct RedisTlsFixture {
    pub(crate) host: String,
    pub(crate) port: u16,
    /// CA certificate PEM that signs the server certificate.
    pub(crate) ca_pem: String,
    /// CA file on disk (inside the retained tempdir) for `tls_ca_cert` config.
    pub(crate) ca_file: PathBuf,
    _ca_dir: tempfile::TempDir,
    _container: ContainerAsync<GenericImage>,
}

impl RedisTlsFixture {
    /// CA path as an owned UTF-8 string, for `tls_ca_cert` configuration.
    /// Tempdir paths are UTF-8 on the Linux hosts this suite runs on;
    /// non-UTF8 is a test-infra bug worth panicking on.
    pub(crate) fn ca_path_string(&self) -> String {
        self.ca_file
            .to_str()
            .expect("CA tempdir path is UTF-8")
            .to_string()
    }
}

impl std::fmt::Display for RedisTlsFixture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "rediss://{}:{}", self.host, self.port)
    }
}

static REDIS_TLS: OnceCell<RedisTlsFixture> = OnceCell::const_new();

/// Shared TLS-only Redis container, started once and reused across all tests.
pub(crate) async fn shared_redis_tls() -> &'static RedisTlsFixture {
    REDIS_TLS
        .get_or_init(|| async {
            super::init_tracing();
            super::install_crypto_provider();

            // rcgen CA + server cert for IP SAN 127.0.0.1, signed by the CA.
            let ca_key = rcgen::KeyPair::generate().expect("CA key pair");
            let mut ca_params = rcgen::CertificateParams::default();
            ca_params
                .distinguished_name
                .push(rcgen::DnType::CommonName, "rust-camel-test-ca");
            ca_params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
            let ca_cert = ca_params.self_signed(&ca_key).expect("CA certificate");

            let server_key = rcgen::KeyPair::generate().expect("server key pair");
            let server_params =
                rcgen::CertificateParams::new(vec![IpAddr::from([127, 0, 0, 1]).to_string()])
                    .expect("server certificate params");
            let issuer = rcgen::Issuer::from_params(&ca_params, &ca_key);
            let server_cert = server_params
                .signed_by(&server_key, &issuer)
                .expect("server certificate");

            let ca_pem = ca_cert.pem();
            let server_pem = server_cert.pem();
            let server_key_pem = server_key.serialize_pem();

            // Process-side copy of the CA for tls_ca_cert path configuration.
            // The fixture owns the tempdir so the file outlives every test.
            let ca_dir = tempfile::TempDir::new().expect("CA tempdir");
            let ca_file = ca_dir.path().join("ca.pem");
            std::fs::write(&ca_file, &ca_pem).expect("write CA PEM to tempdir");

            // Self-provisioning script (same idiom as the sentinel topology):
            // print the PEMs to /tmp/tls/ inside the container, then start a
            // TLS-only Redis on 6379. PEM bodies end with a newline, so each
            // heredoc terminator lands on its own line.
            let script = format!(
                "set -e\n\
                 mkdir -p /tmp/tls\n\
                 cat > /tmp/tls/ca.pem <<'EOF'\n{ca_pem}EOF\n\
                 cat > /tmp/tls/server.pem <<'EOF'\n{server_pem}EOF\n\
                 cat > /tmp/tls/server.key <<'EOF'\n{server_key_pem}EOF\n\
                 exec redis-server --tls-port 6379 --port 0 \
                 --tls-cert-file /tmp/tls/server.pem \
                 --tls-key-file /tmp/tls/server.key \
                 --tls-ca-cert-file /tmp/tls/ca.pem \
                 --tls-auth-clients no\n"
            );

            let image = GenericImage::new("redis", "7-alpine")
                .with_exposed_port(ContainerPort::Tcp(6379))
                .with_cmd(["sh", "-c", &script])
                .with_ready_conditions(vec![WaitFor::message_on_stdout(
                    "Ready to accept connections tls",
                )]);
            let container = image
                .start()
                .await
                .expect("Redis TLS container failed to start");
            let port = container
                .get_host_port_ipv4(6379)
                .await
                .expect("Redis TLS port not available");
            eprintln!("Redis TLS ready at 127.0.0.1:{port}");

            RedisTlsFixture {
                host: "127.0.0.1".to_string(),
                port,
                ca_pem,
                ca_file,
                _ca_dir: ca_dir,
                _container: container,
            }
        })
        .await
}
