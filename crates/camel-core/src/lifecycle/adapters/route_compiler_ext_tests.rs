use super::*;
use crate::lifecycle::application::route_definition::RouteDefinition;
use camel_api::RouteController;
use camel_api::security_policy::{
    AccessMode, AudienceBinding, AuthContext, AuthorizationDecision, CredentialSource, Principal,
    SecurityPolicy, SecurityPolicyConfig,
};
use camel_api::{BoxProcessor, CamelError, Exchange, IdentityProcessor};
use camel_auth::{ProviderEntry, TokenAuthenticator};
use camel_component_api::{
    Component, ComponentContext, ConcurrencyModel, Consumer, ConsumerContext, ConsumerStartupMode,
    Endpoint, ProducerContext, RuntimeObservability, SecurityContext,
};
use std::sync::{Arc, Mutex};

// ── fixtures ──

struct AllowPolicy;

#[async_trait::async_trait]
impl SecurityPolicy for AllowPolicy {
    async fn evaluate(
        &self,
        _exchange: &mut Exchange,
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
impl TokenAuthenticator for StubAuth {
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

fn allow_policy_config() -> SecurityPolicyConfig {
    SecurityPolicyConfig::new(AllowPolicy)
}

fn providers(count: usize, binding: Option<AudienceBinding>) -> ProviderRegistry {
    let registry = ProviderRegistry::new();
    for name in ["idp-a", "idp-b"].into_iter().take(count) {
        registry.register(
            name,
            ProviderEntry {
                authenticator: Arc::new(StubAuth),
                audience_binding: binding.clone(),
            },
        );
    }
    registry
}

fn http_def(id: &str) -> RouteDefinition {
    RouteDefinition::new("http://127.0.0.1:8080/api", vec![]).with_route_id(id)
}

// ── compilation tests ──

#[test]
fn compilation_public_default_no_declaration() {
    let def = http_def("pub-route");
    let plan = compile_route_security_plan(&def, &providers(1, None))
        .expect("compilation must succeed")
        .expect("http consumer route must get a plan");
    assert!(matches!(plan.access_mode, AccessMode::Public));
    assert_eq!(plan.provider_ref, None);
    assert_eq!(plan.transport, TransportId::Http);
    assert!(plan.audience_binding.is_none());
}

#[test]
fn compilation_roles_named_provider_resolves_with_audience() {
    let binding = AudienceBinding {
        issuers: vec!["https://a".into()],
        audiences: vec!["api".into()],
    };
    let def = http_def("roles-named")
        .with_security_policy(allow_policy_config())
        .with_security_provider("idp-a");
    let plan = compile_route_security_plan(&def, &providers(1, Some(binding.clone())))
        .expect("compilation must succeed")
        .expect("plan expected");
    assert!(matches!(plan.access_mode, AccessMode::Authorized(_)));
    assert_eq!(plan.provider_ref.as_deref(), Some("idp-a"));
    assert_eq!(plan.audience_binding, Some(binding));
}

#[test]
fn compilation_route_audiences_override_provider() {
    let binding = AudienceBinding {
        issuers: vec!["https://a".into()],
        audiences: vec!["api".into()],
    };
    let registry = providers(1, Some(binding));

    // Route-level audiences replace the provider audiences; issuers stay.
    let overridden = http_def("aud-override")
        .with_security_policy(allow_policy_config())
        .with_security_provider("idp-a")
        .with_security_audiences(vec!["api-2".into()]);
    let plan = compile_route_security_plan(&overridden, &registry)
        .expect("compilation must succeed")
        .expect("plan expected");
    assert_eq!(
        plan.audience_binding,
        Some(AudienceBinding {
            issuers: vec!["https://a".into()],
            audiences: vec!["api-2".into()],
        })
    );

    // No override → provider binding copied verbatim.
    let plain = http_def("aud-plain")
        .with_security_policy(allow_policy_config())
        .with_security_provider("idp-a");
    let plan = compile_route_security_plan(&plain, &registry)
        .expect("compilation must succeed")
        .expect("plan expected");
    assert_eq!(
        plan.audience_binding,
        Some(AudienceBinding {
            issuers: vec!["https://a".into()],
            audiences: vec!["api".into()],
        })
    );
}

#[test]
fn compilation_named_provider_missing_fails() {
    let def = http_def("miss-route")
        .with_security_policy(allow_policy_config())
        .with_security_provider("ghost");
    let err = compile_route_security_plan(&def, &providers(1, None))
        .expect_err("missing provider must fail, never downgrade to Public");
    let msg = err.to_string();
    assert!(msg.contains("miss-route"), "got: {msg}");
    assert!(msg.contains("ghost"), "got: {msg}");
}

#[test]
fn compilation_unnamed_policy_sole_provider_resolves() {
    // Stand-in for the `wasm` authorization-only DSL form; design.md
    // blesses form-blind classification (only the policy-present shape
    // matters): exactly one registered provider resolves into provider_ref.
    let def = http_def("wasm-sole").with_security_policy(allow_policy_config());
    let plan = compile_route_security_plan(&def, &providers(1, None))
        .expect("compilation must succeed")
        .expect("plan expected");
    assert!(matches!(plan.access_mode, AccessMode::Authorized(_)));
    assert_eq!(plan.provider_ref.as_deref(), Some("idp-a"));
}

#[test]
fn compilation_unnamed_policy_zero_providers_fails() {
    // Stand-in for the `ref`/`wasm`/`permission` authorization-only forms
    // (form-blind): zero providers must fail, never downgrade to Public.
    let def = http_def("zero-providers").with_security_policy(allow_policy_config());
    let err = compile_route_security_plan(&def, &providers(0, None))
        .expect_err("zero providers must fail, never downgrade to Public");
    let msg = err.to_string();
    assert!(msg.contains("zero-providers"), "got: {msg}");
}

#[test]
fn compilation_unnamed_policy_multiple_providers_fails() {
    // Stand-in for the `ref` authorization-only form (form-blind):
    // multiple unnamed providers must require selection.
    let def = http_def("ref-multi").with_security_policy(allow_policy_config());
    let err = compile_route_security_plan(&def, &providers(2, None))
        .expect_err("multiple unnamed providers must require selection");
    let msg = err.to_string();
    assert!(msg.contains("ref-multi"), "got: {msg}");
    assert!(msg.contains("provider"), "got: {msg}");
    assert!(msg.contains("idp-a"), "got: {msg}");
    assert!(msg.contains("idp-b"), "got: {msg}");
}

#[test]
fn compilation_rejects_queryparam_on_mcp_and_ws() {
    let sources = vec![CredentialSource::QueryParam {
        param: "token".into(),
    }];

    let mcp = RouteDefinition::new("mcp:test", vec![])
        .with_route_id("mcp-q")
        .with_security_policy(
            SecurityPolicyConfig::new(AllowPolicy).with_credential_sources(sources.clone()),
        );
    let err = compile_route_security_plan(&mcp, &providers(1, None))
        .expect_err("QueryParam must be rejected on mcp");
    let msg = err.to_string();
    assert!(msg.contains("QueryParam"), "got: {msg}");
    assert!(msg.contains("mcp"), "got: {msg}");

    let ws = RouteDefinition::new("ws://127.0.0.1:8081", vec![])
        .with_route_id("ws-q")
        .with_security_policy(
            SecurityPolicyConfig::new(AllowPolicy).with_credential_sources(sources.clone()),
        );
    let err = compile_route_security_plan(&ws, &providers(1, None))
        .expect_err("QueryParam must be rejected on ws");
    let msg = err.to_string();
    assert!(msg.contains("QueryParam"), "got: {msg}");
    assert!(msg.contains("ws"), "got: {msg}");

    // Http accepts every source, QueryParam included.
    let http = http_def("http-q").with_security_policy(
        SecurityPolicyConfig::new(AllowPolicy).with_credential_sources(sources),
    );
    let plan = compile_route_security_plan(&http, &providers(1, None))
        .expect("http accepts QueryParam")
        .expect("plan expected");
    assert_eq!(plan.transport, TransportId::Http);
    assert!(
        plan.credential_sources
            .iter()
            .any(|s| matches!(s, CredentialSource::QueryParam { .. }))
    );
}

#[test]
fn compilation_rejects_cookie_on_grpc() {
    let sources = vec![CredentialSource::Cookie {
        name: "session".into(),
    }];

    let grpc = RouteDefinition::new("grpc://127.0.0.1:18082", vec![])
        .with_route_id("grpc-cookie")
        .with_security_policy(
            SecurityPolicyConfig::new(AllowPolicy).with_credential_sources(sources),
        );
    let err = compile_route_security_plan(&grpc, &providers(1, None))
        .expect_err("Cookie must be rejected on grpc");
    let msg = err.to_string();
    assert!(msg.contains("Cookie"), "got: {msg}");
    assert!(msg.contains("grpc"), "got: {msg}");
}

// ── wasm: source-route classification (Task 1.2) ──

fn wasm_def(id: &str) -> RouteDefinition {
    RouteDefinition::new("wasm://guest-fixture.wasm?bind=127.0.0.1:0", vec![]).with_route_id(id)
}

#[test]
fn wasm_route_with_security_classifies() {
    let def = wasm_def("wasm-sec").with_security_policy(allow_policy_config());
    let plan = compile_route_security_plan(&def, &providers(1, None))
        .expect("compilation must succeed")
        .expect("wasm consumer route must get a plan");
    assert!(
        matches!(plan.access_mode, AccessMode::Authorized(_)),
        "declared security must classify Authorized, got {:?}",
        plan.access_mode
    );
    assert_eq!(plan.provider_ref.as_deref(), Some("idp-a"));
    assert_eq!(plan.transport, TransportId::Wasm);
}

#[test]
fn wasm_route_without_security_stays_public() {
    let def = wasm_def("wasm-public");
    let plan = compile_route_security_plan(&def, &providers(0, None))
        .expect("compilation must succeed")
        .expect("wasm consumer route must get a plan");
    assert!(matches!(plan.access_mode, AccessMode::Public));
    assert_eq!(plan.provider_ref, None);
    assert_eq!(plan.transport, TransportId::Wasm);
}

#[test]
fn wasm_transport_name_and_all_sources_allowed() {
    assert_eq!(transport_name(TransportId::Wasm), "wasm");
    // A `wasm:` source route carries a full HTTP listener, so every
    // CredentialSource variant is permitted (blessed spec: MODIFIED
    // "Transport credential capability validation").
    for source in [
        CredentialSource::AuthorizationHeader,
        CredentialSource::Header {
            name: "x-api-key".into(),
        },
        CredentialSource::Cookie {
            name: "session".into(),
        },
        CredentialSource::QueryParam {
            param: "token".into(),
        },
    ] {
        assert!(
            credential_source_allowed(TransportId::Wasm, &source),
            "wasm must allow {}",
            source.variant_name()
        );
    }
}

// ── staging test ──

struct RecordingComponent {
    scheme: &'static str,
    log: Arc<Mutex<Vec<String>>>,
}

#[async_trait::async_trait]
impl Component for RecordingComponent {
    fn scheme(&self) -> &str {
        self.scheme
    }

    fn create_endpoint(
        &self,
        _uri: &str,
        _ctx: &dyn ComponentContext,
    ) -> Result<Box<dyn Endpoint>, CamelError> {
        Ok(Box::new(RecordingEndpoint {
            scheme: self.scheme,
            log: Arc::clone(&self.log),
        }))
    }
}

struct RecordingEndpoint {
    scheme: &'static str,
    log: Arc<Mutex<Vec<String>>>,
}

impl Endpoint for RecordingEndpoint {
    fn uri(&self) -> &str {
        self.scheme
    }

    fn create_consumer(
        &self,
        _rt: Arc<dyn RuntimeObservability>,
    ) -> Result<Box<dyn Consumer>, CamelError> {
        Ok(Box::new(RecordingConsumer {
            scheme: self.scheme,
            log: Arc::clone(&self.log),
        }))
    }

    fn create_producer(
        &self,
        _rt: Arc<dyn RuntimeObservability>,
        _ctx: &ProducerContext,
    ) -> Result<BoxProcessor, CamelError> {
        Ok(BoxProcessor::new(IdentityProcessor))
    }
}

/// Records `plan:<scheme>:<transport>` when the controller threads the
/// compiled plan via `set_security_context`, and `start:<scheme>` when the
/// consumer starts — one timeline proving compilation precedes start.
struct RecordingConsumer {
    scheme: &'static str,
    log: Arc<Mutex<Vec<String>>>,
}

#[async_trait::async_trait]
impl Consumer for RecordingConsumer {
    async fn start(&mut self, ctx: ConsumerContext) -> Result<(), CamelError> {
        self.log
            .lock()
            .expect("recorder log")
            .push(format!("start:{}", self.scheme));
        ctx.mark_ready();
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), CamelError> {
        Ok(())
    }

    fn concurrency_model(&self) -> ConcurrencyModel {
        ConcurrencyModel::Concurrent { max: None }
    }

    fn startup_mode(&self) -> ConsumerStartupMode {
        ConsumerStartupMode::Explicit
    }

    fn set_security_context(&mut self, ctx: SecurityContext) {
        let transport = ctx
            .plan
            .as_ref()
            .map(|p| transport_name(p.transport))
            .unwrap_or("none");
        self.log
            .lock()
            .expect("recorder log")
            .push(format!("plan:{}:{}", self.scheme, transport));
    }
}

#[tokio::test]
async fn staging_attaches_plan_before_consumer_start_all_schemes() {
    use crate::lifecycle::adapters::route_controller::DefaultRouteController;
    use crate::shared::components::domain::Registry;

    let log: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let component_registry = Arc::new(std::sync::Mutex::new(Registry::new()));
    {
        let mut guard = component_registry.lock().expect("registry lock");
        for scheme in ["http", "ws", "grpc", "mcp"] {
            guard.register(Arc::new(RecordingComponent {
                scheme,
                log: Arc::clone(&log),
            }));
        }
    }

    let mut controller = DefaultRouteController::new(
        component_registry,
        Arc::new(camel_api::NoopPlatformService::default()),
    );

    let provider_registry = Arc::new(providers(1, None));
    let cases: &[(&str, &str, TransportId)] = &[
        ("http", "http://127.0.0.1:18080/api", TransportId::Http),
        ("ws", "ws://127.0.0.1:18081", TransportId::Ws),
        ("grpc", "grpc://127.0.0.1:18082", TransportId::Grpc),
        ("mcp", "mcp:staging-test", TransportId::Mcp),
    ];

    // Staging: each consumer route compiles a plan at add time, before
    // any consumer exists.
    for (scheme, uri, transport) in cases {
        let def = RouteDefinition::new(*uri, vec![])
            .with_route_id(format!("{scheme}-route"))
            .with_security_policy(allow_policy_config())
            .with_security_authenticator(Arc::new(StubAuth))
            .with_provider_registry(Arc::clone(&provider_registry));
        controller
            .add_route(def)
            .await
            .unwrap_or_else(|e| panic!("staging {scheme} must succeed: {e}"));

        let managed = controller
            .routes
            .get(&format!("{scheme}-route"))
            .expect("route must be registered after staging");
        let plan = managed
            .compiled
            .security_plan
            .as_ref()
            .unwrap_or_else(|| panic!("{scheme} route must hold a compiled plan"));
        assert_eq!(plan.transport, *transport);
    }

    // Start every route; the recording consumers put plan attach and
    // start on one timeline.
    for (scheme, _, _) in cases {
        controller
            .start_route(&format!("{scheme}-route"))
            .await
            .unwrap_or_else(|e| panic!("start {scheme} must succeed: {e}"));
    }

    let recorded = log.lock().expect("recorder log").clone();
    for (scheme, _, transport) in cases {
        let plan_event = format!("plan:{scheme}:{}", transport_name(*transport));
        let start_event = format!("start:{scheme}");
        let ip = recorded
            .iter()
            .position(|e| e == &plan_event)
            .unwrap_or_else(|| panic!("missing '{plan_event}' in {recorded:?}"));
        let is = recorded
            .iter()
            .position(|e| e == &start_event)
            .unwrap_or_else(|| panic!("missing '{start_event}' in {recorded:?}"));
        assert!(
            ip < is,
            "plan attach must precede consumer start for {scheme}: {recorded:?}"
        );
    }

    for (scheme, _, _) in cases {
        controller
            .stop_route(&format!("{scheme}-route"))
            .await
            .expect("stop must succeed");
    }
}

#[tokio::test]
async fn gate_ack_does_not_excuse_failed_sibling() {
    use std::collections::HashMap;

    use crate::lifecycle::adapters::route_controller::DefaultRouteController;
    use crate::lifecycle::adapters::route_controller_trait::BindExposureAcks;
    use crate::shared::components::domain::Registry;

    let log: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let component_registry = Arc::new(std::sync::Mutex::new(Registry::new()));
    {
        let mut guard = component_registry.lock().expect("registry lock");
        guard.register(Arc::new(RecordingComponent {
            scheme: "http",
            log: Arc::clone(&log),
        }));
    }

    let mut controller = DefaultRouteController::new(
        component_registry,
        Arc::new(camel_api::NoopPlatformService::default()),
    );
    // Operator acknowledged public exposure on the bind — it must NOT
    // excuse a sibling whose security plan fails to compile.
    controller.set_bind_exposure_acks(BindExposureAcks::new(HashMap::from([(
        "0.0.0.0:18099".to_string(),
        true,
    )])));

    // Good Public sibling on the bind.
    controller
        .add_route(
            RouteDefinition::new("http://0.0.0.0:18099/public", vec![])
                .with_route_id("public-route"),
        )
        .await
        .expect("public route stages fine");

    // Bad sibling: named provider that cannot resolve → staging must
    // abort with the classification error (Task 1.8), never reaching
    // the gate.
    let bad = RouteDefinition::new("http://0.0.0.0:18099/secured", vec![])
        .with_route_id("bad-route")
        .with_security_policy(allow_policy_config())
        .with_security_provider("ghost")
        .with_provider_registry(Arc::new(providers(1, None)));
    let err = controller
        .add_route(bad)
        .await
        .expect_err("unresolvable provider must abort staging");
    let msg = err.to_string();
    assert!(msg.contains("ghost"), "error must name the provider: {msg}");
    assert!(
        !msg.contains("allow_public_exposure"),
        "gate must not be reached by a classification failure: {msg}"
    );
}
