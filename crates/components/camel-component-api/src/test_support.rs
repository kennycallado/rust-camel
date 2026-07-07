//! Test-only helpers for components + integration tests that need a
//! `RuntimeObservability` stub. Gates behind the `test-support` Cargo feature
//! so it never leaks into production builds.
//!
//! Usage from a downstream component's test mod:
//! ```ignore
//! use camel_component_api::test_support::PanicRuntimeObservability;
//! let rt: Arc<dyn camel_component_api::RuntimeObservability> =
//!     Arc::new(PanicRuntimeObservability);
//! ```

use std::sync::Arc;
use std::time::Duration;

use camel_api::MetricsCollector;

use crate::{HealthCheckRegistry, RuntimeObservability};

/// TLS test helpers: generate ephemeral CA + server certs for integration tests.
/// Uses rcgen (pure Rust, no openssl, no Docker).
pub mod tls {
    use std::path::PathBuf;

    /// Generate a self-signed CA + server cert signed by that CA.
    /// SANs: "localhost", "127.0.0.1", "::1".
    /// Returns (ca_pem, server_cert_pem, server_key_pem).
    pub fn gen_server_cert() -> (String, String, String) {
        use rcgen::{BasicConstraints, CertificateParams, DnType, IsCa, KeyPair};
        use std::net::IpAddr;

        let ca_key = KeyPair::generate().expect("ca keygen");
        let mut ca_params = CertificateParams::default();
        ca_params
            .distinguished_name
            .push(DnType::CommonName, "Test CA");
        ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        let ca_cert = ca_params.self_signed(&ca_key).expect("ca self-sign");

        let server_key = KeyPair::generate().expect("server keygen");
        let mut server_params = CertificateParams::default();
        server_params.subject_alt_names = vec![
            rcgen::SanType::DnsName("localhost".try_into().expect("localhost dns name")),
            rcgen::SanType::IpAddress(IpAddr::V4([127, 0, 0, 1].into())),
            rcgen::SanType::IpAddress("::1".parse().expect("::1 ip address")),
        ];
        let server_cert = server_params
            .signed_by(&server_key, &ca_cert, &ca_key)
            .expect("server cert sign");

        (ca_cert.pem(), server_cert.pem(), server_key.serialize_pem())
    }

    /// Write PEM content to /tmp/camel-tls-test/{name}.
    /// Creates parent dir if needed. Returns the path.
    pub fn write_pem_tmp(name: &str, pem: &str) -> PathBuf {
        let dir = std::env::temp_dir().join("camel-tls-test");
        std::fs::create_dir_all(&dir).expect("create tmp dir");
        let path = dir.join(name);
        std::fs::write(&path, pem).expect("write pem");
        path
    }
}

/// `RuntimeObservability` stub that panics if any method is invoked.
///
/// Use in test mods that exercise Endpoint trait surface but should NOT
/// actually invoke observability methods. Per Phase A spec line 98:
/// "Test fixtures implement `RuntimeObservability` with a stub that panics
/// on use (only observability tests should invoke it)."
#[derive(Debug, Default, Clone, Copy)]
pub struct PanicRuntimeObservability;

impl MetricsCollector for PanicRuntimeObservability {
    fn record_exchange_duration(&self, _: &str, _: Duration) {
        panic!("PanicRuntimeObservability::record_exchange_duration invoked")
    }
    fn increment_errors(&self, _: &str, _: &str) {
        panic!("PanicRuntimeObservability::increment_errors invoked")
    }
    fn increment_exchanges(&self, _: &str) {
        panic!("PanicRuntimeObservability::increment_exchanges invoked")
    }
    fn set_queue_depth(&self, _: &str, _: usize) {
        panic!("PanicRuntimeObservability::set_queue_depth invoked")
    }
    fn record_circuit_breaker_change(&self, _: &str, _: &str, _: &str) {
        panic!("PanicRuntimeObservability::record_circuit_breaker_change invoked")
    }
}

impl HealthCheckRegistry for PanicRuntimeObservability {
    fn force_unhealthy_for_route(&self, _: &str, _: &str, _: &str) {
        panic!("PanicRuntimeObservability::force_unhealthy_for_route invoked")
    }
}

impl RuntimeObservability for PanicRuntimeObservability {
    fn metrics(&self) -> Arc<dyn MetricsCollector> {
        panic!("PanicRuntimeObservability::metrics invoked")
    }
    fn health(&self) -> Arc<dyn HealthCheckRegistry> {
        panic!("PanicRuntimeObservability::health invoked")
    }
}

/// `RuntimeObservability` stub that silently ignores all calls.
///
/// Use for tests that exercise observability paths (e.g., metrics or health
/// calls on error paths) without needing to assert on the values.
/// Contrast with `PanicRuntimeObservability` which panics on any invocation.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopRuntimeObservability;

impl MetricsCollector for NoopRuntimeObservability {
    fn record_exchange_duration(&self, _: &str, _: Duration) {}
    fn increment_errors(&self, _: &str, _: &str) {}
    fn increment_exchanges(&self, _: &str) {}
    fn set_queue_depth(&self, _: &str, _: usize) {}
    fn record_circuit_breaker_change(&self, _: &str, _: &str, _: &str) {}
}

impl HealthCheckRegistry for NoopRuntimeObservability {
    fn force_unhealthy_for_route(&self, _: &str, _: &str, _: &str) {}
}

impl RuntimeObservability for NoopRuntimeObservability {
    fn metrics(&self) -> Arc<dyn MetricsCollector> {
        Arc::new(*self)
    }
    fn health(&self) -> Arc<dyn HealthCheckRegistry> {
        Arc::new(*self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panic_runtime_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<PanicRuntimeObservability>();
    }

    #[test]
    fn noop_runtime_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<NoopRuntimeObservability>();
    }

    #[test]
    fn noop_runtime_metrics_does_not_panic() {
        let rt = NoopRuntimeObservability;
        rt.metrics().increment_errors("any-route", "any-label");
    }

    #[test]
    fn noop_runtime_health_does_not_panic() {
        let rt = NoopRuntimeObservability;
        rt.health()
            .force_unhealthy_for_route("any-route", "any-name", "any-reason");
    }

    #[test]
    fn tls_gen_server_cert_returns_valid_pem() {
        let (ca, cert, key) = super::tls::gen_server_cert();
        assert!(ca.contains("BEGIN CERTIFICATE"), "CA must be PEM cert");
        assert!(
            cert.contains("BEGIN CERTIFICATE"),
            "server cert must be PEM"
        );
        assert!(
            key.contains("PRIVATE KEY"),
            "server key must be PEM private key"
        );
    }

    #[test]
    fn tls_write_pem_tmp_creates_readable_file() {
        let path = super::tls::write_pem_tmp("test-write.pem", "test content");
        let content = std::fs::read_to_string(&path).unwrap();
        assert_eq!(content, "test content");
    }
}
