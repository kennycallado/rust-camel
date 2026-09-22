//! Live `rediss-sentinel://` coverage: a TLS sentinel topology end-to-end
//! (rc-hbde6). One container runs a TLS-only Redis master and a TLS-only
//! sentinel monitoring it (`tls-replication yes`). The cache repository
//! connects through `rediss-sentinel://` with the fixture CA, proving BOTH
//! encrypted surfaces at once: the discovery hop (sentinel link) and the
//! resolved master link (`TlsMode::Secure` via the CA trust landed with
//! rc-hbde6). Negative controls prove each surface really speaks TLS: a
//! plaintext client is rejected on both the sentinel port and the master
//! port.
//!
//! **Requires Docker to be running.** Tests fail if Docker is unavailable.
//!
//! **Requires `integration-tests` feature to compile and run.**

#![cfg(feature = "integration-tests")]

mod support;

use std::sync::Arc;
use std::time::Duration;

use camel_api::ComponentMetrics;
use camel_api::cache::{CacheEntry, CacheRepository, ContentType};
use camel_api::metrics::MetricsHandle;
use camel_redis_repo::{RedisCacheRepository, RedisEndpointConfig};
use support::install_crypto_provider;
use testcontainers::core::{ContainerPort, WaitFor};
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, GenericImage, ImageExt};
use tokio::sync::OnceCell;

/// Fixed loopback ports for the TLS sentinel topology (distinct from every
/// other redis suite's fixed ports: 6379/16379/16380/26379/17489/17490/27489).
const SENT_TLS_MASTER_PORT: u16 = 16443;
const SENT_TLS_SENTINEL_PORT: u16 = 26443;
const SENT_TLS_MASTER_NAME: &str = "mymaster";

/// Labels this suite's container so a previous crashed run can be identified
/// and removed before the fixed ports are bound again.
const SENT_TLS_LABEL_KEY: &str = "org.rust-camel.redis-sentinel-tls-test";
const SENT_TLS_LABEL_VALUE: &str = "true";

/// Deadline for the repository round-trip steps (ADR-0069 — bounded
/// assertions, no sleeps).
const ROUND_TRIP_DEADLINE: Duration = Duration::from_secs(20);

/// Deadline for each negative control.
const NEGATIVE_DEADLINE: Duration = Duration::from_secs(5);

/// One TLS sentinel topology: TLS-only master + TLS-only sentinel, both
/// trusting the fixture CA. The CA PEM is kept process-side for
/// `tls_ca_cert` configuration.
struct RedisSentinelTlsFixture {
    host: String,
    sentinel_port: u16,
    ca_pem: String,
    ca_file: std::path::PathBuf,
}

impl RedisSentinelTlsFixture {
    /// CA path as an owned UTF-8 string for `tls_ca_cert` configuration.
    fn ca_path_string(&self) -> String {
        self.ca_file.to_string_lossy().into_owned()
    }
}

static SENT_TLS: OnceCell<RedisSentinelTlsFixture> = OnceCell::const_new();

/// Force-removes containers left by a previous crashed run of this suite.
/// The fixture is deliberately leaked for the process lifetime, so a
/// following run MUST clear the fixed ports before binding them again —
/// otherwise every start fails with "port is already allocated" (the same
/// failure mode the sibling redis suites engineer around).
async fn remove_stale_sentinel_tls_containers() {
    use std::collections::HashMap;

    let docker = match bollard::Docker::connect_with_local_defaults() {
        Ok(d) => d,
        Err(_) => return,
    };
    let mut filters: HashMap<String, Vec<String>> = HashMap::new();
    filters.insert(
        "label".to_string(),
        vec![format!("{SENT_TLS_LABEL_KEY}={SENT_TLS_LABEL_VALUE}")],
    );
    let options = bollard::query_parameters::ListContainersOptionsBuilder::default()
        .all(true)
        .filters(&filters)
        .build();
    let stale = match docker.list_containers(Some(options)).await {
        Ok(list) => list,
        Err(_) => return,
    };
    let remove = bollard::query_parameters::RemoveContainerOptionsBuilder::default()
        .force(true)
        .build();
    for container in stale {
        if let Some(id) = container.id {
            let _ = docker.remove_container(&id, Some(remove.clone())).await;
        }
    }
}

/// Shared TLS sentinel topology, started once and reused across the tests.
async fn shared_redis_sentinel_tls() -> &'static RedisSentinelTlsFixture {
    SENT_TLS
        .get_or_init(|| async {
            support::init_tracing();
            install_crypto_provider();
            remove_stale_sentinel_tls_containers().await;

            let (ca_pem, server_pem, server_key_pem) =
                camel_component_api::test_support::tls::gen_server_cert();
            let ca_file =
                camel_component_api::test_support::tls::write_pem_tmp("sentinel-tls-ca", &ca_pem);

            // Same heredoc idiom as the standalone TLS fixture: print the
            // PEMs into the container, boot the TLS-only master, then a
            // TLS-only sentinel monitoring it (`tls-replication yes` tells
            // the sentinel its data-node links are TLS too).
            let script = format!(
                "set -e\n\
                 mkdir -p /tmp/tls\n\
                 cat > /tmp/tls/ca.pem <<'EOF'\n{ca_pem}EOF\n\
                 cat > /tmp/tls/server.pem <<'EOF'\n{server_pem}EOF\n\
                 cat > /tmp/tls/server.key <<'EOF'\n{server_key_pem}EOF\n\
                 redis-server --tls-port {SENT_TLS_MASTER_PORT} --port 0 \
                 --tls-cert-file /tmp/tls/server.pem \
                 --tls-key-file /tmp/tls/server.key \
                 --tls-ca-cert-file /tmp/tls/ca.pem \
                 --tls-auth-clients no --daemonize yes\n\
                 until redis-cli --tls --cacert /tmp/tls/ca.pem \
                 -p {SENT_TLS_MASTER_PORT} ping | grep -q PONG; do sleep 0.1; done\n\
                 printf 'port 0\\n\
                 tls-port {SENT_TLS_SENTINEL_PORT}\\n\
                 tls-cert-file /tmp/tls/server.pem\\n\
                 tls-key-file /tmp/tls/server.key\\n\
                 tls-ca-cert-file /tmp/tls/ca.pem\\n\
                 tls-auth-clients no\\n\
                 tls-replication yes\\n\
                 sentinel monitor {SENT_TLS_MASTER_NAME} 127.0.0.1 {SENT_TLS_MASTER_PORT} 1\\n\
                 sentinel down-after-milliseconds {SENT_TLS_MASTER_NAME} 3000\\n\
                 sentinel failover-timeout {SENT_TLS_MASTER_NAME} 10000\\n' > /tmp/sentinel.conf\n\
                 exec redis-sentinel /tmp/sentinel.conf\n"
            );

            let image = GenericImage::new("redis", "7-alpine")
                .with_cmd(["sh", "-c", &script])
                .with_label(SENT_TLS_LABEL_KEY, SENT_TLS_LABEL_VALUE)
                .with_mapped_port(
                    SENT_TLS_MASTER_PORT,
                    ContainerPort::Tcp(SENT_TLS_MASTER_PORT),
                )
                .with_mapped_port(
                    SENT_TLS_SENTINEL_PORT,
                    ContainerPort::Tcp(SENT_TLS_SENTINEL_PORT),
                )
                .with_ready_conditions(vec![WaitFor::message_on_stdout("+monitor")]);

            let _container: ContainerAsync<GenericImage> = image
                .start()
                .await
                .expect("redis TLS sentinel topology failed to start");

            // The container is deliberately leaked (never dropped): the
            // fixture lives for the whole process, exactly like the shared
            // standalone TLS fixture — dropping it would stop Redis while
            // other tests still use the topology.
            std::mem::forget(_container);

            let fixture = RedisSentinelTlsFixture {
                host: "127.0.0.1".to_string(),
                sentinel_port: SENT_TLS_SENTINEL_PORT,
                ca_pem,
                ca_file,
            };

            // Readiness gate: a TLS client with the fixture CA resolves the
            // master through the sentinel — this IS the discovery hop the
            // round-trip test then drives through the repository.
            support::wait::wait_until(
                "TLS sentinel resolves the TLS master",
                Duration::from_secs(60),
                Duration::from_millis(250),
                || async { Ok(sentinel_resolves_master(&fixture).await.is_some()) },
            )
            .await
            .expect("TLS sentinel topology never became ready");

            eprintln!(
                "redis TLS sentinel topology ready: master 127.0.0.1:{SENT_TLS_MASTER_PORT} \
                 (TLS), sentinel 127.0.0.1:{SENT_TLS_SENTINEL_PORT} (TLS)"
            );
            fixture
        })
        .await
}

/// Raw TLS sentinel query: `SENTINEL get-master-addr-by-name` through a
/// client that trusts the fixture CA. `Some(port)` only when the reply is
/// the expected loopback master.
async fn sentinel_resolves_master(fixture: &RedisSentinelTlsFixture) -> Option<u16> {
    let info = redis::IntoConnectionInfo::into_connection_info(format!(
        "rediss://{}:{}",
        fixture.host, fixture.sentinel_port
    ))
    .ok()?;
    let client = redis::Client::build_with_tls(
        info,
        redis::TlsCertificates {
            client_tls: None,
            root_cert: Some(fixture.ca_pem.as_bytes().to_vec()),
        },
    )
    .ok()?;
    let mut conn = client.get_multiplexed_async_connection().await.ok()?;
    let (ip, port): (String, String) = redis::cmd("SENTINEL")
        .arg("get-master-addr-by-name")
        .arg(SENT_TLS_MASTER_NAME)
        .query_async(&mut conn)
        .await
        .ok()?;
    if ip == "127.0.0.1" {
        port.parse().ok()
    } else {
        None
    }
}

fn cache_entry() -> CacheEntry {
    CacheEntry {
        bytes: b"sentinel-tls-live-payload".to_vec(),
        payload_path: None,
        content_type: ContentType::Bytes,
        expires_at: None,
    }
}

/// `rediss-sentinel://` endpoint with the fixture CA. `from_uri` derives
/// `ssl = Some(true)` from the scheme, which routes the resolved master
/// connections through `TlsMode::Secure`.
fn endpoint(fixture: &RedisSentinelTlsFixture) -> RedisEndpointConfig {
    let mut endpoint = RedisEndpointConfig::from_uri(&format!(
        "rediss-sentinel://{}:{}/{}",
        fixture.host, fixture.sentinel_port, SENT_TLS_MASTER_NAME
    ))
    .expect("rediss-sentinel URI parses");
    endpoint.tls_ca_cert = Some(fixture.ca_path_string());
    endpoint
}

/// Runs connect-or-PING against `url` with a raw plaintext client and
/// returns the rejection, asserting it surfaces within the deadline.
async fn plaintext_rejection(url: String) -> redis::RedisError {
    let client = redis::Client::open(url).expect("client URI parses");
    tokio::time::timeout(NEGATIVE_DEADLINE, async {
        match client.get_multiplexed_async_connection().await {
            Err(err) => Some(err),
            Ok(mut conn) => redis::cmd("PING")
                .query_async::<String>(&mut conn)
                .await
                .err(),
        }
    })
    .await
    .expect("the rejection must surface within the deadline, not hang")
    .expect("the plaintext client must be rejected")
}

/// Full round-trip through the repository: the discovery hop (sentinel
/// link) and every command (master link) both encrypt under the fixture CA.
#[tokio::test(flavor = "multi_thread")]
async fn rediss_sentinel_round_trip_through_cache_repository() {
    install_crypto_provider();
    let fixture = shared_redis_sentinel_tls().await;

    // Discovery hop proof (raw client): the TLS sentinel resolves the TLS
    // master address before the repository connects.
    assert_eq!(
        sentinel_resolves_master(fixture).await,
        Some(SENT_TLS_MASTER_PORT),
        "the TLS sentinel must resolve the TLS master"
    );

    let repo = tokio::time::timeout(
        ROUND_TRIP_DEADLINE,
        RedisCacheRepository::connect(
            "sentinel-tls-live",
            &endpoint(fixture),
            "sentlstls",
            Duration::from_secs(300),
            // Live suite: lever-off facade, compile-only wiring.
            ComponentMetrics::new(Arc::new(MetricsHandle::new()), false),
        ),
    )
    .await
    .expect("connect must finish within the deadline")
    .expect("rediss-sentinel:// endpoint connects through the fixture CA");

    let entry = cache_entry();
    tokio::time::timeout(ROUND_TRIP_DEADLINE, async {
        repo.set("k1", entry.clone(), Some(Duration::from_secs(60)))
            .await
            .expect("set through the TLS sentinel topology succeeds");
        let got = repo
            .get("k1")
            .await
            .expect("get through the TLS sentinel topology succeeds")
            .expect("entry is present after set");
        assert_eq!(
            got.bytes, entry.bytes,
            "payload must survive discovery hop + TLS master path"
        );

        repo.invalidate("k1")
            .await
            .expect("invalidate through the TLS sentinel topology succeeds");
        let after = repo.get("k1").await.expect("get after invalidate succeeds");
        assert!(after.is_none(), "entry must be absent after invalidate");
    })
    .await
    .expect("round-trip must finish within the deadline");

    let _ = Arc::new(&repo); // repository stays usable across the suite
}

/// TLS is real on the SENTINEL plane: a plaintext client is rejected by the
/// TLS-only sentinel port.
#[tokio::test(flavor = "multi_thread")]
async fn plaintext_client_against_tls_sentinel_port_fails() {
    install_crypto_provider();
    let fixture = shared_redis_sentinel_tls().await;

    let err = plaintext_rejection(format!(
        "redis://{}:{}",
        fixture.host, fixture.sentinel_port
    ))
    .await;
    assert!(
        matches!(
            err.kind(),
            redis::ErrorKind::Io | redis::ErrorKind::Parse | redis::ErrorKind::Server(_)
        ),
        "expected an IO/protocol-level rejection on the sentinel port, got: {err}"
    );
}

/// TLS is real on the DATA plane: a plaintext client is rejected by the
/// TLS-only master port.
#[tokio::test(flavor = "multi_thread")]
async fn plaintext_client_against_tls_master_port_fails() {
    install_crypto_provider();
    let fixture = shared_redis_sentinel_tls().await;

    let err =
        plaintext_rejection(format!("redis://{}:{}", fixture.host, SENT_TLS_MASTER_PORT)).await;
    assert!(
        matches!(
            err.kind(),
            redis::ErrorKind::Io | redis::ErrorKind::Parse | redis::ErrorKind::Server(_)
        ),
        "expected an IO/protocol-level rejection on the master port, got: {err}"
    );
}
