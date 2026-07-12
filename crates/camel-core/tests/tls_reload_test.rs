//! End-to-end tests for the TLS cert hot-reload control plane:
//!
//! - `RuntimeBus::execute(ReloadTlsCerts)` intercepts BEFORE journal recovery
//!   and dedup, looks up the handler in `TlsReloadRegistry::global()`, and
//!   returns `TlsCertsReloaded { scheme, host, port }` on success or a
//!   `CamelError::Config` if no handler is registered for the endpoint.
//! - The registry is the dispatch table that wires the control plane to the
//!   component-owned handlers. Components register on `get_or_spawn`, unregister
//!   on release/eviction.
//!
//! These tests verify the *intercept path*. The per-component handler unit
//! tests in `camel-component-grpc/src/tls_reload.rs` and
//! `camel-ws/src/tls_reload.rs` cover the cert-swap mechanics.

use std::sync::Arc;
use std::sync::Mutex as StdMutex;

use async_trait::async_trait;
use camel_api::CamelError;
use camel_api::{RuntimeCommand, RuntimeCommandBus, RuntimeCommandResult};
use camel_component_api::tls_source::{TlsReloadHandler, TlsReloadRegistry};
use camel_core::{
    InMemoryCommandDedup, InMemoryEventPublisher, InMemoryProjectionStore, InMemoryRouteRepository,
    RuntimeBus,
};

/// Recording handler — records every `reload()` call so tests can verify the
/// dispatch path actually invoked the handler.
struct RecordingHandler {
    scheme: String,
    host: String,
    port: u16,
    reload_count: Arc<StdMutex<u32>>,
}

#[async_trait]
impl TlsReloadHandler for RecordingHandler {
    fn matches(&self, scheme: &str, host: &str, port: u16) -> bool {
        self.scheme == scheme && self.host == host && self.port == port
    }
    async fn reload(&self) -> Result<(), CamelError> {
        *self
            .reload_count
            .lock()
            .expect("RecordingHandler counter lock poisoned") += 1;
        Ok(())
    }
}

/// Build a minimal `RuntimeBus` with in-memory adapters. The intercept path
/// runs BEFORE the journal/dedup, so no UoW is required for `ReloadTlsCerts`.
fn build_test_bus() -> RuntimeBus {
    RuntimeBus::new(
        Arc::new(InMemoryRouteRepository::default()),
        Arc::new(InMemoryProjectionStore::default()),
        Arc::new(InMemoryEventPublisher::default()),
        Arc::new(InMemoryCommandDedup::default()),
    )
}

#[tokio::test]
async fn runtime_bus_reload_tls_certs_dispatches_to_registered_handler() {
    let counter = Arc::new(StdMutex::new(0u32));
    let handler = Arc::new(RecordingHandler {
        scheme: "grpcs".into(),
        host: "127.0.0.1".into(),
        port: 54321,
        reload_count: Arc::clone(&counter),
    });
    TlsReloadRegistry::global().register(handler);

    let bus = build_test_bus();
    let result = bus
        .execute(RuntimeCommand::ReloadTlsCerts {
            scheme: "grpcs".into(),
            host: "127.0.0.1".into(),
            port: 54321,
            command_id: "cmd-tls-reload-1".into(),
            causation_id: None,
        })
        .await
        .expect("reload should succeed when handler is registered");

    // Verify intercept returned the success variant (NOT a RouteStateChanged
    // or Duplicate — this is infrastructure, not lifecycle).
    match result {
        RuntimeCommandResult::TlsCertsReloaded { scheme, host, port } => {
            assert_eq!(scheme, "grpcs");
            assert_eq!(host, "127.0.0.1");
            assert_eq!(port, 54321);
        }
        other => panic!("expected TlsCertsReloaded, got {other:?}"),
    }

    // Verify the handler was actually invoked.
    assert_eq!(
        *counter.lock().expect("counter lock poisoned"),
        1,
        "handler.reload() must be called exactly once"
    );

    // Cleanup.
    TlsReloadRegistry::global().unregister("grpcs", "127.0.0.1", 54321);
}

#[tokio::test]
async fn runtime_bus_reload_tls_certs_returns_error_when_no_handler() {
    // Use a port that no test has registered.
    let bus = build_test_bus();
    let err = bus
        .execute(RuntimeCommand::ReloadTlsCerts {
            scheme: "grpcs".into(),
            host: "no-such-host".into(),
            port: 31337,
            command_id: "cmd-tls-reload-missing".into(),
            causation_id: None,
        })
        .await
        .expect_err("reload must fail when no handler is registered");

    // The intercept path returns CamelError::Config with a descriptive message
    // — operators can grep for it.
    let msg = format!("{err}");
    assert!(
        msg.contains("no TLS server found for grpcs://no-such-host:31337"),
        "expected descriptive error, got: {msg}"
    );
}

#[tokio::test]
async fn runtime_bus_reload_tls_certs_bypasses_dedup() {
    // The ReloadTlsCerts intercept runs BEFORE the dedup check, so issuing
    // the same command_id twice MUST call the handler twice. This is the
    // invariant the spec requires: cert reloads are idempotent and not
    // journaled, so dedup is intentionally skipped.
    let counter = Arc::new(StdMutex::new(0u32));
    let handler = Arc::new(RecordingHandler {
        scheme: "https".into(),
        host: "127.0.0.1".into(),
        port: 60401,
        reload_count: Arc::clone(&counter),
    });
    TlsReloadRegistry::global().register(handler);

    let bus = build_test_bus();
    for i in 0..3 {
        let _ = bus
            .execute(RuntimeCommand::ReloadTlsCerts {
                scheme: "https".into(),
                host: "127.0.0.1".into(),
                port: 60401,
                command_id: "same-cmd-id".into(),
                causation_id: None,
            })
            .await
            .unwrap_or_else(|e| panic!("reload #{i} should succeed: {e}"));
    }

    assert_eq!(
        *counter.lock().expect("counter lock poisoned"),
        3,
        "ReloadTlsCerts must bypass dedup and call handler every time"
    );

    TlsReloadRegistry::global().unregister("https", "127.0.0.1", 60401);
}

/// `ReloadTlsCerts` MUST NOT reach `execute_command` (which would route it
/// through journal recovery and reject it as unhandled). This test verifies
/// the intercept happens before that code path by registering a handler and
/// ensuring no error is returned. The other tests cover success/error
/// behavior; this is a structural assertion.
#[tokio::test]
async fn runtime_bus_reload_tls_certs_does_not_require_journal_recovery() {
    // No UoW attached — if the intercept didn't run, the bus would error
    // with "Journal recovery required" or similar before reaching the
    // handler. With the intercept in place, the call succeeds.
    let handler = Arc::new(RecordingHandler {
        scheme: "https".into(),
        host: "127.0.0.1".into(),
        port: 60402,
        reload_count: Arc::new(StdMutex::new(0u32)),
    });
    TlsReloadRegistry::global().register(handler);

    let bus = RuntimeBus::new(
        Arc::new(InMemoryRouteRepository::default()),
        Arc::new(InMemoryProjectionStore::default()),
        Arc::new(InMemoryEventPublisher::default()),
        Arc::new(InMemoryCommandDedup::default()),
    );
    // Intentionally do NOT call .with_uow(...). If the intercept path
    // accidentally fell through to execute_command, the missing UoW
    // would surface as an error.
    let result = bus
        .execute(RuntimeCommand::ReloadTlsCerts {
            scheme: "https".into(),
            host: "127.0.0.1".into(),
            port: 60402,
            command_id: "cmd-no-uow".into(),
            causation_id: None,
        })
        .await;
    assert!(
        result.is_ok(),
        "intercept must bypass UoW/journal: {result:?}"
    );

    TlsReloadRegistry::global().unregister("https", "127.0.0.1", 60402);
}
