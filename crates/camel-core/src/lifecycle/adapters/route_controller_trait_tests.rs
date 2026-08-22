//! Tests for the ADR-0061 per-bind public-exposure gate (Task 1.9).

use std::collections::HashMap;
use std::sync::Arc;

use camel_api::CamelError;
use camel_api::security_policy::{AccessMode, AudienceBinding, RouteSecurityPlan, TransportId};

use crate::lifecycle::adapters::route_controller_trait::{
    BindExposureAcks, enforce_bind_exposure_gate,
};

fn public_plan() -> RouteSecurityPlan {
    RouteSecurityPlan {
        access_mode: AccessMode::Public,
        provider_ref: None,
        transport: TransportId::Http,
        credential_sources: vec![],
        audience_binding: None,
    }
}

fn authenticated_plan(provider: &str) -> RouteSecurityPlan {
    RouteSecurityPlan {
        access_mode: AccessMode::Authenticated,
        provider_ref: Some(provider.to_string()),
        transport: TransportId::Http,
        credential_sources: vec![],
        audience_binding: Some(AudienceBinding {
            issuers: vec![],
            audiences: vec![],
        }),
    }
}

/// Runs `f` under a thread-local `fmt` subscriber capturing output into a
/// buffer; returns the captured text.
fn capture_logs(f: impl FnOnce()) -> String {
    struct CaptureWriter {
        buf: Arc<std::sync::Mutex<Vec<u8>>>,
    }
    impl std::io::Write for CaptureWriter {
        fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
            self.buf.lock().unwrap().extend_from_slice(data);
            Ok(data.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for CaptureWriter {
        type Writer = CaptureWriter;
        fn make_writer(&'a self) -> Self::Writer {
            CaptureWriter {
                buf: Arc::clone(&self.buf),
            }
        }
    }
    let buf = Arc::new(std::sync::Mutex::new(Vec::new()));
    let subscriber = tracing_subscriber::fmt::Subscriber::builder()
        .with_writer(CaptureWriter {
            buf: Arc::clone(&buf),
        })
        .with_ansi(false)
        .finish();
    tracing::subscriber::with_default(subscriber, f);
    String::from_utf8(buf.lock().unwrap().clone()).expect("captured output must be UTF-8")
}

#[test]
fn gate_refuses_nonloopback_public_without_ack() {
    let err = enforce_bind_exposure_gate("0.0.0.0:8080", false, &[("r1", &public_plan())], false)
        .unwrap_err();
    let CamelError::RouteError(msg) = &err else {
        panic!("expected RouteError, got {err:?}");
    };
    assert!(msg.contains("0.0.0.0:8080"), "must name the bind: {msg}");
    assert!(msg.contains("r1"), "must name the public route: {msg}");
}

#[test]
fn gate_names_all_public_routes_on_the_bind() {
    let plans = [("r1", &public_plan()), ("r2", &public_plan())];
    let err = enforce_bind_exposure_gate("10.0.0.1:9000", false, &plans, false).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("r1") && msg.contains("r2"),
        "must name both: {msg}"
    );
}

#[test]
fn gate_acknowledged_warns_and_passes() {
    let captured = capture_logs(|| {
        enforce_bind_exposure_gate("0.0.0.0:8080", false, &[("r1", &public_plan())], true)
            .expect("acknowledged bind must pass");
    });
    assert!(
        captured.contains("0.0.0.0:8080"),
        "warn must name the bind: {captured}"
    );
    assert!(
        captured.contains("public_routes=1"),
        "warn must state the public-route count: {captured}"
    );
    assert!(
        captured.to_lowercase().contains("warn"),
        "must be a warning, not silent: {captured}"
    );
}

#[test]
fn gate_loopback_public_needs_no_ack() {
    for (key, loopback) in [
        ("127.0.0.1:0", true),
        ("[::1]:0", true),
        ("localhost:8080", true),
    ] {
        let captured = capture_logs(|| {
            enforce_bind_exposure_gate(key, loopback, &[("r1", &public_plan())], false)
                .unwrap_or_else(|e| panic!("loopback {key} must pass: {e}"));
        });
        assert!(captured.is_empty(), "loopback must not warn: {captured}");
    }
}

#[test]
fn gate_hostname_authority_is_nonloopback() {
    // Hostnames other than localhost fail closed to the gate check.
    let err = enforce_bind_exposure_gate(
        "myhost.example:8080",
        false,
        &[("r1", &public_plan())],
        false,
    )
    .unwrap_err();
    assert!(err.to_string().contains("myhost.example:8080"));
    // And the ack key is the authority string as written.
    enforce_bind_exposure_gate(
        "myhost.example:8080",
        false,
        &[("r1", &public_plan())],
        true,
    )
    .expect("hostname ack by authority string passes");
}

#[test]
fn gate_passes_when_no_public_routes() {
    enforce_bind_exposure_gate(
        "0.0.0.0:8080",
        false,
        &[("r1", &authenticated_plan("idp-a"))],
        false,
    )
    .expect("non-public routes never trip the gate");
}

#[test]
fn bind_acks_default_is_unacknowledged() {
    let acks = BindExposureAcks::new(HashMap::new());
    assert!(!acks.acknowledged("0.0.0.0:8080"));
    let acks = BindExposureAcks::new(HashMap::from([("0.0.0.0:8080".to_string(), true)]));
    assert!(acks.acknowledged("0.0.0.0:8080"));
    assert!(!acks.acknowledged("10.0.0.1:8080"));
}
