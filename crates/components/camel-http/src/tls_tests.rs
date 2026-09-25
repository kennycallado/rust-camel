//! TLS webpki-fallback test cluster, extracted verbatim from
//! lib_tests.rs (mission 272, bd rc-on44c): pure move. Test paths
//! change from `tests::...` to `tls::tests::...`; bodies untouched.

use super::*;
use crate::HttpComponent;
use crate::config::TlsConfig;
use crate::lock_ca_store_test_mutex;
use crate::tls_harness::*;
use camel_component_api::Component;
use camel_component_api::NoOpComponentContext;

#[test]
fn test_webpki_fallback_config_builds_client() {
    // The fallback backend must construct a client on ANY platform —
    // it reads no platform state. This is the exact TLS configuration
    // Termux executes when the primary (platform-verifier) build
    // fails (rc-3j4mq).
    let client = reqwest::Client::builder()
        .tls_backend_preconfigured(webpki_root_client_config())
        .build()
        .expect("webpki-rooted client must build"); // allow-unwrap(test)
    // A built client is usable (internal state initialized); assert
    // via the debug render rather than a network round-trip.
    let rendered = format!("{client:?}");
    assert!(
        !rendered.is_empty(),
        "client must render a debug representation"
    );
}

/// rcgen self-signed one-root CA, written to `ca.pem` inside a fresh
/// tempdir (rcgen 0.14 shapes per camel-component-api test_support).
fn fallback_test_ca() -> (tempfile::TempDir, String) {
    let ca_key = rcgen::KeyPair::generate().expect("ca keygen"); // allow-unwrap(test)
    let mut ca_params = rcgen::CertificateParams::default();
    ca_params
        .distinguished_name
        .push(rcgen::DnType::CommonName, "Fallback test CA");
    ca_params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    let ca_cert = ca_params.self_signed(&ca_key).expect("ca self-sign"); // allow-unwrap(test)
    let dir = tempfile::tempdir().expect("tempdir"); // allow-unwrap(test)
    let ca_path = dir.path().join("ca.pem");
    std::fs::write(&ca_path, ca_cert.pem()).expect("write ca pem"); // allow-unwrap(test)
    (dir, ca_path.display().to_string())
}

/// Valid strict TLS material fixtures (rc-3x5qj): a rcgen one-root
/// CA written to `ca.pem` plus a CA-signed client identity written
/// to `client-cert.pem`/`client-key.pem`, all inside a fresh
/// tempdir. The CA itself is returned too, so handshake tests can
/// sign a server leaf from the SAME CA the config carries — the
/// rebuild-failure tests' precondition (VALID material throughout,
/// so no material error can precede the terminal under test).
/// Named fields for the strict TLS material fixtures (rc-3x5qj):
/// position-only PathBufs in a tuple invited silent cert/key swaps.
struct StrictMaterial {
    dir: tempfile::TempDir,
    ca: TestCa,
    ca_path: std::path::PathBuf,
    cert_path: std::path::PathBuf,
    key_path: std::path::PathBuf,
}

fn strict_material_fixtures() -> StrictMaterial {
    let ca = gen_test_ca();
    let dir = tempfile::tempdir().expect("tempdir"); // allow-unwrap(test)
    let ca_path = dir.path().join("ca.pem");
    std::fs::write(&ca_path, ca.ca_pem.as_bytes()).expect("write ca pem"); // allow-unwrap(test)
    let (client_cert_pem, client_key_pem) = gen_client_identity(&ca);
    let cert_path = dir.path().join("client-cert.pem");
    std::fs::write(&cert_path, client_cert_pem.as_bytes()).expect("write client cert"); // allow-unwrap(test)
    let key_path = dir.path().join("client-key.pem");
    std::fs::write(&key_path, client_key_pem.as_bytes()).expect("write client key"); // allow-unwrap(test)
    StrictMaterial {
        dir,
        ca,
        ca_path,
        cert_path,
        key_path,
    }
}

/// Assert `err` is `EndpointCreationFailed` whose message contains
/// every `fragment`.
fn assert_strict_fallback_error(err: CamelError, fragments: &[&str]) {
    match err {
        CamelError::EndpointCreationFailed(msg) => {
            for fragment in fragments {
                assert!(msg.contains(fragment), "message: {msg}");
            }
        }
        other => panic!("expected EndpointCreationFailed, got {other:?}"),
    }
}

/// Order is hygiene-only for verification, not load-bearing:
/// rustls 0.23.45 `src/webpki/verify.rs:245-263` passes
/// `&roots.roots` to webpki in store order, and rustls-webpki
/// 0.103.15 `src/verify_cert.rs:66-73` +
/// `loop_while_non_fatal_error` (`:757-774`) returns on the FIRST
/// anchor that validates and continues past non-fatal misses, so
/// accept/reject cannot depend on order. Custom-first is pinned
/// anyway for parity with the primary path
/// (rustls-platform-verifier 0.7.0
/// `src/verification/others.rs:81-93` adds extra roots before
/// native certs).
#[test]
fn test_fallback_root_store_union_custom_and_mozilla() {
    let (_dir, ca_path) = fallback_test_ca();
    let store = fallback_root_store(Some(&ca_path), true).expect("union root store must build"); // allow-unwrap(test)
    assert_eq!(
        store.roots.len(),
        webpki_roots::TLS_SERVER_ROOTS.len() + 1,
        "custom CA must UNION with the Mozilla anchors, not replace them"
    );
    let ca_pem = std::fs::read_to_string(&ca_path).expect("read ca pem back"); // allow-unwrap(test)
    let der = rustls_pemfile::certs(&mut std::io::Cursor::new(ca_pem.as_bytes()))
        .next()
        .expect("exactly one CERTIFICATE section in test CA") // allow-unwrap(test)
        .expect("parse test CA pem"); // allow-unwrap(test)
    // Rebuild the expected anchor the same way the production store
    // accepts it, so the equality below is construction-identical.
    let mut scratch = rustls::RootCertStore::empty();
    let (added, _ignored) = scratch.add_parsable_certificates(vec![der]);
    assert_eq!(added, 1, "scratch root store must accept the test CA");
    let expected_anchor = scratch.roots[0].clone();
    assert_eq!(
        store.roots.first(),
        Some(&expected_anchor),
        "custom anchor must sit at index 0 — parity with the primary path's \
         extra-roots-before-platform-roots order"
    );
    assert_eq!(
        store.roots[1..],
        *webpki_roots::TLS_SERVER_ROOTS,
        "everything after the custom anchor must be exactly the bundled Mozilla anchors"
    );
    let first_custom = store
        .roots
        .iter()
        .position(|anchor| anchor == &expected_anchor)
        .expect("custom anchor present in store"); // allow-unwrap(test)
    let first_mozilla = store
        .roots
        .iter()
        .position(|anchor| webpki_roots::TLS_SERVER_ROOTS.contains(anchor))
        .expect("Mozilla anchors present in store"); // allow-unwrap(test)
    assert!(
        first_custom < first_mozilla,
        "first custom anchor (index {first_custom}) must precede the first \
         Mozilla anchor (index {first_mozilla})"
    );
}

#[test]
fn test_fallback_root_store_no_ca_is_mozilla_only() {
    let store = fallback_root_store(None, true).expect("mozilla-only store must build"); // allow-unwrap(test)
    assert_eq!(
        store.roots.len(),
        webpki_roots::TLS_SERVER_ROOTS.len(),
        "no configured CA must leave the bundled Mozilla anchors untouched"
    );
    assert_eq!(
        store.roots,
        *webpki_roots::TLS_SERVER_ROOTS,
        "with no configured CA the whole store IS the bundled Mozilla anchors, \
         byte-for-byte"
    );
}

#[test]
fn test_fallback_root_store_strict_unreadable_ca_fails_closed() {
    let err = match fallback_root_store(Some("/nonexistent/ca-bundle.pem"), true) {
        Err(e) => e,
        Ok(_) => panic!("strict mode must fail closed on an unreadable CA file"),
    };
    assert_strict_fallback_error(err, &["tls.strict/webpki-fallback", "unreadable"]);
}

#[test]
fn test_fallback_root_store_strict_zero_pem_sections_fails_closed() {
    let dir = tempfile::tempdir().expect("tempdir"); // allow-unwrap(test)
    let ca_path = dir.path().join("key-only.pem");
    std::fs::write(
        &ca_path,
        "-----BEGIN PRIVATE KEY-----\nAAAA\n-----END PRIVATE KEY-----\n",
    )
    .expect("write key-only pem"); // allow-unwrap(test)
    let path = ca_path.display().to_string();
    let err = match fallback_root_store(Some(&path), true) {
        Err(e) => e,
        Ok(_) => panic!("strict mode must fail closed on a CA with no CERTIFICATE section"),
    };
    assert_strict_fallback_error(err, &["no parseable PEM CERTIFICATE"]);
}

#[test]
fn test_fallback_root_store_strict_zero_roots_accepted_fails_closed() {
    let dir = tempfile::tempdir().expect("tempdir"); // allow-unwrap(test)
    let ca_path = dir.path().join("garbage-cert.pem");
    std::fs::write(
        &ca_path,
        "-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----\n",
    )
    .expect("write garbage cert pem"); // allow-unwrap(test)
    let path = ca_path.display().to_string();
    let err = match fallback_root_store(Some(&path), true) {
        Err(e) => e,
        Ok(_) => panic!("strict mode must fail closed when the root store rejects the CA"),
    };
    assert_strict_fallback_error(err, &["rejected by the TLS root store"]);
}

#[test]
fn test_fallback_root_store_nonstrict_bad_ca_degrades_to_mozilla() {
    let dir = tempfile::tempdir().expect("tempdir"); // allow-unwrap(test)
    let ca_path = dir.path().join("garbage.pem");
    std::fs::write(&ca_path, "this is not a certificate\n").expect("write garbage pem"); // allow-unwrap(test)
    let path = ca_path.display().to_string();
    let store = fallback_root_store(Some(&path), false)
        .expect("non-strict garbage CA must degrade to Mozilla-only"); // allow-unwrap(test)
    assert_eq!(
        store.roots.len(),
        webpki_roots::TLS_SERVER_ROOTS.len(),
        "non-strict bad CA must degrade to the bundled Mozilla anchors"
    );
}

#[test]
fn test_fallback_client_config_disabled_tls_returns_none() {
    let tls = TlsConfig {
        enabled: false,
        ..Default::default()
    };
    assert!(
        fallback_client_config(&tls)
            .expect("disabled TLS is infallible") // allow-unwrap(test)
            .is_none()
    );
}

#[test]
fn test_fallback_client_config_no_material_verifying_returns_none() {
    let tls = TlsConfig {
        enabled: true,
        ..Default::default()
    };
    assert!(
        fallback_client_config(&tls)
            .expect("plain verifying TLS is infallible") // allow-unwrap(test)
            .is_none()
    );
}

#[test]
fn test_fallback_client_config_strict_identity_garbage_key_fails_closed() {
    let dir = tempfile::tempdir().expect("tempdir"); // allow-unwrap(test)
    let client_key = rcgen::KeyPair::generate().expect("client keygen"); // allow-unwrap(test)
    let client_params = rcgen::CertificateParams::default();
    let client_cert = client_params
        .self_signed(&client_key)
        .expect("client cert self-sign"); // allow-unwrap(test)
    let cert_path = dir.path().join("client.pem");
    std::fs::write(&cert_path, client_cert.pem()).expect("write client pem"); // allow-unwrap(test)
    let key_path = dir.path().join("key.pem");
    std::fs::write(&key_path, "garbage not a key at all\n").expect("write garbage key"); // allow-unwrap(test)
    let tls = TlsConfig {
        enabled: true,
        strict: true,
        client_cert_path: Some(cert_path.display().to_string()),
        client_key_path: Some(key_path.display().to_string()),
        ..Default::default()
    };
    let err = match fallback_client_config(&tls) {
        Err(e) => e,
        Ok(_) => panic!("strict mode must fail closed on a garbage client key"),
    };
    assert_strict_fallback_error(err, &["no parseable private key"]);
}

#[test]
fn test_fallback_client_config_strict_rustls_rejected_key_fails_closed() {
    let dir = tempfile::tempdir().expect("tempdir"); // allow-unwrap(test)
    let client_key = rcgen::KeyPair::generate().expect("client keygen"); // allow-unwrap(test)
    let client_params = rcgen::CertificateParams::default();
    let client_cert = client_params
        .self_signed(&client_key)
        .expect("client cert self-sign"); // allow-unwrap(test)
    let cert_path = dir.path().join("client.pem");
    std::fs::write(&cert_path, client_cert.pem()).expect("write client pem"); // allow-unwrap(test)
    let key_path = dir.path().join("key.pem");
    std::fs::write(
        &key_path,
        "-----BEGIN PRIVATE KEY-----\nAAAA\n-----END PRIVATE KEY-----\n",
    )
    .expect("write pkcs8-framed garbage key"); // allow-unwrap(test)
    let tls = TlsConfig {
        enabled: true,
        strict: true,
        client_cert_path: Some(cert_path.display().to_string()),
        client_key_path: Some(key_path.display().to_string()),
        ..Default::default()
    };
    let err = match fallback_client_config(&tls) {
        Err(e) => e,
        Ok(_) => panic!("strict mode must fail closed when rustls rejects the identity"),
    };
    assert_strict_fallback_error(err, &["rejected by the TLS backend"]);
}

#[test]
fn test_fallback_client_config_half_mtls_pair_strict_fails_nonstrict_no_auth() {
    let dir = tempfile::tempdir().expect("tempdir"); // allow-unwrap(test)
    let cert_path = dir.path().join("client.pem");
    std::fs::write(&cert_path, "cert-only, key missing\n").expect("write placeholder pem"); // allow-unwrap(test)
    let tls_strict = TlsConfig {
        enabled: true,
        strict: true,
        client_cert_path: Some(cert_path.display().to_string()),
        ..Default::default()
    };
    let err = match fallback_client_config(&tls_strict) {
        Err(e) => e,
        Ok(_) => panic!("strict mode must fail closed on a half-configured mTLS pair"),
    };
    assert_strict_fallback_error(err, &["BOTH client_cert_path"]);
    let tls_permissive = TlsConfig {
        enabled: true,
        client_cert_path: Some(cert_path.display().to_string()),
        ..Default::default()
    };
    assert!(
        fallback_client_config(&tls_permissive)
            .expect("non-strict half pair degrades to no-auth") // allow-unwrap(test)
            .is_some()
    );
}

#[test]
fn test_no_verify_verifier_accepts_any_certificate() {
    use rustls::client::danger::ServerCertVerifier;

    let verifier = NoVerifyServerCertVerifier;
    let garbage = rustls::pki_types::CertificateDer::from(vec![0xDE, 0xAD, 0xBE, 0xEF]);
    let server_name =
        rustls::pki_types::ServerName::try_from("localhost").expect("localhost dns name"); // allow-unwrap(test)
    assert!(
        verifier
            .verify_server_cert(
                &garbage,
                &[],
                &server_name,
                &[],
                rustls::pki_types::UnixTime::now()
            )
            .is_ok(),
        "no-verify verifier must accept any certificate without validation"
    );
}

#[test]
fn test_build_client_falls_back_on_empty_platform_ca_store() {
    // Serialize against the primary-path test: the env window below
    // is process-visible and would force its build through the
    // fallback too.
    let _ca_guard = lock_ca_store_test_mutex();
    // Simulate a CA-less platform (Android/Termux, rc-3j4mq):
    // openssl-probe honors SSL_CERT_FILE/SSL_CERT_DIR only when the
    // paths EXIST, so an existing-but-empty file plus an
    // existing-but-empty directory make rustls-native-certs load zero
    // roots — the exact condition that made the platform verifier
    // (and pre-fix startup) fail on Termux. The env window is
    // process-visible: concurrent client builds in other tests also
    // take the fallback, which still builds a working client — no
    // assertion outside this test can distinguish the two paths
    // except via this thread's BUILD_CLIENT_FALLBACKS counter.
    let empty_ca_dir = tempfile::tempdir().expect("tempdir"); // allow-unwrap(test)
    let empty_ca_file = empty_ca_dir.path().join("empty-ca-bundle.pem");
    std::fs::write(&empty_ca_file, b"").expect("write empty CA file"); // allow-unwrap(test)

    // Safety: test-only mutation of process env vars. This test does
    // not spawn threads that read these vars outside the guarded
    // window below; the guard restores previous values before return.
    let (prev_file, prev_dir) = unsafe {
        let prev = (
            std::env::var_os("SSL_CERT_FILE"),
            std::env::var_os("SSL_CERT_DIR"),
        );
        std::env::set_var("SSL_CERT_FILE", &empty_ca_file);
        std::env::set_var("SSL_CERT_DIR", empty_ca_dir.path());
        prev
    };

    let result = std::panic::catch_unwind(|| {
        let fallbacks_before = build_client_fallback_count();
        let _client = build_client(&HttpConfig::default(), None).expect("client must build"); // allow-unwrap(test)
        build_client_fallback_count() - fallbacks_before
    });

    // Restore FIRST so the guard holds even if assertions fail.
    // Safety: restoring the previously-captured values.
    unsafe {
        match prev_file {
            Some(v) => std::env::set_var("SSL_CERT_FILE", v),
            None => std::env::remove_var("SSL_CERT_FILE"),
        }
        match prev_dir {
            Some(v) => std::env::set_var("SSL_CERT_DIR", v),
            None => std::env::remove_var("SSL_CERT_DIR"),
        }
    }

    let fallbacks_taken = result.expect("build_client must not panic on an empty CA store"); // allow-unwrap(test)
    assert_eq!(
        fallbacks_taken, 1,
        "an empty platform CA store must route exactly one build through \
         the webpki fallback (primary build failed, fallback succeeded)"
    );
}

/// Throwaway self-signed root used ONLY to make the native-root
/// store non-empty hermetically (tests need a parseable PEM
/// CERTIFICATE section, nothing more — the private key was discarded
/// at generation and the root signs/trusts nothing). Generated
/// 2026-09-21, self-expiring 2126.
const HERMETIC_TEST_ROOT_PEM: &str = "-----BEGIN CERTIFICATE-----\n\
    MIIDVzCCAj+gAwIBAgIUQ+hB0JPFtHpUKaNwcTB8ijGqEW0wDQYJKoZIhvcNAQEL\n\
    BQAwOjEdMBsGA1UEAwwUY2FtZWwtaHR0cCB0ZXN0IHJvb3QxGTAXBgNVBAoMEGNh\n\
    bWVsLWh0dHAgdGVzdHMwIBcNMjYwOTIxMTEyMzUwWhgPMjEyNjA4MjgxMTIzNTBa\n\
    MDoxHTAbBgNVBAMMFGNhbWVsLWh0dHAgdGVzdCByb290MRkwFwYDVQQKDBBjYW1l\n\
    bC1odHRwIHRlc3RzMIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEAolnF\n\
    SiT/bK8pMl9n50HTQj4oxPZGXK34Q/OSq8TMUEcmhUFzBjzwZKoFqvDIt/e0mSi/\n\
    h8WQlnrHAvYOnsRlpDsfs4yder+lpEYE84OooJjQvj/kREj7ncK6WQKDY1NTGikx\n\
    lF8kIaCrNHNIlUgoSWMR4E6UshLFwSu5lKtcWCeN6FNzGVQ9jxYJSFsmVk+wFyHI\n\
    edvpPbnFkD3M1/GKNVuxCCR50sO+cQeB7w9FCyFdHvoTWWuwPV4Qq2LBM2n4eedG\n\
    pCXkqmuARtFuOKjpYIRryGybic9u9ZW59FbTbgHSj6rcOXNtBUVV/KnWRDRWo7ZC\n\
    cR5SmO2YwtfpISjbmwIDAQABo1MwUTAdBgNVHQ4EFgQUkhwU0Q5JYNLrs7yOqbXE\n\
    szzn2/cwHwYDVR0jBBgwFoAUkhwU0Q5JYNLrs7yOqbXEszzn2/cwDwYDVR0TAQH/\n\
    BAUwAwEB/zANBgkqhkiG9w0BAQsFAAOCAQEAXLqN7poLyxlpxN9vY1jISs403P2I\n\
    edwgX3eWyJgXQfc/+dhnig8Pi7SkJKNM9PM27qain3+e2/HeetNYGBD8xnzQo5ga\n\
    1ScPvm7u2DWIWc5y0bpm5JuCPqXoDl/7kBcaelA2lZRrPJzQuu6uPnH1ubjUPFJj\n\
    9SxWhR+QgJW3cFc17C179WSVSJSPLozxX1ODABSyxNXnz14Rq+MOJi2tEsDggyMm\n\
    txh6GwgVbiSey9c9bvu3B28iPfNpULuxBzxSLHcRrSSoII+46foY0G63V/jx7huY\n\
    Fc2DPwOcO2Ni8fLssMaAnEdWPZaIN/DdjmUFw9XgHE9Eld6aKEXBcRFPzA==\n\
    -----END CERTIFICATE-----";

#[test]
fn test_build_client_primary_path_when_platform_store_non_empty() {
    // The webpki set is a FALLBACK, never a replacement for platform
    // roots (managed fleets keep OS root-program control). Hermetic:
    // SSL_CERT_FILE pinned to a parseable fixture root makes the
    // native store non-empty on ANY host — including genuinely
    // CA-less ones — so the no-fallback assertion cannot depend on
    // the build machine. Serialized against the CA-less sibling test
    // by the CA-store mutex (its env window forces the fallback).
    let _ca_guard = lock_ca_store_test_mutex();
    let ca_dir = tempfile::tempdir().expect("tempdir"); // allow-unwrap(test)
    let ca_file = ca_dir.path().join("hermetic-root.pem");
    std::fs::write(&ca_file, HERMETIC_TEST_ROOT_PEM).expect("write fixture root"); // allow-unwrap(test)

    // Safety: test-only mutation of process env vars, restored
    // before any assertion below.
    let (prev_file, prev_dir) = unsafe {
        let prev = (
            std::env::var_os("SSL_CERT_FILE"),
            std::env::var_os("SSL_CERT_DIR"),
        );
        std::env::set_var("SSL_CERT_FILE", &ca_file);
        std::env::set_var("SSL_CERT_DIR", ca_dir.path());
        prev
    };

    let fallbacks_before = build_client_fallback_count();
    let _client = build_client(&HttpConfig::default(), None).expect("client must build"); // allow-unwrap(test)
    let fallbacks_taken = build_client_fallback_count() - fallbacks_before;

    // Safety: restoring the previously-captured values.
    unsafe {
        match prev_file {
            Some(v) => std::env::set_var("SSL_CERT_FILE", v),
            None => std::env::remove_var("SSL_CERT_FILE"),
        }
        match prev_dir {
            Some(v) => std::env::set_var("SSL_CERT_DIR", v),
            None => std::env::remove_var("SSL_CERT_DIR"),
        }
    }

    assert_eq!(
        fallbacks_taken, 0,
        "with at least one platform root loadable, build_client must \
         use the platform-verifier primary path, never the webpki \
         fallback"
    );
}

#[test]
fn test_build_client_forced_fallback_strict_store_rejected_ca_returns_typed_error() {
    // pemfile-ok / store-rejected CA: the PEM section base64-decodes
    // (rustls-pemfile does not validate DER), so only the root store
    // rejects it — the store-rejection class through the full
    // build_client fallback path.
    let dir = tempfile::tempdir().expect("tempdir"); // allow-unwrap(test)
    let ca_path = dir.path().join("garbage-cert.pem");
    std::fs::write(
        &ca_path,
        "-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----\n",
    )
    .expect("write garbage cert pem"); // allow-unwrap(test)
    let config = HttpConfig {
        tls: Some(TlsConfig {
            enabled: true,
            strict: true,
            ca_cert_path: Some(ca_path.display().to_string()),
            ..Default::default()
        }),
        ..Default::default()
    };

    let (err, fallbacks_taken) = forced_fallback_env(|| {
        let fallbacks_before = build_client_fallback_count();
        let err = match build_client(&config, None) {
            Err(e) => e,
            Ok(_) => panic!("strict mode must fail closed when the root store rejects the CA"),
        };
        (err, build_client_fallback_count() - fallbacks_before)
    });

    assert_strict_fallback_error(
        err,
        &[
            "tls.strict/webpki-fallback",
            "rejected by the TLS root store",
        ],
    );
    assert_eq!(
        fallbacks_taken, 1,
        "the typed error must come from exactly one webpki fallback entry"
    );
}

#[test]
fn test_build_client_forced_fallback_strict_material_rejection_classes() {
    // One fixture set, four strict rejection classes, each routed
    // through the full build_client fallback path.
    let dir = tempfile::tempdir().expect("tempdir"); // allow-unwrap(test)

    // (a) CA material with zero PEM CERTIFICATE sections.
    let plain_text_ca = dir.path().join("plain-text.pem");
    std::fs::write(&plain_text_ca, "this is not a certificate\n").expect("write plain text ca"); // allow-unwrap(test)

    // Valid rcgen client identity shared by the mTLS classes.
    let client_key = rcgen::KeyPair::generate().expect("client keygen"); // allow-unwrap(test)
    let client_cert = rcgen::CertificateParams::default()
        .self_signed(&client_key)
        .expect("client self-sign"); // allow-unwrap(test)
    let cert_path = dir.path().join("client-cert.pem");
    std::fs::write(&cert_path, client_cert.pem()).expect("write client cert"); // allow-unwrap(test)
    let valid_key_path = dir.path().join("client-key.pem");
    std::fs::write(&valid_key_path, client_key.serialize_pem()).expect("write client key"); // allow-unwrap(test)
    let garbage_key_path = dir.path().join("garbage-key.pem");
    std::fs::write(&garbage_key_path, "not a key\n").expect("write garbage key"); // allow-unwrap(test)
    // PEM section parses; rustls rejects the bytes at client-auth
    // configuration.
    let rejected_key_path = dir.path().join("rejected-key.pem");
    std::fs::write(
        &rejected_key_path,
        "-----BEGIN PRIVATE KEY-----\nAAAA\n-----END PRIVATE KEY-----\n",
    )
    .expect("write rejected key"); // allow-unwrap(test)

    struct RejectionCase {
        name: &'static str,
        tls: TlsConfig,
        fragment: &'static str,
    }
    let cases = [
        RejectionCase {
            name: "CA with zero PEM CERTIFICATE sections",
            tls: TlsConfig {
                enabled: true,
                strict: true,
                ca_cert_path: Some(plain_text_ca.display().to_string()),
                ..Default::default()
            },
            fragment: "no parseable PEM CERTIFICATE section",
        },
        RejectionCase {
            name: "client key with no parseable section",
            tls: TlsConfig {
                enabled: true,
                strict: true,
                client_cert_path: Some(cert_path.display().to_string()),
                client_key_path: Some(garbage_key_path.display().to_string()),
                ..Default::default()
            },
            fragment: "no parseable private key section",
        },
        RejectionCase {
            name: "client key rejected by the TLS backend",
            tls: TlsConfig {
                enabled: true,
                strict: true,
                client_cert_path: Some(cert_path.display().to_string()),
                client_key_path: Some(rejected_key_path.display().to_string()),
                ..Default::default()
            },
            fragment: "rejected by the TLS backend",
        },
        RejectionCase {
            name: "unreadable client cert",
            tls: TlsConfig {
                enabled: true,
                strict: true,
                client_cert_path: Some("/nonexistent/client-cert.pem".to_string()),
                client_key_path: Some(valid_key_path.display().to_string()),
                ..Default::default()
            },
            fragment: "unreadable",
        },
    ];

    for case in cases {
        let config = HttpConfig {
            tls: Some(case.tls),
            ..Default::default()
        };
        let err = forced_fallback_env(|| match build_client(&config, None) {
            Err(e) => e,
            Ok(_) => panic!("strict case '{}' must fail closed", case.name),
        });
        assert_strict_fallback_error(err, &["tls.strict/webpki-fallback", case.fragment]);
    }
}

#[test]
fn test_build_client_forced_fallback_nonstrict_bad_ca_still_builds() {
    let dir = tempfile::tempdir().expect("tempdir"); // allow-unwrap(test)
    let ca_path = dir.path().join("garbage.pem");
    std::fs::write(&ca_path, "this is not a certificate\n").expect("write garbage pem"); // allow-unwrap(test)
    let config = HttpConfig {
        tls: Some(TlsConfig {
            enabled: true,
            strict: false,
            ca_cert_path: Some(ca_path.display().to_string()),
            ..Default::default()
        }),
        ..Default::default()
    };

    let (_client, fallbacks_taken) = forced_fallback_env(|| {
        let fallbacks_before = build_client_fallback_count();
        let client = build_client(&config, None)
            .expect("non-strict bad CA must degrade per-item and still build"); // allow-unwrap(test)
        (client, build_client_fallback_count() - fallbacks_before)
    });

    assert_eq!(
        fallbacks_taken, 1,
        "permissive item downgrade still routes exactly one build \
         through the webpki fallback"
    );
}

#[test]
fn test_http_component_with_config_folds_fallback_error_without_panic() {
    let dir = tempfile::tempdir().expect("tempdir"); // allow-unwrap(test)
    let ca_path = dir.path().join("garbage-cert.pem");
    std::fs::write(
        &ca_path,
        "-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----\n",
    )
    .expect("write garbage cert pem"); // allow-unwrap(test)
    let config = HttpConfig {
        tls: Some(TlsConfig {
            enabled: true,
            strict: true,
            ca_cert_path: Some(ca_path.display().to_string()),
            ..Default::default()
        }),
        ..Default::default()
    };

    let endpoint_err = forced_fallback_env(|| {
        // Construction must complete without panic; the build error
        // folds into strict_tls_error and surfaces at endpoint
        // creation.
        let component = HttpComponent::with_config(config);
        component
            .create_endpoint("http://h/p", &NoOpComponentContext)
            .err()
    });

    let err = endpoint_err.expect("folded fallback error must fail endpoint creation"); // allow-unwrap(test)
    assert!(
        err.to_string().contains("rejected by the TLS root store"),
        "folded error must carry the fallback diagnosis: {err}"
    );
}

#[test]
fn test_http_component_new_no_panic_on_empty_ca_store() {
    let endpoint = forced_fallback_env(|| {
        let component = HttpComponent::new();
        component
            .create_endpoint("http://h/p", &NoOpComponentContext)
            .ok()
    });
    let _endpoint = endpoint.expect("material-free webpki fallback must serve endpoints"); // allow-unwrap(test)
}

// ---- Forced-fallback loopback TLS handshake scenarios (castrict 1.3)
//
// Real TLS handshakes against a loopback rustls server prove that
// the configured TLS material is CARRIED into the webpki fallback
// backend: the strict material-free build demonstrably routes
// through the forced webpki fallback (its Mozilla roots reject the
// rcgen CA), while every material-carrying build completes a real
// handshake against a server certified ONLY by the configured CA.
//
// Path forcing (e_glm adjudication, rc-hl9cn — Task 1.3 deviation
// note in openspec/changes/castrict/tasks.md): rustls-platform-
// verifier 0.7.0 merges configured extra roots into the platform
// store FIRST and hard-errors only when the merged store is EMPTY,
// so on Linux an empty `SSL_CERT_FILE`/`SSL_CERT_DIR` window plus a
// valid configured CA keeps the PRIMARY build succeeding (extra-
// roots rescue); the fallback-with-valid-material path is real only
// on verifier-hard-error platforms (android/apple — the Termux
// case). The material-carrying tests below therefore run
// `build_client` under the `force_webpki_fallback` seam (primary
// skipped, fallback entered directly; the trigger warn's error
// field reads "forced webpki fallback entry (test)") and prove path
// control with a `build_client_fallback_count()` delta of exactly
// one. The material-free test keeps the REAL env window — no extra
// roots exist to rescue the verifier, so its fallback entry is
// genuine. Every send is timeout-bounded, so a broken harness
// fails fast instead of hanging.

#[tracing_test::traced_test]
#[tokio::test]
async fn tls_handshake_strict_custom_ca_carried() {
    let ca = gen_test_ca();
    let dir = tempfile::tempdir().expect("tempdir"); // allow-unwrap(test)
    let ca_path = dir.path().join("ca.pem");
    std::fs::write(&ca_path, ca.ca_pem.as_bytes()).expect("write ca pem"); // allow-unwrap(test)
    let (server_cert, server_key) =
        gen_leaf_signed_by_ca(&ca, "127.0.0.1".parse().expect("loopback ip")); // allow-unwrap(test)
    let (addr, server) = spawn_tls_server(&server_cert, &server_key, None).await;

    let config = HttpConfig {
        tls: Some(TlsConfig {
            enabled: true,
            strict: true,
            ca_cert_path: Some(ca_path.display().to_string()),
            ..Default::default()
        }),
        ..Default::default()
    };
    let client = build_under_seam(&config);

    let resp = tokio::time::timeout(
        Duration::from_secs(15),
        client.get(format!("https://{addr}/")).send(),
    )
    .await
    .expect("request must complete within the harness timeout") // allow-unwrap(test)
    .expect("strict webpki fallback must carry the configured CA into the handshake"); // allow-unwrap(test)
    assert_eq!(resp.status().as_u16(), 200);
    let body = resp.text().await.expect("response body"); // allow-unwrap(test)
    assert_eq!(body, "ok");

    // Path control beyond the counter: the strict material-carried
    // info log fires inside the fallback (scope-filtered
    // `logs_contain`, injected by #[traced_test], so sibling tests'
    // events cannot pollute this assertion).
    assert!(
        logs_contain("carries configured TLS material"),
        "strict webpki fallback with configured material must log the carry"
    );

    server.abort();
}

#[tracing_test::traced_test]
#[tokio::test]
async fn tls_handshake_strict_material_free_fails_same_server() {
    let ca = gen_test_ca();
    let (server_cert, server_key) =
        gen_leaf_signed_by_ca(&ca, "127.0.0.1".parse().expect("loopback ip")); // allow-unwrap(test)
    let (addr, server) = spawn_tls_server(&server_cert, &server_key, None).await;

    let config = HttpConfig {
        tls: Some(TlsConfig {
            enabled: true,
            strict: true,
            ..Default::default()
        }),
        ..Default::default()
    };
    let client = forced_fallback_env(|| {
        build_client(&config, None).expect("client must build") // allow-unwrap(test)
    });

    let send_result = tokio::time::timeout(
        Duration::from_secs(15),
        client.get(format!("https://{addr}/")).send(),
    )
    .await
    .expect("request must complete within the harness timeout"); // allow-unwrap(test)
    assert!(
        send_result.is_err(),
        "material-free strict fallback trusts only Mozilla roots, which do not \
         trust the rcgen CA — the handshake must fail"
    );

    // No TLS material configured: no per-item degradation warns
    // (spec scenario — material-free strict fallback). Scope-
    // filtered `logs_contain` (injected by #[traced_test]) so
    // sibling tests' mTLS-degrade warns cannot pollute the
    // negations; the positive control proves capture works.
    assert!(
        logs_contain("HTTP client build failed on platform TLS roots"),
        "positive control: this test's own fallback warn must be captured"
    );
    assert!(
        !logs_contain("client certificate NOT used"),
        "no identity was configured — the identity degrade warn must not fire"
    );
    assert!(
        !logs_contain("falling back to bundled Mozilla roots"),
        "no CA was configured — the CA degrade warn must not fire"
    );

    server.abort();
}

#[tracing_test::traced_test]
#[tokio::test]
async fn tls_handshake_strict_mtls_identity_carried() {
    let ca = gen_test_ca();
    let dir = tempfile::tempdir().expect("tempdir"); // allow-unwrap(test)
    let ca_path = dir.path().join("ca.pem");
    std::fs::write(&ca_path, ca.ca_pem.as_bytes()).expect("write ca pem"); // allow-unwrap(test)
    let (client_cert_pem, client_key_pem) = gen_client_identity(&ca);
    let client_cert_path = dir.path().join("client-cert.pem");
    std::fs::write(&client_cert_path, client_cert_pem.as_bytes()).expect("write client cert"); // allow-unwrap(test)
    let client_key_path = dir.path().join("client-key.pem");
    std::fs::write(&client_key_path, client_key_pem.as_bytes()).expect("write client key"); // allow-unwrap(test)
    let (server_cert, server_key) =
        gen_leaf_signed_by_ca(&ca, "127.0.0.1".parse().expect("loopback ip")); // allow-unwrap(test)
    // The server REQUIRES client certificates verified against the
    // same CA that signed the identity.
    let (addr, server) = spawn_tls_server(&server_cert, &server_key, Some(&ca.ca_pem)).await;

    let config = HttpConfig {
        tls: Some(TlsConfig {
            enabled: true,
            strict: true,
            ca_cert_path: Some(ca_path.display().to_string()),
            client_cert_path: Some(client_cert_path.display().to_string()),
            client_key_path: Some(client_key_path.display().to_string()),
            ..Default::default()
        }),
        ..Default::default()
    };
    let client = build_under_seam(&config);

    let resp = tokio::time::timeout(
        Duration::from_secs(15),
        client.get(format!("https://{addr}/")).send(),
    )
    .await
    .expect("request must complete within the harness timeout") // allow-unwrap(test)
    .expect("strict webpki fallback must carry the mTLS identity into the handshake"); // allow-unwrap(test)
    assert_eq!(resp.status().as_u16(), 200);
    let body = resp.text().await.expect("response body"); // allow-unwrap(test)
    assert_eq!(body, "ok");

    // Path control beyond the counter: the strict material-carried
    // info log fires inside the fallback (scope-filtered
    // `logs_contain`, injected by #[traced_test]).
    assert!(
        logs_contain("carries configured TLS material"),
        "strict webpki fallback with configured material must log the carry"
    );

    server.abort();
}

#[tokio::test]
async fn tls_handshake_strict_mtls_less_client_rejected() {
    let ca = gen_test_ca();
    let dir = tempfile::tempdir().expect("tempdir"); // allow-unwrap(test)
    let ca_path = dir.path().join("ca.pem");
    std::fs::write(&ca_path, ca.ca_pem.as_bytes()).expect("write ca pem"); // allow-unwrap(test)
    let (server_cert, server_key) =
        gen_leaf_signed_by_ca(&ca, "127.0.0.1".parse().expect("loopback ip")); // allow-unwrap(test)
    let (addr, server) = spawn_tls_server(&server_cert, &server_key, Some(&ca.ca_pem)).await;

    // Same CA-carried strict config but WITHOUT the client
    // identity: the handshake must fail server-side.
    let config = HttpConfig {
        tls: Some(TlsConfig {
            enabled: true,
            strict: true,
            ca_cert_path: Some(ca_path.display().to_string()),
            ..Default::default()
        }),
        ..Default::default()
    };
    let client = build_under_seam(&config);

    let send_result = tokio::time::timeout(
        Duration::from_secs(15),
        client.get(format!("https://{addr}/")).send(),
    )
    .await
    .expect("request must complete within the harness timeout"); // allow-unwrap(test)
    assert!(
        send_result.is_err(),
        "the server requires client certificates — an identity-less client \
         must be rejected at the handshake"
    );

    server.abort();
}

#[tracing_test::traced_test]
#[tokio::test]
async fn tls_handshake_nonstrict_valid_ca_carried() {
    let ca = gen_test_ca();
    let dir = tempfile::tempdir().expect("tempdir"); // allow-unwrap(test)
    let ca_path = dir.path().join("ca.pem");
    std::fs::write(&ca_path, ca.ca_pem.as_bytes()).expect("write ca pem"); // allow-unwrap(test)
    let (server_cert, server_key) =
        gen_leaf_signed_by_ca(&ca, "127.0.0.1".parse().expect("loopback ip")); // allow-unwrap(test)
    let (addr, server) = spawn_tls_server(&server_cert, &server_key, None).await;

    let config = HttpConfig {
        tls: Some(TlsConfig {
            enabled: true,
            strict: false,
            ca_cert_path: Some(ca_path.display().to_string()),
            ..Default::default()
        }),
        ..Default::default()
    };
    let client = build_under_seam(&config);

    let resp = tokio::time::timeout(
        Duration::from_secs(15),
        client.get(format!("https://{addr}/")).send(),
    )
    .await
    .expect("request must complete within the harness timeout") // allow-unwrap(test)
    .expect("non-strict webpki fallback must carry the valid configured CA into the handshake"); // allow-unwrap(test)
    assert_eq!(resp.status().as_u16(), 200);
    let body = resp.text().await.expect("response body"); // allow-unwrap(test)
    assert_eq!(body, "ok");

    // Loadable material never degrades: no per-item warns even
    // outside strict mode (spec scenario — valid material carried
    // regardless of strict). Scope-filtered `logs_contain`
    // (injected by #[traced_test]) so sibling tests' mTLS-degrade
    // warns cannot pollute the negations; the counter delta above
    // is the positive path control. The platform warn DOES fire
    // under the seam — that is the Forced-trigger notice, not a
    // material warn; the negations below target the per-item
    // material warns only.
    assert!(
        !logs_contain("client certificate NOT used"),
        "no identity was configured — the identity degrade warn must not fire"
    );
    assert!(
        !logs_contain("falling back to bundled Mozilla roots"),
        "the CA loaded cleanly — the CA degrade warn must not fire"
    );

    server.abort();
}

/// Genuine-trigger anchor for the force seam (castrict 1.3-deviation,
/// Option C, weakened variant): strict mTLS identity WITHOUT
/// `ca_cert_path`, verification ON, under the REAL env window — no
/// force seam, and no extra roots exist to rescue the verifier, so
/// the primary genuinely fails. The prescribed 200-ok variant is
/// architecturally unreachable: reqwest's `!certs_verification`
/// branch constructs the NoVerifier verifier and never builds the
/// platform verifier, so a verify-off config cannot genuinely fail
/// the primary on ANY platform; and every verify-on genuine fallback
/// trusts only Mozilla ∪ custom roots, which cannot handshake with a
/// hermetic rcgen CA. Handshake-level identity proof therefore lives
/// in the seam tests, which share all downstream code from
/// `fallback_client_config` onward.
#[tracing_test::traced_test]
#[tokio::test]
async fn tls_handshake_mtls_identity_only_genuine_fallback_anchor() {
    let ca = gen_test_ca();
    let dir = tempfile::tempdir().expect("tempdir"); // allow-unwrap(test)
    let (client_cert_pem, client_key_pem) = gen_client_identity(&ca);
    let client_cert_path = dir.path().join("client-cert.pem");
    std::fs::write(&client_cert_path, client_cert_pem.as_bytes()).expect("write client cert"); // allow-unwrap(test)
    let client_key_path = dir.path().join("client-key.pem");
    std::fs::write(&client_key_path, client_key_pem.as_bytes()).expect("write client key"); // allow-unwrap(test)

    let config = HttpConfig {
        tls: Some(TlsConfig {
            enabled: true,
            strict: true,
            client_cert_path: Some(client_cert_path.display().to_string()),
            client_key_path: Some(client_key_path.display().to_string()),
            ..Default::default()
        }),
        ..Default::default()
    };
    let (client, fallbacks_taken) = {
        let fallbacks_before = build_client_fallback_count();
        let client = forced_fallback_env(|| {
            build_client(&config, None).expect("client must build") // allow-unwrap(test)
        });
        (client, build_client_fallback_count() - fallbacks_before)
    };
    assert_eq!(
        fallbacks_taken, 1,
        "the empty-store env window must genuinely fail the platform-verifier primary"
    );

    // The strict identity is carried into the fallback config
    // (`with_client_auth_cert` accepted it) — the build is Ok, not
    // fail-closed. No handshake is attempted: the fallback client
    // trusts only Mozilla roots client-side, which cannot verify the
    // hermetic rcgen CA, so a send would fail by design (see the
    // comment above).
    drop(client);

    // Trigger authenticity: the REAL platform warn (not the Forced
    // seam notice) fires inside the fallback. Scope-filtered
    // `logs_contain` (injected by #[traced_test]) so sibling tests'
    // events cannot pollute the assertion.
    assert!(
        logs_contain("HTTP client build failed on platform TLS roots"),
        "the genuine env-window trigger must emit the real platform warn"
    );
}

// Task 1.4 (castrict): verification-parity coverage. The two
// danger-mode parity tests prove a verify-off fallback config
// handshakes with a self-signed server the Mozilla/platform roots
// cannot trust; the remaining three prove strict material failures
// still fail closed, non-strict degradation stays item-wise, and a
// verifying fallback rejects the same self-signed server — the
// control showing the parity tests pass BECAUSE of the danger
// verifier, not because the harness skips verification. All five
// build under the FORCE_WEBPKI_FALLBACK seam: danger mode makes
// reqwest construct the NoVerifier branch and never build the
// platform verifier, so the primary cannot genuinely fail for
// verify-off configs on ANY platform (1.3-deviation note in
// openspec/changes/castrict/tasks.md).

/// Self-signed (NOT CA-signed) server leaf: signer unrelated to any
/// configured CA — exactly what the parity scenarios need.
#[tokio::test]
async fn tls_parity_insecure_self_signed_succeeds() {
    let (server_cert, server_key) = gen_self_signed_server_cert();
    let (addr, server) = spawn_tls_server(&server_cert, &server_key, None).await;

    let config = HttpConfig {
        tls: Some(TlsConfig {
            enabled: true,
            strict: false,
            insecure: true,
            ..Default::default()
        }),
        ..Default::default()
    };
    let client = build_under_seam(&config);

    let resp = tokio::time::timeout(
        Duration::from_secs(15),
        client.get(format!("https://{addr}/")).send(),
    )
    .await
    .expect("request must complete within the harness timeout") // allow-unwrap(test)
    .expect("verify-off (insecure) fallback must handshake with the self-signed server"); // allow-unwrap(test)
    assert_eq!(resp.status().as_u16(), 200);
    let body = resp.text().await.expect("response body"); // allow-unwrap(test)
    assert_eq!(body, "ok");

    server.abort();
}

#[tokio::test]
async fn tls_parity_verify_peer_false_self_signed_succeeds() {
    let (server_cert, server_key) = gen_self_signed_server_cert();
    let (addr, server) = spawn_tls_server(&server_cert, &server_key, None).await;

    let config = HttpConfig {
        tls: Some(TlsConfig {
            enabled: true,
            strict: false,
            verify_peer: false,
            ..Default::default()
        }),
        ..Default::default()
    };
    let client = build_under_seam(&config);

    let resp = tokio::time::timeout(
        Duration::from_secs(15),
        client.get(format!("https://{addr}/")).send(),
    )
    .await
    .expect("request must complete within the harness timeout") // allow-unwrap(test)
    .expect("verify_peer=false fallback must handshake with the self-signed server"); // allow-unwrap(test)
    assert_eq!(resp.status().as_u16(), 200);
    let body = resp.text().await.expect("response body"); // allow-unwrap(test)
    assert_eq!(body, "ok");

    server.abort();
}

#[tokio::test]
async fn tls_parity_insecure_strict_bad_material_still_fails_closed() {
    // No server needed: the build must fail before any connection.
    let dir = tempfile::tempdir().expect("tempdir"); // allow-unwrap(test)
    // Store-rejected fixture: a PEM CERTIFICATE section whose body
    // is not base64 — the root store accepts 0 of 1 certs.
    let ca_path = dir.path().join("rejected-ca.pem");
    std::fs::write(
        &ca_path,
        b"-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----\n",
    )
    .expect("write rejected ca fixture"); // allow-unwrap(test)

    let config = HttpConfig {
        tls: Some(TlsConfig {
            enabled: true,
            strict: true,
            insecure: true,
            ca_cert_path: Some(ca_path.display().to_string()),
            ..Default::default()
        }),
        ..Default::default()
    };
    // Raw force-guard + match: build_under_seam expects Ok, and the
    // strict material failure must surface as Err before any send —
    // verification-disable never masks strict material failure.
    let err = match force_webpki_fallback(|| build_client(&config, None)) {
        Err(e) => e,
        Ok(_) => panic!(
            "strict mode must fail closed on store-rejected CA material \
             even with insecure=true"
        ),
    };
    assert_strict_fallback_error(err, &["tls.strict/webpki-fallback"]);
}

#[test]
fn strict_rebuild_failure_returns_typed_error() {
    let StrictMaterial {
        dir: _dir,
        ca_path,
        cert_path,
        key_path,
        ..
    } = strict_material_fixtures();

    // VALID material throughout: no material error can precede the
    // rebuild terminal, so any typed Err must come from the
    // strict-fail-closed second-error terminal itself.
    let config = HttpConfig {
        tls: Some(TlsConfig {
            enabled: true,
            strict: true,
            ca_cert_path: Some(ca_path.display().to_string()),
            client_cert_path: Some(cert_path.display().to_string()),
            client_key_path: Some(key_path.display().to_string()),
            ..Default::default()
        }),
        ..Default::default()
    };
    let fallbacks_before = build_client_fallback_count();
    let err = match force_webpki_fallback_rebuild_failure(|| build_client(&config, None)) {
        Err(e) => e,
        Ok(_) => panic!(
            "strict mode must fail closed when the webpki fallback client \
             rebuild fails — never the material-free emergency client"
        ),
    };
    assert_eq!(
        build_client_fallback_count() - fallbacks_before,
        1,
        "the rebuild-failure call must route through the webpki fallback exactly once"
    );
    assert_strict_fallback_error(
        err,
        &[
            "tls.strict/webpki-fallback:",
            "webpki fallback client rebuild failed",
            "forced webpki fallback rebuild failure",
        ],
    );
}

#[test]
fn non_strict_rebuild_failure_returns_emergency_client() {
    let StrictMaterial {
        dir: _dir,
        ca_path,
        cert_path,
        key_path,
        ..
    } = strict_material_fixtures();

    let config = HttpConfig {
        tls: Some(TlsConfig {
            enabled: true,
            strict: false,
            ca_cert_path: Some(ca_path.display().to_string()),
            client_cert_path: Some(cert_path.display().to_string()),
            client_key_path: Some(key_path.display().to_string()),
            ..Default::default()
        }),
        ..Default::default()
    };
    let fallbacks_before = build_client_fallback_count();
    let _client = force_webpki_fallback_rebuild_failure(|| build_client(&config, None))
        .expect("non-strict rebuild failure must degrade to the emergency client"); // allow-unwrap(test)
    assert_eq!(
        build_client_fallback_count() - fallbacks_before,
        1,
        "the rebuild-failure call must route through the webpki fallback exactly once"
    );
}

#[test]
fn strict_rebuild_failure_surfaces_at_endpoint_creation() {
    let StrictMaterial {
        dir: _dir,
        ca_path,
        cert_path,
        key_path,
        ..
    } = strict_material_fixtures();

    // VALID strict config: no material error can precede the
    // rebuild terminal, so any folded Err must come from the
    // strict-fail-closed rebuild terminal itself.
    let config = HttpConfig {
        tls: Some(TlsConfig {
            enabled: true,
            strict: true,
            ca_cert_path: Some(ca_path.display().to_string()),
            client_cert_path: Some(cert_path.display().to_string()),
            client_key_path: Some(key_path.display().to_string()),
            ..Default::default()
        }),
        ..Default::default()
    };

    let endpoint_err = force_webpki_fallback_rebuild_failure(|| {
        // Construction must complete without panic; the rebuild-
        // failure error folds into strict_tls_error and surfaces at
        // endpoint creation (mirrors the forced-fallback fold test).
        let component = HttpComponent::with_config(config);
        component
            .create_endpoint("http://h/p", &NoOpComponentContext)
            .err()
    });

    let err = endpoint_err.expect("folded rebuild failure must fail endpoint creation"); // allow-unwrap(test)
    assert_strict_fallback_error(
        err,
        &[
            "tls.strict/webpki-fallback:",
            "webpki fallback client rebuild failed",
        ],
    );
}

#[tracing_test::traced_test]
#[tokio::test]
async fn non_strict_rebuild_failure_drops_custom_trust() {
    let StrictMaterial {
        dir: _dir,
        ca,
        ca_path,
        ..
    } = strict_material_fixtures();

    // Server certified ONLY by the same CA the config carries: a
    // material-carrying fallback build completes this handshake,
    // the emergency client cannot.
    let (server_cert, server_key) =
        gen_leaf_signed_by_ca(&ca, "127.0.0.1".parse().expect("loopback ip")); // allow-unwrap(test)
    let (addr, server) = spawn_tls_server(&server_cert, &server_key, None).await;

    let config = HttpConfig {
        tls: Some(TlsConfig {
            enabled: true,
            strict: false,
            ca_cert_path: Some(ca_path.display().to_string()),
            ..Default::default()
        }),
        ..Default::default()
    };
    // Arrange phase — build BOTH clients first, each inside its own
    // seam guard scope (the guards must not nest); requests run
    // AFTER both scopes close, matching the isolation pattern of
    // the parity tests.
    //
    // CONTROL: forced fallback WITHOUT the rebuild-failure seam —
    // the normal fallback build honors ca_cert_path, so this client
    // carries the CA and verifies the server.
    let control = force_webpki_fallback(|| {
        build_client(&config, None).expect("control client must build") // allow-unwrap(test)
    });
    // EMERGENCY: both seams armed — the rebuild-failure
    // substitution drops the custom trust, leaving Mozilla roots
    // only.
    let client = force_webpki_fallback_rebuild_failure(|| {
        build_client(&config, None).expect("emergency client must build") // allow-unwrap(test)
    });

    // Assert phase — same server, and the ONLY delta between the
    // two clients is the rebuild-failure substitution: control
    // succeeds (positive control), emergency fails (causal trust
    // loss, not any incidental network condition).
    let control_resp = tokio::time::timeout(
        Duration::from_secs(15),
        control.get(format!("https://{addr}/")).send(),
    )
    .await
    .expect("request must complete within the harness timeout") // allow-unwrap(test)
    .expect("the control client carries the CA — the CA-signed handshake must succeed"); // allow-unwrap(test)
    assert_eq!(control_resp.status().as_u16(), 200);

    let send_result = tokio::time::timeout(
        Duration::from_secs(15),
        client.get(format!("https://{addr}/")).send(),
    )
    .await
    .expect("request must complete within the harness timeout"); // allow-unwrap(test)
    assert!(
        send_result.is_err(),
        "the emergency client carries Mozilla roots only — the configured \
         CA trust is provably dropped, so the CA-signed handshake must fail"
    );

    // The rebuild terminal's system-broken error line is unchanged
    // by this change. Scope-filtered `logs_contain` (injected by
    // #[traced_test]) so sibling tests' events cannot pollute.
    assert!(
        logs_contain("webpki fallback client build failed — TLS stack broken process-wide"),
        "the rebuild-failure terminal must log the system-broken diagnosis"
    );

    server.abort();
}

#[tracing_test::traced_test]
#[tokio::test]
async fn tls_parity_nonstrict_itemwise_valid_ca_bad_identity() {
    let ca = gen_test_ca();
    let dir = tempfile::tempdir().expect("tempdir"); // allow-unwrap(test)
    let ca_path = dir.path().join("ca.pem");
    std::fs::write(&ca_path, ca.ca_pem.as_bytes()).expect("write ca pem"); // allow-unwrap(test)
    let (client_cert_pem, _client_key_pem) = gen_client_identity(&ca);
    let client_cert_path = dir.path().join("client-cert.pem");
    std::fs::write(&client_cert_path, client_cert_pem.as_bytes()).expect("write client cert"); // allow-unwrap(test)
    let (server_cert, server_key) =
        gen_leaf_signed_by_ca(&ca, "127.0.0.1".parse().expect("loopback ip")); // allow-unwrap(test)
    // No client-auth requirement: the identity item's fate is
    // client-side only.
    let (addr, server) = spawn_tls_server(&server_cert, &server_key, None).await;

    let config = HttpConfig {
        tls: Some(TlsConfig {
            enabled: true,
            strict: false,
            ca_cert_path: Some(ca_path.display().to_string()),
            client_cert_path: Some(client_cert_path.display().to_string()),
            // UNREADABLE key path per the spec scenario: the mTLS
            // pair cannot load, so the identity item degrades.
            client_key_path: Some("/nonexistent/key.pem".to_string()),
            ..Default::default()
        }),
        ..Default::default()
    };
    let client = build_under_seam(&config);

    let resp = tokio::time::timeout(
        Duration::from_secs(15),
        client.get(format!("https://{addr}/")).send(),
    )
    .await
    .expect("request must complete within the harness timeout") // allow-unwrap(test)
    .expect("the valid CA item must be carried despite the unreadable key"); // allow-unwrap(test)
    assert_eq!(resp.status().as_u16(), 200);
    let body = resp.text().await.expect("response body"); // allow-unwrap(test)
    assert_eq!(body, "ok");

    // Item-wise degradation: the identity item warned and was
    // dropped; the CA item loaded cleanly — no Mozilla-roots
    // downgrade. Scope-filtered `logs_contain` (injected by
    // #[traced_test]) so sibling tests' events cannot pollute.
    assert!(
        logs_contain("client certificate NOT used"),
        "the unreadable key must degrade the identity item with the mTLS warn"
    );
    assert!(
        !logs_contain("falling back to bundled Mozilla roots"),
        "the CA loaded cleanly — the CA degrade warn must not fire"
    );

    server.abort();
}

#[tokio::test]
async fn tls_parity_verifying_client_rejects_self_signed_without_insecure() {
    let (server_cert, server_key) = gen_self_signed_server_cert();
    let (addr, server) = spawn_tls_server(&server_cert, &server_key, None).await;

    // Strict, NO material, verification ON: the fallback trusts
    // only Mozilla roots, which cannot verify the self-signed
    // server — the control proving the two parity tests above pass
    // BECAUSE of the danger verifier, not because the harness
    // skips verification.
    let config = HttpConfig {
        tls: Some(TlsConfig {
            enabled: true,
            strict: true,
            ..Default::default()
        }),
        ..Default::default()
    };
    let client = build_under_seam(&config);

    let send_result = tokio::time::timeout(
        Duration::from_secs(15),
        client.get(format!("https://{addr}/")).send(),
    )
    .await
    .expect("request must complete within the harness timeout"); // allow-unwrap(test)
    assert!(
        send_result.is_err(),
        "a verifying client must reject the self-signed server at the handshake"
    );

    server.abort();
}
