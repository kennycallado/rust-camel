//! gRPC transport kernel-auth lifecycle tests (`unify-transport-auth`,
//! Task 2.1).
//!
//! The gRPC interceptor now runs the authentication kernel per request:
//! credential extraction per the compiled plan's `credential_sources`,
//! `kernel_authenticate` against the route's named provider, and the typed
//! carrier installed on each fresh request exchange before the pipeline
//! runs. These tests pin the three task gates plus the Nth-dispatch
//! mandate: after the FIRST request succeeds, a SECOND request on the same
//! route must ALSO carry the carrier on ITS fresh exchange (per-request
//! install, not a first-request-only artifact) — the property Task 2.9's
//! core dispatch check will rely on.

use std::path::PathBuf;
use std::sync::Arc;

use camel_api::security_policy::{
    AccessMode, AuthPrincipal, CredentialSource, PRINCIPAL_SUBJECT_KEY, RouteSecurityPlan,
    TransportId,
};
use camel_api::{Body, Exchange, Message};
use camel_auth::native_auth::{
    NativeCredential, NativeCredentialSecret, NativeCredentialStore, StaticTokenAuthenticator,
};
use camel_auth::{ProviderEntry, ProviderRegistry, RolePolicy, TokenAuthenticator, read_carrier};
use camel_component_api::{
    Consumer, ConsumerContext, ExchangeEnvelope, NoOpComponentContext, RuntimeObservability,
    SecurityContext,
};
use camel_component_grpc::GrpcMode;
use camel_component_grpc::config::GrpcServerConfig;
use camel_component_grpc::consumer::GrpcConsumer;
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use zeroize::Zeroizing;

mod helloworld {
    tonic::include_proto!("helloworld");
}

const PROVIDER_ID: &str = "idp-grpc";
const TOKEN: &str = "test-token-grpc";

fn test_rt() -> Arc<dyn RuntimeObservability> {
    Arc::new(NoOpComponentContext)
}

fn fixture_registry() -> ProviderRegistry {
    let store = NativeCredentialStore::try_new(vec![NativeCredential {
        secret: NativeCredentialSecret::Plaintext {
            value: Zeroizing::new(TOKEN.to_string()),
        },
        principal: camel_api::security_policy::Principal {
            subject: "svc-grpc".to_string(),
            issuer: "test".to_string(),
            audience: vec![],
            scopes: vec![],
            roles: vec!["grpc-role".to_string()],
            claims: serde_json::Value::Null,
        },
    }])
    .expect("credential store");
    let registry = ProviderRegistry::new();
    registry.register(
        PROVIDER_ID,
        ProviderEntry {
            authenticator: Arc::new(StaticTokenAuthenticator::new(store)),
            audience_binding: None,
        },
    );
    registry
}

fn authenticated_plan(sources: Vec<CredentialSource>) -> RouteSecurityPlan {
    RouteSecurityPlan {
        access_mode: AccessMode::Authenticated,
        provider_ref: Some(PROVIDER_ID.to_string()),
        transport: TransportId::Grpc,
        credential_sources: sources,
        audience_binding: None,
    }
}

/// Start a kernel-secured unary gRPC consumer on an ephemeral port.
///
/// The security context carries the compiled plan plus the provider
/// registry, wired through `set_security_context` BEFORE start — the
/// construction-order lifecycle Task 2.1 mandates. Returns the bound port
/// and the receiver for exchanges the route pipeline would process.
async fn start_secured_consumer(
    plan: RouteSecurityPlan,
    providers: Arc<ProviderRegistry>,
) -> (u16, mpsc::Receiver<ExchangeEnvelope>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().expect("addr").port();

    let proto_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/helloworld.proto");
    let mut consumer = GrpcConsumer::new(
        "127.0.0.1".to_string(),
        port,
        "/helloworld.Greeter/SayHello".to_string(),
        proto_path,
        "helloworld.Greeter".to_string(),
        "SayHello".to_string(),
        GrpcMode::Unary,
        test_rt(),
        GrpcServerConfig::default(),
    );

    // Legacy authenticator view of the same fixture provider: the pre-2.9
    // dual path keeps the policy-evaluation leg green while the kernel
    // takes over minting.
    let entry = providers.resolve(PROVIDER_ID).expect("fixture provider");
    let authenticator: Arc<dyn TokenAuthenticator> = Arc::clone(&entry.authenticator);

    let policy = RolePolicy::new(vec!["grpc-role".to_string()], true);
    let sec_ctx = SecurityContext::new(policy, authenticator)
        .with_credential_sources(plan.credential_sources.clone())
        .with_plan(plan)
        .with_providers(providers);
    consumer.set_security_context(sec_ctx);

    let (route_tx, route_rx) = mpsc::channel(16);
    let cancel_token = CancellationToken::new();
    let ctx = ConsumerContext::new(route_tx, cancel_token, "grpc-auth-test-route".to_string());

    tokio::spawn(async move {
        consumer
            .start_with_listener(ctx, listener)
            .await
            .expect("consumer start");
    });
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;

    (port, route_rx)
}

async fn greeter_client(
    port: u16,
) -> helloworld::greeter_client::GreeterClient<tonic::transport::Channel> {
    let channel = tonic::transport::Endpoint::from_shared(format!("http://127.0.0.1:{port}"))
        .expect("endpoint")
        .connect_lazy();
    helloworld::greeter_client::GreeterClient::new(channel)
}

fn ok_reply() -> Exchange {
    Exchange::new(Message::new(Body::Json(
        serde_json::json!({"message": "ok"}),
    )))
}

#[tokio::test]
async fn grpc_denies_without_credentials_under_kernel() {
    let providers = Arc::new(fixture_registry());
    let plan = authenticated_plan(vec![CredentialSource::AuthorizationHeader]);
    let (port, mut route_rx) = start_secured_consumer(plan, providers).await;

    let mut client = greeter_client(port).await;
    let err = client
        .say_hello(helloworld::HelloRequest {
            name: "World".to_string(),
        })
        .await
        .expect_err("request without credentials must be denied");
    assert_eq!(
        err.code(),
        tonic::Code::Unauthenticated,
        "denial must be UNAUTHENTICATED, got: {err}"
    );

    // Route body counter: the denied request must never reach the route.
    assert!(
        route_rx.try_recv().is_err(),
        "denied request must not produce a route exchange"
    );
}

#[tokio::test]
async fn grpc_named_header_credential_authenticates() {
    let providers = Arc::new(fixture_registry());
    let plan = authenticated_plan(vec![CredentialSource::Header {
        name: "x-api-key".to_string(),
    }]);
    let (port, mut route_rx) = start_secured_consumer(plan, providers).await;

    let pipeline = tokio::spawn(async move {
        let envelope = route_rx.recv().await.expect("exchange reaches route");
        let subject = envelope
            .exchange
            .property(PRINCIPAL_SUBJECT_KEY)
            .and_then(|v| v.as_str().map(str::to_string));
        if let Some(tx) = envelope.reply_tx {
            let _ = tx.send(Ok(ok_reply()));
        }
        subject
    });

    let mut client = greeter_client(port).await;
    let mut request = tonic::Request::new(helloworld::HelloRequest {
        name: "World".to_string(),
    });
    request
        .metadata_mut()
        .insert("x-api-key", TOKEN.parse().expect("token metadata value"));
    let response = client
        .say_hello(request)
        .await
        .expect("valid named-header credential must authenticate");
    assert_eq!(response.into_inner().message, "ok");

    let subject = pipeline.await.expect("pipeline join");
    assert_eq!(
        subject.as_deref(),
        Some("svc-grpc"),
        "authenticated principal must reach the route"
    );
}

#[tokio::test]
async fn grpc_carrier_present_on_second_request_fresh_exchange() {
    // Nth-dispatch mandate: the carrier must ride EVERY fresh request
    // exchange. The FIRST request succeeding must not mask a
    // first-request-only install; the SECOND request on the same route
    // must carry the typed carrier on ITS exchange too — the gate Task
    // 2.9's core dispatch check will flip to rely on.
    let providers = Arc::new(fixture_registry());
    let plan = authenticated_plan(vec![CredentialSource::AuthorizationHeader]);
    let (port, mut route_rx) = start_secured_consumer(plan, providers).await;

    let pipeline = tokio::spawn(async move {
        let mut carried = 0usize;
        for _ in 0..2 {
            let envelope = route_rx.recv().await.expect("exchange reaches route");
            if let Some(carrier) = read_carrier(&envelope.exchange) {
                assert_eq!(carrier.provider_id(), PROVIDER_ID);
                assert_eq!(carrier.principal().subject, "svc-grpc");
                carried += 1;
            }
            if let Some(tx) = envelope.reply_tx {
                let _ = tx.send(Ok(ok_reply()));
            }
        }
        carried
    });

    let mut client = greeter_client(port).await;
    for i in 0..2 {
        let mut request = tonic::Request::new(helloworld::HelloRequest {
            name: format!("World-{i}"),
        });
        request.metadata_mut().insert(
            "authorization",
            format!("Bearer {TOKEN}")
                .parse()
                .expect("bearer metadata value"),
        );
        let response = client
            .say_hello(request)
            .await
            .expect("request {i} must succeed under the kernel path");
        assert_eq!(response.into_inner().message, "ok");
    }

    let carried = pipeline.await.expect("pipeline join");
    assert_eq!(
        carried, 2,
        "typed carrier must be present on EVERY fresh exchange, not just the first"
    );
}
