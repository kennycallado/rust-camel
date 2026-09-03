//! Endpoint wiring + passive health-check tests (extracted from `lib.rs`
//! per the 1k-line rule; keeps `super::` access to the crate root items).

use std::sync::{Arc, Mutex};
use std::time::Duration;

use camel_component_api::test_support::NoopRuntimeObservability;
use camel_component_api::{Body as CamelBody, Component, ConsumerContext, NoOpComponentContext};
use futures::{SinkExt, StreamExt};
use tokio::sync::{mpsc, watch};
use tokio_tungstenite::tungstenite::protocol::Message as ClientMessage;
use tokio_util::sync::CancellationToken;

use super::{ClientConnState, WsComponent};
use crate::health::ConnectionStateCheck;

/// `ComponentContext` test double recording the names of health checks
/// registered through `register_current_route_health_check`. The trait
/// has no default-free-method escapes here: every required method gets
/// an explicit minimal impl.
struct RecordingComponentContext {
    registered: Mutex<Vec<String>>,
}

impl camel_component_api::ComponentContext for RecordingComponentContext {
    fn resolve_component(&self, _scheme: &str) -> Option<Arc<dyn camel_component_api::Component>> {
        None
    }

    fn resolve_language(&self, _name: &str) -> Option<Arc<dyn camel_language_api::Language>> {
        None
    }

    fn metrics(&self) -> Arc<dyn camel_api::MetricsCollector> {
        Arc::new(camel_api::NoOpMetrics)
    }

    fn platform_service(&self) -> Arc<dyn camel_api::PlatformService> {
        Arc::new(camel_api::NoopPlatformService::default())
    }

    fn register_route_health_check(
        &self,
        _route_id: &str,
        _check: Arc<dyn camel_api::AsyncHealthCheck>,
    ) {
    }

    fn unregister_route_health_check(&self, _route_id: &str) {}

    fn register_current_route_health_check(&self, check: Arc<dyn camel_api::AsyncHealthCheck>) {
        self.registered
            .lock()
            .expect("recording lock")
            .push(check.name().to_string());
    }
}

#[test]
fn wiring_registers_correct_check() {
    // Server mode (default): the TCP-listener probe under the "ws" name.
    let server_ctx = RecordingComponentContext {
        registered: Mutex::new(Vec::new()),
    };
    let _endpoint = WsComponent::new()
        .create_endpoint("ws://localhost:0/echo", &server_ctx)
        .expect("server-mode endpoint must build");
    assert_eq!(
        *server_ctx.registered.lock().expect("recording lock"),
        vec!["ws".to_string()],
        "server mode must register exactly the ws listener check"
    );

    // Client mode: the passive connection-state check under "ws-client".
    let client_ctx = RecordingComponentContext {
        registered: Mutex::new(Vec::new()),
    };
    let _endpoint = WsComponent::new()
        .create_endpoint("ws://localhost:0/echo?consumeAsClient=true", &client_ctx)
        .expect("client-mode endpoint must build");
    assert_eq!(
        *client_ctx.registered.lock().expect("recording lock"),
        vec!["ws-client".to_string()],
        "client mode must register exactly the ws-client state check"
    );
}

#[tokio::test]
async fn connection_state_check_passive_states() {
    use camel_api::AsyncHealthCheck as _;

    let (tx, rx) = watch::channel(ClientConnState::Connecting);
    let check = ConnectionStateCheck::new(rx);
    assert_eq!(check.name(), "ws-client");

    let connecting = check.check().await;
    assert_eq!(connecting.status, camel_api::HealthStatus::Unhealthy);
    assert!(
        connecting
            .message
            .as_deref()
            .is_some_and(|m| m.contains("Connecting")),
        "unhealthy message must name the state: {:?}",
        connecting.message
    );

    tx.send_replace(ClientConnState::Connected);
    let connected = check.check().await;
    assert_eq!(connected.status, camel_api::HealthStatus::Healthy);
    assert!(connected.message.is_none());
    // Zero TCP connections by construction: the type holds only a watch
    // receiver and performs no I/O in check().
}

#[tokio::test]
async fn client_mode_wires_client_consumer() {
    // Reachable push server: handshake, push one frame, hold open.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut ws = match tokio_tungstenite::accept_async(stream).await {
                    Ok(ws) => ws,
                    Err(_) => return,
                };
                if ws.send(ClientMessage::Text("wired".into())).await.is_err() {
                    return;
                }
                while let Some(Ok(msg)) = ws.next().await {
                    if msg.is_close() {
                        break;
                    }
                }
            });
        }
    });

    let uri = format!("ws://127.0.0.1:{}/echo?consumeAsClient=true", addr.port());
    let endpoint = WsComponent::new()
        .create_endpoint(&uri, &NoOpComponentContext)
        .expect("client-mode endpoint must build");
    let mut consumer = endpoint
        .create_consumer(Arc::new(NoopRuntimeObservability))
        .expect("consumer must build");

    let (route_tx, mut route_rx) = mpsc::channel(16);
    let (signal, _receiver) = camel_component_api::StartupSignal::pair();
    let ctx = ConsumerContext::new(route_tx, CancellationToken::new(), "r1".to_string())
        .with_startup(signal);
    consumer.start(ctx).await.expect("client consumer start");

    let env = tokio::time::timeout(Duration::from_secs(2), route_rx.recv())
        .await
        .expect("frame must arrive within 2s")
        .expect("route channel open");
    match &env.exchange.input.body {
        CamelBody::Text(s) => assert_eq!(s, "wired", "the pushed frame must flow"),
        other => panic!("expected Text body, got {other:?}"),
    }

    consumer.stop().await.expect("stop must succeed");
    server.abort();
}
