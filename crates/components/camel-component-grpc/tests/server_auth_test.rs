//! gRPC transport kernel-auth lifecycle tests (`unify-transport-auth`,
//! Task 2.1; legacy arm deleted in `finish-auth-flip` Task 1.2).
//!
//! The gRPC interceptor runs the authentication kernel per request:
//! credential extraction per the compiled plan's `credential_sources`,
//! `kernel_authenticate` against the route's named provider, and the typed
//! carrier installed on each fresh request exchange before the pipeline
//! runs. There is no legacy arm: a plan-less context is Public
//! pass-through (no extraction), and policy enforcement lives wholly in
//! the pipeline layer plus the strict dispatch check. These tests pin
//! the task gates plus the Nth-dispatch mandate: after the FIRST request
//! succeeds, a SECOND request on the same route must ALSO carry the
//! carrier on ITS fresh exchange (per-request install, not a
//! first-request-only artifact) — the property Task 2.9's core dispatch
//! check relies on.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use camel_api::security_policy::{
    AccessMode, AuthPrincipal, CredentialSource, PRINCIPAL_SUBJECT_KEY, Principal,
    RouteSecurityPlan, TransportId,
};
use camel_api::{Body, CamelError, Exchange, Message};
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
use tokio_stream::StreamExt;
use tokio_util::sync::CancellationToken;
use zeroize::Zeroizing;

mod helloworld {
    tonic::include_proto!("helloworld");
}

mod streaming {
    tonic::include_proto!("streaming");
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

/// Start a unary gRPC consumer on an ephemeral port with the given
/// security context (or none — a contextless consumer is Public
/// pass-through). The context is wired through `set_security_context`
/// BEFORE start — the construction-order lifecycle Task 2.1 mandates.
/// Returns the bound port and the receiver for exchanges the route
/// pipeline would process.
async fn start_consumer(
    sec_ctx: Option<SecurityContext>,
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

    if let Some(sec_ctx) = sec_ctx {
        consumer.set_security_context(sec_ctx);
    }

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
    let policy = RolePolicy::new(vec!["grpc-role".to_string()], true);
    let sec_ctx = SecurityContext::new(policy)
        .with_credential_sources(plan.credential_sources.clone())
        .with_plan(plan)
        .with_providers(providers);
    start_consumer(Some(sec_ctx)).await
}

async fn greeter_client(
    port: u16,
) -> helloworld::greeter_client::GreeterClient<tonic::transport::Channel> {
    let channel = tonic::transport::Endpoint::from_shared(format!("http://127.0.0.1:{port}"))
        .expect("endpoint")
        .connect_lazy();
    helloworld::greeter_client::GreeterClient::new(channel)
}

/// Start a kernel-secured streaming gRPC consumer on an ephemeral port
/// for the given `streaming.StreamService` method. Same
/// construction-order lifecycle as `start_consumer`; the security
/// context is wired through `set_security_context` BEFORE start.
async fn start_streaming_consumer(
    sec_ctx: Option<SecurityContext>,
    method: &str,
    mode: GrpcMode,
) -> (u16, mpsc::Receiver<ExchangeEnvelope>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().expect("addr").port();

    let proto_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/streaming.proto");
    let mut consumer = GrpcConsumer::new(
        "127.0.0.1".to_string(),
        port,
        format!("/streaming.StreamService/{method}"),
        proto_path,
        "streaming.StreamService".to_string(),
        method.to_string(),
        mode,
        test_rt(),
        GrpcServerConfig::default(),
    );

    if let Some(sec_ctx) = sec_ctx {
        consumer.set_security_context(sec_ctx);
    }

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

async fn stream_service_client(
    port: u16,
) -> streaming::stream_service_client::StreamServiceClient<tonic::transport::Channel> {
    let channel = tonic::transport::Endpoint::from_shared(format!("http://127.0.0.1:{port}"))
        .expect("endpoint")
        .connect_lazy();
    streaming::stream_service_client::StreamServiceClient::new(channel)
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

/// `TokenAuthenticator` that counts every authenticate call: a plan-less
/// context must never reach ANY provider.
struct CountingProvider {
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl TokenAuthenticator for CountingProvider {
    async fn authenticate_bearer(&self, _token: &str) -> Result<Principal, CamelError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(Principal {
            subject: "svc-grpc".to_string(),
            issuer: "test".to_string(),
            audience: vec![],
            scopes: vec![],
            roles: vec!["grpc-role".to_string()],
            claims: serde_json::Value::Null,
        })
    }
}

#[tokio::test]
async fn grpc_public_no_extraction_counted_provider() {
    // Registry WITHOUT a plan: post-1.2 a context without a kernel plan
    // is Public pass-through — no extraction, so the provider must never
    // be consulted even though the client presents a credential the
    // provider would accept (the deleted legacy arm would have called
    // it).
    let calls = Arc::new(AtomicUsize::new(0));
    let registry = ProviderRegistry::new();
    registry.register(
        PROVIDER_ID,
        ProviderEntry {
            authenticator: Arc::new(CountingProvider {
                calls: Arc::clone(&calls),
            }),
            audience_binding: None,
        },
    );
    let sec_ctx = SecurityContext::from_arc(Arc::new(RolePolicy::new(vec![], true)))
        .with_providers(Arc::new(registry));
    let (port, mut route_rx) = start_consumer(Some(sec_ctx)).await;

    let pipeline = tokio::spawn(async move {
        let envelope = route_rx
            .recv()
            .await
            .expect("public pass-through reaches route");
        if let Some(tx) = envelope.reply_tx {
            let _ = tx.send(Ok(ok_reply()));
        }
    });

    let mut client = greeter_client(port).await;
    let mut request = tonic::Request::new(helloworld::HelloRequest {
        name: "World".to_string(),
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
        .expect("plan-less route is Public pass-through");
    assert_eq!(response.into_inner().message, "ok");

    pipeline.await.expect("pipeline join");
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "plan-less route must never consult a provider"
    );
}

#[tokio::test]
async fn grpc_kernel_auth_policy_denied_regression() {
    // The transport no longer evaluates the route policy (the per-arm
    // scratch evaluation was deleted): a kernel-authenticated request
    // (valid token) must REACH the route pipeline even when the route's
    // policy would deny, and a pipeline policy denial must still surface
    // as PERMISSION_DENIED at the transport idiom.
    let providers = Arc::new(fixture_registry());
    let plan = authenticated_plan(vec![CredentialSource::AuthorizationHeader]);
    // Denying route policy: requires a role the fixture principal
    // ("grpc-role") lacks. Inert at the transport post-1.2 — it stands
    // as the route's declaration for the pipeline layer.
    let policy = RolePolicy::new(vec!["other-role".to_string()], true);
    let sec_ctx = SecurityContext::new(policy)
        .with_credential_sources(plan.credential_sources.clone())
        .with_plan(plan)
        .with_providers(providers);
    let (port, mut route_rx) = start_consumer(Some(sec_ctx)).await;

    let pipeline = tokio::spawn(async move {
        let envelope = route_rx
            .recv()
            .await
            .expect("kernel-authenticated request must reach the route pipeline");
        // Pipeline-side enforcement stand-in (SecurityPolicyService
        // denies with Unauthorized): the pipeline policy denial idiom.
        if let Some(tx) = envelope.reply_tx {
            let _ = tx.send(Err(CamelError::Unauthorized("missing role".to_string())));
        }
    });

    let mut client = greeter_client(port).await;
    let mut request = tonic::Request::new(helloworld::HelloRequest {
        name: "World".to_string(),
    });
    request.metadata_mut().insert(
        "authorization",
        format!("Bearer {TOKEN}")
            .parse()
            .expect("bearer metadata value"),
    );
    let err = client
        .say_hello(request)
        .await
        .expect_err("pipeline policy denial must deny the call");
    assert_eq!(
        err.code(),
        tonic::Code::PermissionDenied,
        "pipeline policy denial must surface as PERMISSION_DENIED, got: {err}"
    );

    pipeline.await.expect("pipeline join");
}

#[tokio::test]
async fn grpc_server_streaming_pipeline_denial_regression() {
    // Streaming denial regression (`finish-auth-flip` review F1): the
    // server-streaming envelope used to carry `reply_tx: None`, so a
    // pipeline policy denial died inside the route controller and the
    // client stream ended as a silent, empty success. The envelope now
    // carries a pipeline reply channel: a kernel-authenticated request
    // (valid token) still REACHES the route pipeline even when the
    // route's policy would deny, and the pipeline denial surfaces on
    // the stream as PERMISSION_DENIED — the same idiom the deleted
    // transport-side scratch evaluation emitted.
    let providers = Arc::new(fixture_registry());
    let plan = authenticated_plan(vec![CredentialSource::AuthorizationHeader]);
    // Denying route policy: requires a role the fixture principal
    // ("grpc-role") lacks. Inert at the transport — it stands as the
    // route's declaration for the pipeline layer.
    let policy = RolePolicy::new(vec!["other-role".to_string()], true);
    let sec_ctx = SecurityContext::new(policy)
        .with_credential_sources(plan.credential_sources.clone())
        .with_plan(plan)
        .with_providers(providers);
    let (port, mut route_rx) =
        start_streaming_consumer(Some(sec_ctx), "ServerList", GrpcMode::ServerStreaming).await;

    let pipeline = tokio::spawn(async move {
        let envelope = route_rx
            .recv()
            .await
            .expect("kernel-authenticated request must reach the route pipeline");
        // Pipeline-side enforcement stand-in (SecurityPolicyService
        // denies with Unauthorized): the pipeline policy denial idiom.
        if let Some(tx) = envelope.reply_tx {
            let _ = tx.send(Err(CamelError::Unauthorized("missing role".to_string())));
        }
    });

    let mut client = stream_service_client(port).await;
    let mut request = tonic::Request::new(streaming::ListRequest { count: 3 });
    request.metadata_mut().insert(
        "authorization",
        format!("Bearer {TOKEN}")
            .parse()
            .expect("bearer metadata value"),
    );
    let mut stream = client
        .server_list(request)
        .await
        .expect("stream must open before the denial verdict")
        .into_inner();
    let err = stream
        .next()
        .await
        .expect("denial must produce a stream frame")
        .expect_err("pipeline policy denial must surface on the stream");
    assert_eq!(
        err.code(),
        tonic::Code::PermissionDenied,
        "pipeline policy denial must surface as PERMISSION_DENIED, got: {err}"
    );

    pipeline.await.expect("pipeline join");
}

#[tokio::test]
async fn grpc_client_streaming_pipeline_denial_regression() {
    // Client-streaming denial regression (bd rc-938k, sibling of the
    // server-streaming case above): intermediate AND completion
    // envelopes carry pipeline reply channels, so a kernel-authenticated
    // request (valid token) still REACHES the route pipeline even when
    // the route's policy would deny. Only the completion exchange's
    // reply becomes the RPC verdict, and this arm's error mapping also
    // turns a dropped reply sender into INTERNAL ("pipeline reply
    // dropped") — asserting PERMISSION_DENIED pins the
    // `pipeline_error_to_status` denial mapping specifically.
    let providers = Arc::new(fixture_registry());
    let plan = authenticated_plan(vec![CredentialSource::AuthorizationHeader]);
    // Denying route policy: requires a role the fixture principal
    // ("grpc-role") lacks. Inert at the transport — it stands as the
    // route's declaration for the pipeline layer.
    let policy = RolePolicy::new(vec!["other-role".to_string()], true);
    let sec_ctx = SecurityContext::new(policy)
        .with_credential_sources(plan.credential_sources.clone())
        .with_plan(plan)
        .with_providers(providers);
    let (port, mut route_rx) =
        start_streaming_consumer(Some(sec_ctx), "ClientSum", GrpcMode::ClientStreaming).await;

    let pipeline = tokio::spawn(async move {
        // Reply the denial idiom to every envelope: intermediate replies
        // are discarded by design; the completion exchange's reply is
        // the RPC verdict. Stop at the completion marker so the stand-in
        // terminates while the consumer keeps serving.
        loop {
            let envelope = route_rx
                .recv()
                .await
                .expect("kernel-authenticated request must reach the route pipeline");
            // Pipeline-side enforcement stand-in (SecurityPolicyService
            // denies with Unauthorized): the pipeline policy denial idiom.
            if let Some(tx) = envelope.reply_tx {
                let _ = tx.send(Err(CamelError::Unauthorized("missing role".to_string())));
            }
            let complete = matches!(
                envelope
                    .exchange
                    .input
                    .header("CamelGrpcClientStreamComplete"),
                Some(serde_json::Value::Bool(true))
            );
            if complete {
                break;
            }
        }
    });

    let mut client = stream_service_client(port).await;
    let mut request = tonic::Request::new(tokio_stream::iter(vec![
        streaming::NumberRequest { value: 1 },
        streaming::NumberRequest { value: 2 },
    ]));
    request.metadata_mut().insert(
        "authorization",
        format!("Bearer {TOKEN}")
            .parse()
            .expect("bearer metadata value"),
    );
    let err = client
        .client_sum(request)
        .await
        .expect_err("pipeline policy denial must fail the RPC");
    assert_eq!(
        err.code(),
        tonic::Code::PermissionDenied,
        "pipeline policy denial must surface as PERMISSION_DENIED, got: {err}"
    );

    pipeline.await.expect("pipeline join");
}

#[tokio::test]
async fn grpc_bidi_pipeline_denial_regression() {
    // Bidi denial regression (bd rc-938k): every bidi envelope carries a
    // pipeline reply channel, and a per-message watcher renders an Err
    // reply through `pipeline_error_to_status` onto the response
    // stream, so a kernel-authenticated request (valid token) still
    // REACHES the route pipeline even when the route's policy would
    // deny, and the denial surfaces on the stream as
    // PERMISSION_DENIED. A dropped reply sender emits NOTHING (the
    // watcher stays silent), so the asserted frame pins the denial
    // verdict path specifically.
    let providers = Arc::new(fixture_registry());
    let plan = authenticated_plan(vec![CredentialSource::AuthorizationHeader]);
    // Denying route policy: requires a role the fixture principal
    // ("grpc-role") lacks. Inert at the transport — it stands as the
    // route's declaration for the pipeline layer.
    let policy = RolePolicy::new(vec!["other-role".to_string()], true);
    let sec_ctx = SecurityContext::new(policy)
        .with_credential_sources(plan.credential_sources.clone())
        .with_plan(plan)
        .with_providers(providers);
    let (port, mut route_rx) =
        start_streaming_consumer(Some(sec_ctx), "BidiEcho", GrpcMode::Bidi).await;

    let pipeline = tokio::spawn(async move {
        let envelope = route_rx
            .recv()
            .await
            .expect("kernel-authenticated request must reach the route pipeline");
        // Pipeline-side enforcement stand-in (SecurityPolicyService
        // denies with Unauthorized): the pipeline policy denial idiom.
        if let Some(tx) = envelope.reply_tx {
            let _ = tx.send(Err(CamelError::Unauthorized("missing role".to_string())));
        }
    });

    // Hold the request stream open past the verdict: the forward loop
    // treats client-stream end as completion, and racing completion
    // against the denial frame would make the first frame ambiguous.
    let request_stream = tokio_stream::iter(vec![streaming::EchoRequest {
        message: "hello".to_string(),
    }])
    .chain(tokio_stream::pending());
    let mut request = tonic::Request::new(request_stream);
    request.metadata_mut().insert(
        "authorization",
        format!("Bearer {TOKEN}")
            .parse()
            .expect("bearer metadata value"),
    );
    let mut client = stream_service_client(port).await;
    let mut stream = client
        .bidi_echo(request)
        .await
        .expect("stream must open before the denial verdict")
        .into_inner();
    let err = stream
        .next()
        .await
        .expect("denial must produce a stream frame")
        .expect_err("pipeline policy denial must surface on the stream");
    assert_eq!(
        err.code(),
        tonic::Code::PermissionDenied,
        "pipeline policy denial must surface as PERMISSION_DENIED, got: {err}"
    );

    pipeline.await.expect("pipeline join");
}

/// Recursively collect the `.rs` files under `dir`.
fn collect_rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("read source dir") {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            collect_rs_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

/// Task 1.2 guard (blessed by tasks.md, restored per review F2): the
/// legacy component-owned auth arm stays deleted. Scans every production
/// source file under `src/` for the legacy markers — test-file mentions
/// are out of scope by construction (the scan never leaves `src/`), and
/// the needles are assembled at runtime so this guard carries no
/// compile-time matchable literal of its own.
#[test]
fn grpc_legacy_arm_deleted_source_scan() {
    let needles = [
        // Deleted gRPC-side principal type.
        ["Grpc", "Principal"].concat(),
        // Deleted transport-side credential extraction helper.
        ["extract", "_principal"].concat(),
        // Deleted legacy authenticator lookup.
        ["legacy", "_authenticator"].concat(),
    ];
    let src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    collect_rs_files(&src, &mut files);
    assert!(
        !files.is_empty(),
        "source scan must find production sources"
    );
    for file in files {
        let content = std::fs::read_to_string(&file).expect("read source file");
        for needle in &needles {
            assert!(
                !content.contains(needle.as_str()),
                "legacy marker `{needle}` must stay deleted: {}",
                file.display()
            );
        }
    }
}
