//! End-to-end tests for the expired-certificate allowlist build path
//! (bd rc-ishar): loopback TLS handshakes through `crate::build_client`
//! with the allowlist wrapper installed, proving every rejection class
//! from openspec/changes/httpcertallow — wrong host, wrong fingerprint,
//! hostname mismatch, unknown issuer, unbundled CA — and the accept
//! path for a pinned expired leaf, the warn rate limit, and the
//! config-scoping of the allowlist.

use std::io::Cursor;
use std::net::SocketAddr;
use std::time::Duration;

use camel_component_api::Component;
use camel_component_api::NoOpComponentContext;
use camel_component_api::test_support::EnvGuard;

use crate::HttpsComponent;
use crate::build_client;
use crate::config::ExpiredCertAllowEntry;
use crate::config::HttpConfig;
use crate::config::TlsConfig;
use crate::tls_harness::HARNESS_OK_REPLY;
use crate::tls_harness::gen_expired_leaf_signed_by_ca;
use crate::tls_harness::gen_leaf_signed_by_ca;
use crate::tls_harness::gen_leaf_signed_by_ca_san;
use crate::tls_harness::gen_self_signed_server_cert;
use crate::tls_harness::gen_test_ca;
use crate::tls_harness::sha256_fingerprint_hex;
use crate::tls_harness::spawn_tls_server;

/// Bound for every request this test cluster sends — a broken harness
/// fails the test, never hangs it (mirrors tls_tests.rs).
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

/// `rcgen::SanType::IpAddress` for the loopback harness address.
fn ip_san_127() -> rcgen::SanType {
    rcgen::SanType::IpAddress("127.0.0.1".parse().expect("loopback ip")) // allow-unwrap(test)
}

/// Config entry with host, the server's bound (declarative) port, and a
/// `sha256:`-prefixed fingerprint.
fn allow_entry(host: &str, port: u16, fingerprint: &str) -> ExpiredCertAllowEntry {
    ExpiredCertAllowEntry {
        host: host.to_string(),
        port,
        fingerprint: fingerprint.to_string(),
    }
}

/// Config with TLS enabled, loopback SSRF consent, and `entries` as the
/// expired-certificate allowlist.
fn allowlist_config(entries: Vec<ExpiredCertAllowEntry>) -> HttpConfig {
    HttpConfig {
        tls: Some(TlsConfig {
            enabled: true,
            expired_allowlist: entries,
            ..Default::default()
        }),
        allow_internal: true,
        ..Default::default()
    }
}

/// Verifying-TLS config with NO allowlist entries (the inert control).
fn plain_tls_config() -> HttpConfig {
    HttpConfig {
        tls: Some(TlsConfig {
            enabled: true,
            ..Default::default()
        }),
        allow_internal: true,
        ..Default::default()
    }
}

/// DER bytes of the first PEM CERTIFICATE section — for harness helpers
/// that return PEM strings without the rcgen handle.
fn first_der(pem: &str) -> Vec<u8> {
    rustls_pemfile::certs(&mut Cursor::new(pem.as_bytes()))
        .next()
        .expect("leaf pem carries a CERTIFICATE section") // allow-unwrap(test)
        .expect("leaf DER parses") // allow-unwrap(test)
        .as_ref()
        .to_vec()
}

/// Timeout-bounded GET against the harness server; the outer Result is
/// the request outcome, and a timeout surfaces as a panic in here.
async fn send_get(
    client: &reqwest::Client,
    addr: SocketAddr,
) -> Result<reqwest::Response, reqwest::Error> {
    tokio::time::timeout(
        REQUEST_TIMEOUT,
        client.get(format!("https://{addr}/")).send(),
    )
    .await
    .expect("request must complete within the harness timeout") // allow-unwrap(test)
}

/// Asserts a request failed fast with a connect/TLS-class rejection —
/// never a timeout (that would hang at send_get) and never an unrelated
/// transport class.
fn assert_tls_rejection(err: reqwest::Error) {
    let rendered = err.to_string();
    assert!(
        err.is_connect() || rendered.contains("certificate") || rendered.contains("invalid"),
        "expected a connect/TLS-class rejection, got: {rendered}"
    );
}

/// The harness reply's body bytes ("ok") — derived from the shared
/// constant so a reply-contract change cannot strand these assertions.
fn harness_body() -> &'static str {
    HARNESS_OK_REPLY
        .rsplit_once("\r\n\r\n")
        .map(|(_, body)| body)
        .unwrap_or_default() // allow-unwrap(test)
}

/// Counts captured log lines that both name the accepted-expired
/// message and carry `fingerprint` (the `sha256:`-prefixed form the
/// warn logs). The tracing-test buffer is process-wide, so the
/// fingerprint — fresh keys per test — scopes the count to THIS test's
/// own handshakes; sibling tests' warns cannot collide.
fn count_expired_accept_warns(fingerprint: &str) -> usize {
    let buf = tracing_test::internal::global_buf()
        .lock()
        .expect("log buffer lock poisoned"); // allow-unwrap(test)
    String::from_utf8_lossy(&buf)
        .lines()
        .filter(|line| {
            line.contains("accepted expired TLS certificate") && line.contains(fingerprint)
        })
        .count()
}

#[tokio::test]
async fn expired_cert_on_listed_host_handshakes() {
    let ca = gen_test_ca();
    let leaf = gen_expired_leaf_signed_by_ca(&ca, ip_san_127());
    // The pin is computed from the SAME leaf handed to the server.
    let fingerprint = sha256_fingerprint_hex(leaf.cert.der().as_ref());
    let (addr, server) = spawn_tls_server(&leaf.cert_pem, &leaf.key_pem, None).await;

    let config = allowlist_config(vec![allow_entry("127.0.0.1", addr.port(), &fingerprint)]);
    let client = build_client(&config, None).expect("allowlisted client builds"); // allow-unwrap(test)

    let resp = send_get(&client, addr)
        .await
        .expect("expired cert on a listed host must handshake"); // allow-unwrap(test)
    assert_eq!(resp.status().as_u16(), 200);
    let body = resp.text().await.expect("response body"); // allow-unwrap(test)
    assert_eq!(body, harness_body());

    server.abort();
}

#[tokio::test]
async fn expired_cert_on_unlisted_host_rejected() {
    let ca = gen_test_ca();
    let leaf = gen_expired_leaf_signed_by_ca(&ca, ip_san_127());
    let fingerprint = sha256_fingerprint_hex(leaf.cert.der().as_ref());
    let (addr, server) = spawn_tls_server(&leaf.cert_pem, &leaf.key_pem, None).await;

    // The pin matches the leaf's fingerprint but names a different host.
    let config = allowlist_config(vec![allow_entry(
        "example.internal",
        addr.port(),
        &fingerprint,
    )]);
    let client = build_client(&config, None).expect("allowlisted client builds"); // allow-unwrap(test)

    let err = send_get(&client, addr)
        .await
        .expect_err("expired cert on an unlisted host must fail"); // allow-unwrap(test)
    assert_tls_rejection(err);

    server.abort();
}

#[tokio::test]
async fn expired_cert_with_wrong_fingerprint_rejected() {
    let ca = gen_test_ca();
    let leaf = gen_expired_leaf_signed_by_ca(&ca, ip_san_127());
    let other = gen_expired_leaf_signed_by_ca(&ca, ip_san_127());
    let (addr, server) = spawn_tls_server(&leaf.cert_pem, &leaf.key_pem, None).await;

    // Listed host + port, but the fingerprint of a DIFFERENT leaf.
    let config = allowlist_config(vec![allow_entry(
        "127.0.0.1",
        addr.port(),
        &sha256_fingerprint_hex(other.cert.der().as_ref()),
    )]);
    let client = build_client(&config, None).expect("allowlisted client builds"); // allow-unwrap(test)

    let err = send_get(&client, addr)
        .await
        .expect_err("a wrong fingerprint must stay rejected"); // allow-unwrap(test)
    assert_tls_rejection(err);

    server.abort();
}

#[tokio::test]
async fn expired_cert_hostname_mismatch_rejected_even_when_listed() {
    let ca = gen_test_ca();
    let leaf = gen_expired_leaf_signed_by_ca(
        &ca,
        rcgen::SanType::DnsName(
            rcgen::string::Ia5String::try_from("backend.internal").expect("dns san is ia5 string"), // allow-unwrap(test)
        ),
    );
    let fingerprint = sha256_fingerprint_hex(leaf.cert.der().as_ref());
    let (addr, server) = spawn_tls_server(&leaf.cert_pem, &leaf.key_pem, None).await;

    // The pin carries the leaf's exact fingerprint, but the request
    // targets the IP — the wrapper's hostname re-check must reject.
    let config = allowlist_config(vec![allow_entry("127.0.0.1", addr.port(), &fingerprint)]);
    let client = build_client(&config, None).expect("allowlisted client builds"); // allow-unwrap(test)

    let err = send_get(&client, addr)
        .await
        .expect_err("hostname mismatch must stay rejected even when listed"); // allow-unwrap(test)
    assert_tls_rejection(err);

    server.abort();
}

#[tokio::test]
async fn unknown_issuer_still_fails_on_listed_host() {
    let (server_cert, server_key) = gen_self_signed_server_cert();
    let fingerprint = sha256_fingerprint_hex(&first_der(&server_cert));
    let (addr, server) = spawn_tls_server(&server_cert, &server_key, None).await;

    // UNEXPIRED self-signed leaf, fully pinned — only expiry errors are
    // tolerated, so unknown-issuer must propagate.
    let config = allowlist_config(vec![allow_entry("127.0.0.1", addr.port(), &fingerprint)]);
    let client = build_client(&config, None).expect("allowlisted client builds"); // allow-unwrap(test)

    let err = send_get(&client, addr)
        .await
        .expect_err("an unknown issuer must stay rejected on a listed host"); // allow-unwrap(test)
    assert_tls_rejection(err);

    server.abort();
}

#[tokio::test]
async fn cert_chained_to_unbundled_ca_fails_on_allowlist_path() {
    // Owns the spec scenario "Certificate rooted only in the OS trust
    // store fails on the allowlist path" WITHOUT mutating the machine
    // trust store: a UNEXPIRED leaf signed by a rcgen CA that is
    // neither in the bundled Mozilla set nor configured as
    // tls.ca_cert_path. The chain is valid under SOME store but absent
    // from the allowlist path's roots — unknown issuer must reject.
    let unbound_ca = gen_test_ca();
    let (server_cert, server_key) = gen_leaf_signed_by_ca_san(&unbound_ca, ip_san_127());
    let fingerprint = sha256_fingerprint_hex(&first_der(&server_cert));
    let (addr, server) = spawn_tls_server(&server_cert, &server_key, None).await;

    let config = allowlist_config(vec![allow_entry("127.0.0.1", addr.port(), &fingerprint)]);
    let client = build_client(&config, None).expect("allowlisted client builds"); // allow-unwrap(test)

    let err = send_get(&client, addr)
        .await
        .expect_err("a chain unbundled from the configured roots must fail"); // allow-unwrap(test)
    assert_tls_rejection(err);

    server.abort();
}

#[tokio::test]
async fn valid_cert_unaffected_by_allowlist() {
    let ca = gen_test_ca();
    let (server_cert, server_key) =
        gen_leaf_signed_by_ca(&ca, "127.0.0.1".parse().expect("loopback ip")); // allow-unwrap(test)
    let fingerprint = sha256_fingerprint_hex(&first_der(&server_cert));
    let dir = tempfile::tempdir().expect("tempdir"); // allow-unwrap(test)
    let ca_path = dir.path().join("ca.pem");
    std::fs::write(&ca_path, ca.ca_pem.as_bytes()).expect("write ca pem"); // allow-unwrap(test)
    let (addr, server) = spawn_tls_server(&server_cert, &server_key, None).await;

    // VALID leaf + configured CA + a (satisfied) allowlist: the chain
    // validates normally in the wrapper's inner verifier, no tolerance
    // needed.
    let config = HttpConfig {
        tls: Some(TlsConfig {
            enabled: true,
            ca_cert_path: Some(ca_path.display().to_string()),
            expired_allowlist: vec![allow_entry("127.0.0.1", addr.port(), &fingerprint)],
            ..Default::default()
        }),
        allow_internal: true,
        ..Default::default()
    };
    let client = build_client(&config, None).expect("allowlisted client builds"); // allow-unwrap(test)

    let resp = send_get(&client, addr)
        .await
        .expect("a valid cert must handshake on the allowlist path"); // allow-unwrap(test)
    assert_eq!(resp.status().as_u16(), 200);
    let body = resp.text().await.expect("response body"); // allow-unwrap(test)
    assert_eq!(body, harness_body());

    server.abort();
}

#[tokio::test]
async fn allowlist_unset_keeps_current_behavior() {
    let ca = gen_test_ca();
    let leaf = gen_expired_leaf_signed_by_ca(&ca, ip_san_127());
    let (addr, server) = spawn_tls_server(&leaf.cert_pem, &leaf.key_pem, None).await;

    let config = plain_tls_config();
    let client = {
        // Plain platform-verifier build: take the canonical env lock so
        // a sibling's CA-less window cannot starve the root probe.
        let _env = EnvGuard::acquire();
        build_client(&config, None).expect("plain verifying client builds") // allow-unwrap(test)
    };

    // Error CLASS only: on this host the platform verifier rejects the
    // expired leaf; on a CA-less host the webpki fallback would reject
    // it too — either way a connect/TLS failure, never a specific
    // string.
    let err = send_get(&client, addr)
        .await
        .expect_err("an expired cert must fail without the allowlist"); // allow-unwrap(test)
    assert_tls_rejection(err);

    server.abort();
}

#[tokio::test]
async fn empty_allowlist_list_is_inert() {
    let ca = gen_test_ca();
    let leaf = gen_expired_leaf_signed_by_ca(&ca, ip_san_127());
    let (addr, server) = spawn_tls_server(&leaf.cert_pem, &leaf.key_pem, None).await;

    let config = allowlist_config(Vec::new());
    let client = {
        let _env = EnvGuard::acquire();
        build_client(&config, None).expect("plain verifying client builds") // allow-unwrap(test)
    };

    let err = send_get(&client, addr)
        .await
        .expect_err("an empty allowlist must stay inert"); // allow-unwrap(test)
    assert_tls_rejection(err);

    server.abort();
}

#[test]
fn insecure_allowlist_conflict_fails_endpoint_creation() {
    let config = HttpConfig {
        tls: Some(TlsConfig {
            enabled: true,
            insecure: true,
            expired_allowlist: vec![allow_entry(
                "127.0.0.1",
                8443,
                &format!("sha256:{}", "0".repeat(64)),
            )],
            ..Default::default()
        }),
        allow_internal: true,
        ..Default::default()
    };
    let component = HttpsComponent::with_config(config);
    let err = component
        .create_endpoint("https://127.0.0.1:8443/api", &NoOpComponentContext)
        .err()
        .expect("insecure + expired_allowlist must fail endpoint creation"); // allow-unwrap(test)
    let rendered = err.to_string();
    assert!(
        rendered.contains("expired_allowlist"),
        "conflict error must name the option: {rendered}"
    );
}

#[tokio::test]
async fn allowlist_does_not_leak_across_components() {
    let ca = gen_test_ca();
    let leaf = gen_expired_leaf_signed_by_ca(&ca, ip_san_127());
    let fingerprint = sha256_fingerprint_hex(leaf.cert.der().as_ref());
    let (addr, server) = spawn_tls_server(&leaf.cert_pem, &leaf.key_pem, None).await;

    // build_client IS the baking point: two clients from two configs
    // against the same expired-leaf server.
    let pinned = allowlist_config(vec![allow_entry("127.0.0.1", addr.port(), &fingerprint)]);
    let allowlisted = build_client(&pinned, None).expect("allowlisted client builds"); // allow-unwrap(test)
    let plain = {
        let _env = EnvGuard::acquire();
        build_client(&plain_tls_config(), None).expect("plain client builds") // allow-unwrap(test)
    };

    let resp = send_get(&allowlisted, addr)
        .await
        .expect("the allowlisted client must succeed"); // allow-unwrap(test)
    assert_eq!(resp.status().as_u16(), 200);
    let err = send_get(&plain, addr)
        .await
        .expect_err("the non-allowlisted client must reject the expired cert"); // allow-unwrap(test)
    assert_tls_rejection(err);

    server.abort();
}

#[tracing_test::traced_test]
#[tokio::test]
async fn acceptance_warn_is_rate_limited() {
    let ca = gen_test_ca();
    let leaf = gen_expired_leaf_signed_by_ca(&ca, ip_san_127());
    let fingerprint = sha256_fingerprint_hex(leaf.cert.der().as_ref());
    let (addr, server) = spawn_tls_server(&leaf.cert_pem, &leaf.key_pem, None).await;

    let config = allowlist_config(vec![allow_entry("127.0.0.1", addr.port(), &fingerprint)]);
    let client = build_client(&config, None).expect("allowlisted client builds"); // allow-unwrap(test)

    // Two requests through the SAME client, asserting exactly one warn —
    // but with honest bounds. The first request performs a fresh handshake
    // and calls the verifier (expired cert → allowlist accept → warn). The
    // SECOND request may resume the rustls session (default in-memory
    // session store) and skip certificate verification entirely, so it may
    // never reach the verifier; Connection: close only forces fresh TCP,
    // not fresh TLS handshakes. This test therefore proves the UPPER bound:
    // no MORE than one warn per limiter window. The lower-bound semantics —
    // acceptance stays unconditional on every verifier call, the limiter
    // gates only the warn — are pinned by the unit test
    // `wrapper_accepts_expired_cert_on_every_call_within_window` in
    // tls_allowlist.rs.
    for _ in 0..2 {
        let resp = send_get(&client, addr)
            .await
            .expect("request must complete"); // allow-unwrap(test)
        assert_eq!(resp.status().as_u16(), 200);
    }

    assert_eq!(
        count_expired_accept_warns(&fingerprint),
        1,
        "exactly ONE rate-limited warn must carry fingerprint {fingerprint}"
    );

    server.abort();
}

#[tracing_test::traced_test]
#[test]
fn allowlist_info_log_present() {
    let config = allowlist_config(vec![allow_entry(
        "127.0.0.1",
        8443,
        &format!("sha256:{}", "0".repeat(64)),
    )]);
    let _client = build_client(&config, None).expect("allowlisted client builds"); // allow-unwrap(test)

    assert!(
        logs_contain("allowlist active"),
        "the allowlist info line must be captured when allowlist_client runs"
    );
    assert!(
        logs_contain("1 pinned entries"),
        "the info line must name the pinned-entry count"
    );
}
