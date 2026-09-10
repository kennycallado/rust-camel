//! Live `rediss://` coverage: a full cache repository round-trip over TLS
//! plus negative controls proving that a plaintext client is rejected by
//! the TLS-only port, that a TLS client is rejected by a plaintext port,
//! and that a CA which did not sign the server certificate fails
//! verification.
//!
//! **Requires Docker to be running.** Tests will fail if Docker is unavailable.
//!
//! **Requires `integration-tests` feature to compile and run.**

#![cfg(feature = "integration-tests")]

mod support;

use std::time::Duration;

use camel_api::cache::{CacheEntry, CacheRepository, ContentType};
use camel_redis_repo::{RedisCacheRepository, RedisEndpointConfig};
use support::install_crypto_provider;
use support::redis::{shared_redis, shared_redis_tls};

/// Deadline for the positive round-trip: every awaited repository step must
/// finish inside it (ADR-0069 s13 — bounded assertions, no sleeps).
const ROUND_TRIP_DEADLINE: Duration = Duration::from_secs(10);

/// Deadline for each negative control: the rejection must surface fast,
/// never hang the suite.
const NEGATIVE_DEADLINE: Duration = Duration::from_secs(5);

fn cache_entry() -> CacheEntry {
    CacheEntry {
        bytes: b"tls-live-payload".to_vec(),
        payload_path: None,
        content_type: ContentType::Bytes,
        expires_at: None,
    }
}

/// Endpoint for `host:port` with the `rediss://` scheme. `from_uri` sets
/// `ssl` from the scheme; the CA path is set directly on the endpoint
/// config because the repository's `connect_executor` never applies
/// global defaults.
fn rediss_endpoint(host: &str, port: u16, ca_path: Option<String>) -> RedisEndpointConfig {
    let mut endpoint =
        RedisEndpointConfig::from_uri(&format!("rediss://{host}:{port}")).expect("URI parses");
    endpoint.tls_ca_cert = ca_path;
    endpoint
}

/// Runs connect-or-PING against `url` with a raw redis client and returns
/// the resulting error, asserting the failure surfaces within
/// [`NEGATIVE_DEADLINE`] instead of hanging. A multiplexed connection that
/// establishes cleanly is still probed with PING: scheme mismatches often
/// only explode on the first command.
async fn rejected_command_err(url: String) -> redis::RedisError {
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
    .expect("the mismatched-scheme client must be rejected")
}

#[tokio::test(flavor = "multi_thread")]
async fn redis_tls_fixture_boot() {
    install_crypto_provider();
    let fixture = shared_redis_tls().await;

    assert!(fixture.port > 0, "mapped TLS port must be non-zero");
    assert!(
        fixture.ca_pem.starts_with("-----BEGIN CERTIFICATE-----"),
        "ca_pem must be a PEM certificate"
    );
    let on_disk = std::fs::read_to_string(&fixture.ca_file).expect("read CA file");
    assert_eq!(on_disk, fixture.ca_pem, "CA file on disk must match ca_pem");
}

#[tokio::test(flavor = "multi_thread")]
async fn rediss_round_trip_through_cache_repository() {
    install_crypto_provider();
    let fixture = shared_redis_tls().await;
    let endpoint = rediss_endpoint(&fixture.host, fixture.port, Some(fixture.ca_path_string()));

    let repo = tokio::time::timeout(
        ROUND_TRIP_DEADLINE,
        RedisCacheRepository::connect("tls-live", &endpoint, "tlstest", Duration::from_secs(300)),
    )
    .await
    .expect("connect must finish within the deadline")
    .expect("rediss:// endpoint connects with the fixture CA");

    let entry = cache_entry();
    tokio::time::timeout(ROUND_TRIP_DEADLINE, async {
        repo.set("k1", entry.clone(), Some(Duration::from_secs(60)))
            .await
            .expect("set over TLS succeeds");
        let got = repo
            .get("k1")
            .await
            .expect("get over TLS succeeds")
            .expect("entry is present after set");
        assert_eq!(got.bytes, entry.bytes, "payload must survive the TLS path");
        assert_eq!(got.content_type, entry.content_type);

        repo.invalidate("k1")
            .await
            .expect("invalidate over TLS succeeds");
        let after = repo.get("k1").await.expect("get after invalidate succeeds");
        assert!(after.is_none(), "entry must be absent after invalidate");
    })
    .await
    .expect("round-trip must finish within the deadline");
}

#[tokio::test(flavor = "multi_thread")]
async fn plaintext_client_against_tls_port_fails() {
    install_crypto_provider();
    let fixture = shared_redis_tls().await;

    let err = rejected_command_err(format!("redis://{}:{}", fixture.host, fixture.port)).await;
    assert!(
        matches!(
            err.kind(),
            redis::ErrorKind::Io | redis::ErrorKind::Parse | redis::ErrorKind::Server(_)
        ),
        "expected an IO/protocol-level rejection, got: {err}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn rediss_client_against_plaintext_port_fails() {
    install_crypto_provider();
    let plaintext = shared_redis().await;

    let err = rejected_command_err(format!("rediss://{plaintext}")).await;
    assert!(
        matches!(
            err.kind(),
            redis::ErrorKind::Io | redis::ErrorKind::Parse | redis::ErrorKind::Server(_)
        ),
        "expected an IO/protocol-level rejection, got: {err}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn wrong_ca_is_rejected() {
    install_crypto_provider();
    let fixture = shared_redis_tls().await;

    // A second, unrelated CA: the server certificate it did not sign must
    // fail verification, proving the fixture CA is actually consulted.
    // Same rcgen idiom as the fixture bootstrap in support/redis.rs.
    let ca_key = rcgen::KeyPair::generate().expect("wrong-CA key pair");
    let mut ca_params = rcgen::CertificateParams::default();
    ca_params
        .distinguished_name
        .push(rcgen::DnType::CommonName, "rust-camel-test-wrong-ca");
    ca_params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    let ca_cert = ca_params
        .self_signed(&ca_key)
        .expect("wrong CA certificate");

    // The fixture owns its tempdir for the process lifetime; this one only
    // needs to outlive the connect attempt below.
    let ca_dir = tempfile::TempDir::new().expect("wrong-CA tempdir");
    let ca_file = ca_dir.path().join("wrong-ca.pem");
    std::fs::write(&ca_file, ca_cert.pem()).expect("write wrong CA PEM");

    let endpoint = rediss_endpoint(
        &fixture.host,
        fixture.port,
        Some(ca_file.to_str().expect("tempdir path is UTF-8").to_string()),
    );

    let err = tokio::time::timeout(
        NEGATIVE_DEADLINE,
        RedisCacheRepository::connect("tls-live", &endpoint, "tlstest", Duration::from_secs(300)),
    )
    .await
    .expect("connect must finish within the deadline, not hang")
    .expect_err("a CA that did not sign the server certificate must be rejected");

    assert!(
        err.to_string().to_lowercase().contains("certificate"),
        "expected a TLS certificate-verification failure, got: {err}"
    );
}
