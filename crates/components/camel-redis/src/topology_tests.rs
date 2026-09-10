//! Tests for the Redis topology factory.
//! Sibling file via `#[path]` so the production module stays scannable;
//! still in-crate for private-function access (`read_standalone_ca_pem`).

use super::*;

#[tokio::test]
async fn standalone_topology_resolve_returns_fixed_client() {
    let cfg = RedisEndpointConfig::from_uri("redis://127.0.0.1:6379").expect("valid uri");
    let topology = StandaloneTopology::new(&cfg);

    let r1 = topology.resolve(ServerKind::Master).await;
    let r2 = topology.resolve(ServerKind::Master).await;

    let c1 = r1.expect("first resolve should succeed");
    let c2 = r2.expect("second resolve should succeed");
    assert_eq!(
        c1.get_connection_info().addr().to_string(),
        "127.0.0.1:6379"
    );
    assert_eq!(
        c2.get_connection_info().addr().to_string(),
        "127.0.0.1:6379"
    );
}

#[tokio::test]
async fn standalone_topology_carries_configured_db() {
    let cfg = RedisEndpointConfig::from_uri("redis://localhost:6379?command=GET&db=2")
        .expect("valid uri");
    let topology = StandaloneTopology::new(&cfg);

    let client = topology
        .resolve(ServerKind::Master)
        .await
        .expect("resolve should succeed");

    assert_eq!(client.get_connection_info().redis_settings().db(), 2);
}

#[tokio::test]
async fn standalone_topology_default_db_zero() {
    let cfg =
        RedisEndpointConfig::from_uri("redis://localhost:6379?command=GET").expect("valid uri");
    let topology = StandaloneTopology::new(&cfg);

    let client = topology
        .resolve(ServerKind::Master)
        .await
        .expect("resolve should succeed");

    assert_eq!(client.get_connection_info().redis_settings().db(), 0);
}

#[tokio::test]
async fn standalone_topology_tls_addr_keeps_db() {
    let cfg = RedisEndpointConfig::from_uri("rediss://localhost:6380?command=GET&db=3")
        .expect("valid uri");
    let topology = StandaloneTopology::new(&cfg);

    let client = topology
        .resolve(ServerKind::Master)
        .await
        .expect("resolve should succeed");

    let info = client.get_connection_info();
    assert!(
        matches!(
            info.addr(),
            redis::ConnectionAddr::TcpTls {
                insecure: false,
                ..
            }
        ),
        "expected TcpTls with insecure=false, got {:?}",
        info.addr()
    );
    assert_eq!(info.redis_settings().db(), 3);
}

#[tokio::test]
#[cfg(not(feature = "tls"))]
async fn topology_from_config_rejects_tls_without_feature() {
    let mut cfg =
        RedisEndpointConfig::from_uri("rediss://redis-prod:6379?command=GET").expect("valid uri");
    cfg.resolve_defaults();
    let result = topology_from_config(&cfg);
    assert!(
        matches!(result, Err(CamelError::Config(_))),
        "topology_from_config must fail closed with a Config error when the \
         endpoint resolved to TLS but the tls cargo feature is absent"
    );
}

#[tokio::test]
async fn topology_from_config_accepts_plaintext_without_feature() {
    let mut cfg =
        RedisEndpointConfig::from_uri("redis://localhost:6379?command=GET").expect("valid uri");
    cfg.resolve_defaults();
    assert!(topology_from_config(&cfg).is_ok());
}

#[tokio::test]
async fn standalone_topology_password_raw() {
    let cfg =
        RedisEndpointConfig::from_uri("redis://localhost:6379?command=GET&password=p@ss:word")
            .expect("valid uri");
    let topology = StandaloneTopology::new(&cfg);

    let client = topology
        .resolve(ServerKind::Master)
        .await
        .expect("resolve should succeed");

    assert_eq!(
        client.get_connection_info().redis_settings().password(),
        Some("p@ss:word")
    );
}

#[tokio::test]
async fn fake_topology_returns_address_sequence() {
    let topology = FakeTopology::addrs(vec!["redis://a:6379".into(), "redis://b:6379".into()]);

    let r1 = topology.resolve(ServerKind::Master).await;
    let r2 = topology.resolve(ServerKind::Master).await;
    let r3 = topology.resolve(ServerKind::Master).await;

    let c1 = r1.expect("first resolve should succeed");
    let c2 = r2.expect("second resolve should succeed");
    let c3 = r3.expect("third resolve should succeed (reuse last)");
    assert_eq!(c1.get_connection_info().addr().to_string(), "a:6379");
    assert_eq!(c2.get_connection_info().addr().to_string(), "b:6379");
    assert_eq!(c3.get_connection_info().addr().to_string(), "b:6379");
    assert_eq!(topology.resolve_call_count(), 3);
}

#[tokio::test]
async fn fake_topology_returns_programmed_error() {
    let topology = FakeTopology::new(vec![Err(CamelError::ProcessorError("no master".into()))]);

    let result = topology.resolve(ServerKind::Master).await;

    assert!(result.is_err());
    assert!(
        result.unwrap_err().to_string().contains("no master"),
        "error should contain 'no master'"
    );
    assert_eq!(topology.resolve_call_count(), 1);
}

#[test]
fn embed_sentinel_creds_injects_credentials() {
    let result = embed_sentinel_creds("redis://s-a:26379", &Some(("su".into(), "sp".into())))
        .expect("tcp node with creds should embed");
    assert!(
        result.contains("su:sp"),
        "expected credentials in URL, got: {result}"
    );
    assert!(
        result.contains("s-a:26379"),
        "expected host:port preserved, got: {result}"
    );
}

#[test]
fn embed_sentinel_creds_preserves_node_when_no_creds() {
    let node = "redis://s-b:26379";
    let result = embed_sentinel_creds(node, &None).expect("no creds should pass the node through");
    assert_eq!(result, node);
}

// M2 fail-closed: an unparsable node with credentials configured must
// return Err (naming the node, redacted), NOT the node unchanged — the
// old silent pass-through dropped the credentials and auth failed later.
#[test]
fn embed_sentinel_creds_fails_closed_on_unparsable_node() {
    let result = embed_sentinel_creds("", &Some(("su".into(), "sp".into())));
    let err = result.expect_err("empty node URL must fail closed");
    let msg = err.to_string();
    assert!(
        msg.contains("cannot inject sentinel credentials"),
        "error must name the failure: {msg}"
    );
}

// M2 fail-closed: unix-socket nodes have no URL form to rewrite with
// credentials — fail closed instead of silently dropping them.
#[test]
fn embed_sentinel_creds_fails_closed_on_unix_socket() {
    let result = embed_sentinel_creds("unix:///tmp/redis.sock", &Some(("su".into(), "sp".into())));
    let err = result.expect_err("unix node with creds must fail closed");
    let msg = err.to_string();
    assert!(
        msg.contains("unsupported address kind"),
        "error must name the unsupported kind: {msg}"
    );
    assert!(
        !msg.contains("sp"),
        "error must not leak the sentinel secret: {msg}"
    );
}

// Redaction helper: any pre-existing userinfo is stripped for logs/errors.
#[test]
fn redact_userinfo_strips_credentials() {
    assert_eq!(
        redact_userinfo("redis://user:pass@host:26379"),
        "redis://host:26379"
    );
    assert_eq!(redact_userinfo("redis://host:26379"), "redis://host:26379");
}

#[cfg(feature = "sentinel")]
#[test]
fn sentinel_node_conn_info_carries_username() {
    use crate::sentinel_config::SentinelConfig;

    let config = RedisEndpointConfig {
        host: None,
        port: None,
        command: crate::config::RedisCommand::Set,
        channels: vec![],
        key: None,
        timeout: 1,
        username: Some("svc".to_string()),
        password: Some("p".to_string()),
        db: 2,
        ssl: None,
        tls_ca_cert: None,
        reconnect: camel_component_api::NetworkRetryPolicy::default(),
        connection_timeout_secs: 10,
        topology_kind: crate::sentinel_config::TopologyKind::Sentinel(
            SentinelConfig::default()
                .with_nodes(vec!["redis://s-a:26379".into()])
                .with_master_name("orders"),
        ),
    };

    // sentinel_node_conn_info embeds the redis settings; redis 1.6.0 has no
    // public getter on SentinelNodeConnectionInfo, so assert on the exact
    // RedisConnectionInfo it embeds (via its getters) plus Some(..) on the
    // wrapper itself.
    assert!(sentinel_node_conn_info(&config).is_some());
    let redis_info = node_redis_connection_info(&config);
    assert_eq!(redis_info.username(), Some("svc"));
    assert_eq!(redis_info.password(), Some("p"));
    assert_eq!(redis_info.db(), 2);
}

#[cfg(feature = "sentinel")]
#[test]
fn sentinel_topology_rejects_empty_nodes() {
    let result = SentinelTopology::new(vec![], "m".into(), None, None);
    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("sentinel requires"),
        "error should mention sentinel requires"
    );
}

#[cfg(feature = "sentinel")]
#[test]
fn sentinel_topology_rejects_empty_master_name() {
    let result = SentinelTopology::new(vec!["redis://s:26379".into()], "".into(), None, None);
    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("sentinel requires"),
        "error should mention sentinel requires"
    );
}

#[test]
fn embed_sentinel_creds_keeps_credentials_separate() {
    // Sentinel creds and node creds must not cross-contaminate: injecting
    // one pair on a node must not leak the other pair.
    let sentinel = embed_sentinel_creds("redis://s-a:26379", &Some(("su".into(), "sp".into())))
        .expect("tcp node should embed");
    let node = embed_sentinel_creds("redis://s-a:26379", &Some(("nu".into(), "np".into())))
        .expect("tcp node should embed");
    assert!(
        sentinel.contains("su:sp"),
        "expected sentinel creds in URL, got: {sentinel}"
    );
    assert!(
        node.contains("nu:np"),
        "expected node creds in URL, got: {node}"
    );
    assert!(
        !sentinel.contains("nu:np"),
        "sentinel URL leaked node creds: {sentinel}"
    );
    assert!(
        !node.contains("su:sp"),
        "node URL leaked sentinel creds: {node}"
    );
}

// redis-rs only parses `rediss://` URLs when a TLS feature is enabled, so
// this test needs the `tls` feature to exercise the TLS-preserving path.
#[cfg(feature = "tls")]
#[test]
fn embed_sentinel_creds_preserves_tls_scheme() {
    // rediss:// must stay TLS after cred injection.
    let result = embed_sentinel_creds("rediss://s-a:26379", &Some(("su".into(), "sp".into())))
        .expect("tls node should embed");
    assert!(
        result.starts_with("rediss://"),
        "expected rediss scheme preserved, got: {result}"
    );
    assert!(
        result.contains("su:sp"),
        "expected creds in URL, got: {result}"
    );
    assert!(
        result.contains("s-a:26379"),
        "expected host:port preserved, got: {result}"
    );
}

#[test]
fn embed_sentinel_creds_percent_encodes_special_chars() {
    let result = embed_sentinel_creds(
        "redis://s-a:26379",
        &Some(("u".into(), "p@ss:word/evil".into())),
    )
    .expect("tcp node should embed");
    // Verify percent-encoding of special characters via NON_ALPHANUMERIC
    assert!(
        result.contains("p%40ss"),
        "expected @ encoded as %40, got: {result}"
    );
    assert!(
        result.contains("%3Aword"),
        "expected : encoded as %3A, got: {result}"
    );
    assert!(
        result.contains("%2Fevil"),
        "expected / encoded as %2F, got: {result}"
    );
    // Round-trip: parse back and verify original creds
    let info = result
        .into_connection_info()
        .expect("should parse back as valid connection info");
    assert_eq!(
        info.redis_settings().password(),
        Some("p@ss:word/evil"),
        "round-trip password mismatch"
    );
    assert_eq!(
        info.redis_settings().username(),
        Some("u"),
        "round-trip username mismatch"
    );
}

#[cfg(feature = "sentinel")]
#[tokio::test]
async fn sentinel_topology_replica_resolve_errors() {
    let topology = SentinelTopology::new(vec!["redis://s:26379".into()], "m".into(), None, None)
        .expect("construction should succeed without network");
    let result = topology.resolve(ServerKind::Replica).await;
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("replica reads not yet supported"),
        "error should mention replica reads, got: {err}"
    );
}

// ── CA-backed TLS client (redis-live-tls-and-ready-race task 2.2) ──

// Fixed valid self-signed CA (EC P-256, CN=rust-camel-test-ca), embedded
// as a byte string: rcgen is not a dev-dependency of this crate. Client
// construction parses this PEM but never connects. Gated on `tls`
// because every consumer below is tls-gated (avoids dead_code in
// feature-less builds).
#[cfg(feature = "tls")]
const TEST_CA_PEM: &str = "\
-----BEGIN CERTIFICATE-----
MIIBjzCCATWgAwIBAgIUFuS4/TNFYXCZLJ3/uvKsij+qlPMwCgYIKoZIzj0EAwIw
HTEbMBkGA1UEAwwScnVzdC1jYW1lbC10ZXN0LWNhMB4XDTI2MDkxMDE0MDUzMloX
DTM2MDkwNzE0MDUzMlowHTEbMBkGA1UEAwwScnVzdC1jYW1lbC10ZXN0LWNhMFkw
EwYHKoZIzj0CAQYIKoZIzj0DAQcDQgAEhhNzvgq4uCMWRfxE1YVtxsJvg+sfhZVW
zW/+PFdHxSc/O3zUzfE1ppWcTgPcy3ofZa4sO7kuKcQ8VSvbplDDyqNTMFEwHQYD
VR0OBBYEFJNmN8KOdlFjI0fKC3C3npFOjW1CMB8GA1UdIwQYMBaAFJNmN8KOdlFj
I0fKC3C3npFOjW1CMA8GA1UdEwEB/wQFMAMBAf8wCgYIKoZIzj0EAwIDSAAwRQIg
cXwEPRXySFlXamOkqPj9Mll14M0978hpzKBEvU0E+rECIQDFNC+1qFpN/bTG9+wD
Hy8+9icj50unvO7Lgx7T1549xg==
-----END CERTIFICATE-----
";

// Unique path under the OS temp dir (a `tempfile` path without adding
// the tempfile crate as a dev-dependency). The pid keeps concurrent test
// binaries from colliding.
#[cfg(feature = "tls")]
fn temp_ca_path(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "camel-redis-test-ca-{}-{name}.pem",
        std::process::id()
    ))
}

#[cfg(feature = "tls")]
#[tokio::test]
async fn ca_configured_topology_stores_ca_and_resolves() {
    let path = temp_ca_path("configured");
    std::fs::write(&path, TEST_CA_PEM).expect("write CA fixture");
    let mut cfg =
        RedisEndpointConfig::from_uri("rediss://localhost:6380?command=GET").expect("valid uri");
    cfg.resolve_defaults();
    cfg.tls_ca_cert = Some(path.to_string_lossy().into_owned());

    // What the factory itself reads: the configured CA bytes. This is
    // the non-tautological proof that `topology_from_config` took the
    // CA branch (the storage assertion below exercises the constructor
    // it calls into).
    assert_eq!(
        read_standalone_ca_pem(&cfg).expect("ca read"),
        Some(TEST_CA_PEM.as_bytes().to_vec())
    );

    // Factory path: a readable CA builds a topology whose client
    // constructs (never connects) with the CA as root trust. (`expect`
    // is fine on this Ok path — only `expect_err` would require the
    // `Arc<dyn RedisTopology>` success type to be `Debug`.)
    let topology = topology_from_config(&cfg)
        .unwrap_or_else(|e| panic!("readable CA must build a topology: {e}"));
    topology
        .resolve(ServerKind::Master)
        .await
        .expect("client construction with CA must succeed");

    // Storage assertion on the concrete constructor the factory calls
    // into — `Arc<dyn RedisTopology>` cannot expose the accessor. `Ok`
    // alone proves nothing (`Client::open` also accepts TcpTls); this
    // storage assertion plus the Task 3.2 wrong-CA live rejection
    // jointly prove the branch was taken.
    let direct = StandaloneTopology::new_with_ca(&cfg, Some(TEST_CA_PEM.as_bytes().to_vec()));
    assert_eq!(direct.ca_pem(), Some(TEST_CA_PEM.as_bytes()));

    let _ = std::fs::remove_file(&path);
}

#[cfg(feature = "tls")]
#[tokio::test]
async fn ca_absent_topology_keeps_default_constructor() {
    let mut cfg =
        RedisEndpointConfig::from_uri("rediss://localhost:6380?command=GET").expect("valid uri");
    cfg.resolve_defaults();

    // No `tls_ca_cert` configured: the factory's CA read yields `None`
    // (and never touches the filesystem).
    assert_eq!(read_standalone_ca_pem(&cfg).expect("ca read"), None);

    let topology =
        topology_from_config(&cfg).unwrap_or_else(|e| panic!("TLS topology without CA: {e}"));
    topology
        .resolve(ServerKind::Master)
        .await
        .expect("client construction must succeed");

    assert!(StandaloneTopology::new(&cfg).ca_pem().is_none());
}

#[tokio::test]
async fn plaintext_endpoint_ignores_configured_ca() {
    let mut cfg =
        RedisEndpointConfig::from_uri("redis://localhost:6379?command=GET").expect("valid uri");
    cfg.resolve_defaults();
    cfg.ssl = Some(false);
    cfg.tls_ca_cert = Some("/nonexistent/ca.pem".into());

    // The CA path is never read for a plaintext endpoint: no filesystem
    // access, no error.
    let topology = topology_from_config(&cfg)
        .unwrap_or_else(|e| panic!("plaintext endpoint must ignore the CA: {e}"));
    topology
        .resolve(ServerKind::Master)
        .await
        .expect("client construction must succeed");
}

#[cfg(feature = "tls")]
#[tokio::test]
async fn unreadable_ca_file_fails_closed() {
    let mut cfg =
        RedisEndpointConfig::from_uri("rediss://localhost:6380?command=GET").expect("valid uri");
    cfg.resolve_defaults();
    cfg.tls_ca_cert = Some("/nonexistent/ca.pem".into());

    // (No `.expect_err`: `Arc<dyn RedisTopology>` is not `Debug`.)
    let err = match topology_from_config(&cfg) {
        Ok(_) => panic!("unreadable CA must fail closed"),
        Err(e) => e,
    };
    let msg = err.to_string();
    assert!(
        msg.contains("/nonexistent/ca.pem"),
        "error must name the CA path: {msg}"
    );
    assert!(
        !crate::config::is_transient_redis_error(&err),
        "unreadable CA is a config error, never transient: {msg}"
    );
}
