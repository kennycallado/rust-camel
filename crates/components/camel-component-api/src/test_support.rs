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
    use std::sync::atomic::{AtomicU64, Ordering};

    /// Per-call counter for unique temp filenames (avoids collision when tests run in parallel).
    static PEM_COUNTER: AtomicU64 = AtomicU64::new(0);

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
        // rcgen 0.14: signing needs an Issuer built from the CA params + key
        // (the old signed_by(key, ca_cert, ca_key) 3-arg form is gone).
        let issuer = rcgen::Issuer::from_params(&ca_params, &ca_key);
        let server_cert = server_params
            .signed_by(&server_key, &issuer)
            .expect("server cert sign");

        (ca_cert.pem(), server_cert.pem(), server_key.serialize_pem())
    }

    /// Write PEM content to a unique temp file, return the path.
    /// Each call produces a distinct filename (pid + atomic counter) so parallel
    /// tests don't race on the same path.
    pub fn write_pem_tmp(name: &str, pem: &str) -> PathBuf {
        let dir = std::env::temp_dir().join("camel-tls-test");
        std::fs::create_dir_all(&dir).expect("create tmp dir");
        let counter = PEM_COUNTER.fetch_add(1, Ordering::Relaxed);
        let stem = std::path::Path::new(name)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(name);
        let ext = std::path::Path::new(name)
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("pem");
        let unique_name = format!("{stem}_{}_{counter}.{ext}", std::process::id());
        let path = dir.join(unique_name);
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

/// Uniform acquisition deadline for process-global test-serialization
/// locks: 900 s. Derivation — a waiting test's healthy worst case is
/// (holders − 1) × longest holder inside its single test binary; the
/// largest chain is camel-ws `REGISTRY_TEST_LOCK` at 602 s (43 × 14 s,
/// 5 s connect bound + short body per holder). 900 s ≈ 1.5× margin for
/// CI load jitter, so an acquisition timeout indicates a stalled
/// holder, not queue depth (bd rc-88old).
pub const TEST_LOCK_DEADLINE: std::time::Duration = std::time::Duration::from_secs(900);

/// Acquire a process-global test-serialization lock with a deadline.
///
/// Test locks are empty `Mutex<()>` guards held for a whole test body.
/// A stalled holder parks every queued test forever (bd rc-88old,
/// rc-y24l camel-ws hang class). Bounding the acquisition turns the
/// wedge into one failing test with a named lock + site.
///
/// The outer fn is synchronous and `#[track_caller]`: the caller site
/// is captured into a local BEFORE the async block is returned — a
/// fully well-defined sync capture, no reliance on `#[track_caller]`
/// behavior across async polling. On timeout the acquisition panics
/// naming the lock, the deadline, and the call site.
#[track_caller]
pub fn acquire_deadline<'a, T: ?Sized>(
    lock: &'a tokio::sync::Mutex<T>,
    what: &'a str,
    deadline: std::time::Duration,
) -> impl std::future::Future<Output = tokio::sync::MutexGuard<'a, T>> + 'a {
    let caller = std::panic::Location::caller();
    async move {
        match tokio::time::timeout(deadline, lock.lock()).await {
            Ok(guard) => guard,
            Err(_) => panic!(
                "test lock {what} not acquired within {deadline:?} \
                 — holder stalled (bd rc-88old), site {caller}"
            ),
        }
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

/// Unit tests for `acquire_deadline`: the uncontended fast path, the
/// stalled-holder panic (lock + deadline named), and the `#[track_caller]`
/// site capture. The stalled holder is by construction: it acquires
/// THROUGH the helper while the lock is uncontended (so its 60 s holder
/// deadline cannot fire), then parks on `pending()` — the wedge the
/// helper exists to bound. No adjudicated waits in this module.
#[cfg(test)]
mod lock_deadline_tests {
    use std::sync::Arc;
    use std::time::Duration;

    use tokio::sync::Mutex;

    use super::acquire_deadline;

    #[tokio::test]
    async fn acquire_deadline_uncontended_returns_guard() {
        let lock = Mutex::new(());
        let _guard = acquire_deadline(&lock, "UNIT_LOCK", Duration::from_secs(60)).await;
        drop(_guard);
    }

    #[tokio::test]
    #[should_panic(expected = "test lock STALLED_UNIT_LOCK not acquired within 100ms")]
    async fn acquire_deadline_stalled_holder_panics_naming_lock() {
        let lock = Arc::new(Mutex::new(()));
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        let holder = {
            let lock = Arc::clone(&lock);
            tokio::spawn(async move {
                // Holder dogfoods the helper: it acquires while the lock is
                // uncontended (the test contends only after the ack), so the
                // 60 s holder deadline cannot fire — the stall comes from
                // pending() below, not from the acquisition.
                let _guard =
                    acquire_deadline(&lock, "unit fixture holder", Duration::from_secs(60)).await;
                let _ = tx.send(());
                std::future::pending::<()>().await;
            })
        };
        // Await the ack FIRST so the holder provably holds the lock before
        // the acquisition attempt (deterministic, race-free, sleep-free).
        rx.await
            .expect("stalled holder acks after acquiring the lock");
        // Always panics at the deadline; `let _` because MutexGuard is
        // #[must_use] and the guard can never be observed here.
        let _ = acquire_deadline(&lock, "STALLED_UNIT_LOCK", Duration::from_millis(100)).await;
        holder.abort(); // unreachable: the line above must panic
    }

    #[test]
    fn acquire_deadline_panic_names_call_site() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .expect("current-thread tokio runtime");
        let lock = Arc::new(Mutex::new(()));
        // The panic unwinds before the async block could return `site`, so
        // the captured location is smuggled out through a Cell (`&'static
        // Location` is Copy, and the set happens before the panicking await).
        let captured_site: std::cell::Cell<Option<&'static std::panic::Location<'static>>> =
            std::cell::Cell::new(None);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            rt.block_on(async {
                let (tx, rx) = tokio::sync::oneshot::channel::<()>();
                let holder = {
                    let lock = Arc::clone(&lock);
                    tokio::spawn(async move {
                        // Same dogfooding as above: uncontended helper
                        // acquisition, then the deliberate pending() stall.
                        let _guard =
                            acquire_deadline(&lock, "unit fixture holder", Duration::from_secs(60))
                                .await;
                        let _ = tx.send(());
                        std::future::pending::<()>().await;
                    })
                };
                rx.await
                    .expect("stalled holder acks after acquiring the lock");
                // Regression-proof site capture: `checked_acquire` is
                // #[track_caller] and caller state propagates through
                // #[track_caller] calls, so `site` and the panic's embedded
                // caller are the SAME Location (the `checked_acquire(...)`
                // statement below). If `acquire_deadline` ever loses
                // #[track_caller], the panic names its own body line and the
                // assertion after the catch_unwind fails.
                #[track_caller]
                fn checked_acquire<'a, T: ?Sized>(
                    lock: &'a tokio::sync::Mutex<T>,
                    what: &'a str,
                    deadline: std::time::Duration,
                ) -> (
                    &'static std::panic::Location<'static>,
                    impl std::future::Future<Output = tokio::sync::MutexGuard<'a, T>> + 'a,
                ) {
                    let site = std::panic::Location::caller();
                    let fut = acquire_deadline(lock, what, deadline);
                    (site, fut)
                }

                let (site, fut) =
                    checked_acquire(&lock, "STALLED_UNIT_LOCK", Duration::from_millis(100));
                captured_site.set(Some(site));
                let _ = std::pin::pin!(fut).await;
                holder.abort(); // unreachable: the line above must panic
            });
        }));
        let err = result.expect_err("stalled acquisition must panic");
        let msg = err
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| err.downcast_ref::<&'static str>().copied())
            .expect("panic payload downcasts to a string");
        assert!(
            msg.contains("STALLED_UNIT_LOCK"),
            "panic must name the lock: {msg}"
        );
        assert!(msg.contains("100ms"), "panic must name the deadline: {msg}");
        let site = captured_site
            .get()
            .expect("site captured before the stalled acquisition");
        assert!(
            msg.contains(&format!("site {site}")),
            "panic must name the exact acquisition call site: {msg}"
        );
    }
}
