//! Tests for the ADR-0061 per-bind public-exposure gate (Task 1.9) and
//! the plan-only SecurityContext delivery to server-route consumers
//! (Task 1.2).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use camel_api::CamelError;
use camel_api::RouteController;
use camel_api::security_policy::{
    AccessMode, AudienceBinding, AuthContext, AuthorizationDecision, Principal, RouteSecurityPlan,
    SecurityPolicy, SecurityPolicyConfig, TransportId,
};

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

/// Serializes tests that emit at the shared `bind_gate` acknowledged-warn
/// callsite (camel-auth `enforce_bind_exposure_gate`, acked branch).
///
/// Why: tracing-core caches each callsite's interest process-wide. If a
/// non-recorder thread first-registers the warn callsite AFTER the recorder
/// test's `with_default` dispatch is built but BEFORE it emits,
/// `Rebuilder::JustOne` consults the polluter thread's no-op dispatcher and
/// caches `Interest::never()`; the recorder's `warn!` is then filtered
/// before its thread-local subscriber is consulted and the capture comes
/// back empty. Holding this lock for the whole test body keeps the recorder
/// and the only non-recorder emitter (the acknowledged call in
/// `gate_hostname_authority_is_nonloopback`) from overlapping on the
/// callsite. Same pattern as `PEEK_STALE_LOG_LOCK` in camel-processor.
static BIND_GATE_WARN_LOCK: Mutex<()> = Mutex::new(());

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
    let _gate_guard = BIND_GATE_WARN_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
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
    let _gate_guard = BIND_GATE_WARN_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
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

// ── plan-only SecurityContext delivery (Task 1.2) ──

struct AllowPolicy;

#[async_trait::async_trait]
impl SecurityPolicy for AllowPolicy {
    async fn evaluate(
        &self,
        _exchange: &mut camel_api::Exchange,
        _auth: &AuthContext<'_>,
    ) -> Result<AuthorizationDecision, CamelError> {
        Ok(AuthorizationDecision::Granted {
            principal: Principal {
                subject: "tester".into(),
                issuer: "test".into(),
                audience: vec![],
                scopes: vec![],
                roles: vec![],
                claims: serde_json::Value::Null,
            },
        })
    }
}

struct StubAuth;

#[async_trait::async_trait]
impl camel_auth::TokenAuthenticator for StubAuth {
    async fn authenticate_bearer(&self, _token: &str) -> Result<Principal, CamelError> {
        Ok(Principal {
            subject: "tester".into(),
            issuer: "test".into(),
            audience: vec![],
            scopes: vec![],
            roles: vec![],
            claims: serde_json::Value::Null,
        })
    }
}

/// What `set_security_context` delivered, reduced to comparable facts
/// (`SecurityContext` holds trait objects without equality).
#[derive(Debug)]
struct CapturedSecurityContext {
    policy_present: bool,
    public_plan: bool,
    authorized_plan: bool,
    providers_present: bool,
}

fn capture_from(ctx: &camel_component_api::SecurityContext) -> CapturedSecurityContext {
    CapturedSecurityContext {
        policy_present: ctx.policy.is_some(),
        public_plan: matches!(
            ctx.plan.as_ref().map(|p| &p.access_mode),
            Some(AccessMode::Public)
        ),
        authorized_plan: matches!(
            ctx.plan.as_ref().map(|p| &p.access_mode),
            Some(AccessMode::Authorized(_))
        ),
        providers_present: ctx.providers.is_some(),
    }
}

struct ContextCaptureComponent {
    captured: Arc<Mutex<Option<CapturedSecurityContext>>>,
}

#[async_trait::async_trait]
impl camel_component_api::Component for ContextCaptureComponent {
    fn scheme(&self) -> &str {
        "http"
    }

    fn create_endpoint(
        &self,
        _uri: &str,
        _ctx: &dyn camel_component_api::ComponentContext,
    ) -> Result<Box<dyn camel_component_api::Endpoint>, CamelError> {
        Ok(Box::new(ContextCaptureEndpoint {
            captured: Arc::clone(&self.captured),
        }))
    }
}

struct ContextCaptureEndpoint {
    captured: Arc<Mutex<Option<CapturedSecurityContext>>>,
}

impl camel_component_api::Endpoint for ContextCaptureEndpoint {
    fn uri(&self) -> &str {
        "http"
    }

    fn create_consumer(
        &self,
        _rt: Arc<dyn camel_component_api::RuntimeObservability>,
    ) -> Result<Box<dyn camel_component_api::Consumer>, CamelError> {
        Ok(Box::new(ContextCaptureConsumer {
            captured: Arc::clone(&self.captured),
        }))
    }

    fn create_producer(
        &self,
        _rt: Arc<dyn camel_component_api::RuntimeObservability>,
        _ctx: &camel_component_api::ProducerContext,
    ) -> Result<camel_api::BoxProcessor, CamelError> {
        Ok(camel_api::BoxProcessor::new(camel_api::IdentityProcessor))
    }
}

struct ContextCaptureConsumer {
    captured: Arc<Mutex<Option<CapturedSecurityContext>>>,
}

#[async_trait::async_trait]
impl camel_component_api::Consumer for ContextCaptureConsumer {
    async fn start(&mut self, ctx: camel_component_api::ConsumerContext) -> Result<(), CamelError> {
        ctx.mark_ready();
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), CamelError> {
        Ok(())
    }

    fn startup_mode(&self) -> camel_component_api::ConsumerStartupMode {
        camel_component_api::ConsumerStartupMode::Explicit
    }

    fn set_security_context(&mut self, ctx: camel_component_api::SecurityContext) {
        *self.captured.lock().expect("capture slot") = Some(capture_from(&ctx));
    }
}

async fn stage_and_start(
    uri: &str,
    route_id: &str,
    security: impl FnOnce(
        crate::lifecycle::application::route_definition::RouteDefinition,
    ) -> crate::lifecycle::application::route_definition::RouteDefinition,
) -> CapturedSecurityContext {
    use crate::lifecycle::adapters::route_controller::DefaultRouteController;
    use crate::shared::components::domain::Registry;

    let captured: Arc<Mutex<Option<CapturedSecurityContext>>> = Arc::new(Mutex::new(None));
    let component_registry = Arc::new(std::sync::Mutex::new(Registry::new()));
    component_registry
        .lock()
        .expect("registry lock")
        .register(Arc::new(ContextCaptureComponent {
            captured: Arc::clone(&captured),
        }));

    let mut controller = DefaultRouteController::new(
        component_registry,
        Arc::new(camel_api::NoopPlatformService::default()),
    );

    let def = security(
        crate::lifecycle::application::route_definition::RouteDefinition::new(uri, vec![])
            .with_route_id(route_id),
    );
    controller
        .add_route(def)
        .await
        .unwrap_or_else(|e| panic!("staging {route_id} must succeed: {e}"));
    controller
        .start_route(route_id)
        .await
        .unwrap_or_else(|e| panic!("start {route_id} must succeed: {e}"));

    let snapshot = captured
        .lock()
        .expect("capture slot")
        .take()
        .unwrap_or_else(|| panic!("set_security_context must have been invoked for {route_id}"));

    controller
        .stop_route(route_id)
        .await
        .unwrap_or_else(|e| panic!("stop {route_id} must succeed: {e}"));
    snapshot
}

#[tokio::test]
async fn undeclared_server_route_receives_plan_only_context() {
    let captured =
        stage_and_start("http://127.0.0.1:18061/api", "undeclared-route", |def| def).await;
    assert!(
        !captured.policy_present,
        "plan-only context carries no policy: {captured:?}"
    );
    assert!(
        captured.public_plan,
        "plan must be the compiled Public plan: {captured:?}"
    );
    assert!(
        !captured.providers_present,
        "no providers without a registry: {captured:?}"
    );
}

#[tokio::test]
async fn declared_route_context_unchanged() {
    let provider_registry = Arc::new(camel_auth::ProviderRegistry::new());
    provider_registry.register(
        "idp-a",
        camel_auth::ProviderEntry {
            authenticator: Arc::new(StubAuth),
            audience_binding: None,
        },
    );
    let captured = stage_and_start("http://127.0.0.1:18062/api", "declared-route", |def| {
        def.with_security_policy(SecurityPolicyConfig::new(AllowPolicy))
            .with_security_authenticator(Arc::new(StubAuth))
            .with_provider_registry(provider_registry)
    })
    .await;
    assert!(
        captured.policy_present,
        "declared route keeps its policy: {captured:?}"
    );
    assert!(
        captured.authorized_plan,
        "plan must carry the Authorized classification: {captured:?}"
    );
    assert!(
        captured.providers_present,
        "declared route keeps its providers: {captured:?}"
    );
}
