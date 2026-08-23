//! Guest-free security-context capture tests for `WasmSourceConsumer`
//! (`wasm-source-auth-kernel`, Task 1.3).
//!
//! These tests exercise ONLY the `set_security_context` capture: no
//! `start()`, no `.wasm` guest fixture, no listener. The consumer is
//! constructed directly and the context is delivered the same way the
//! route controller delivers it — before `start()`. The fixture follows
//! the grpc precedent (`server_auth_test.rs`): the context carries the
//! compiled plan plus the provider registry.

use std::path::PathBuf;
use std::sync::Arc;

use camel_api::security_policy::{AccessMode, CredentialSource, RouteSecurityPlan, TransportId};
use camel_auth::{ProviderRegistry, RolePolicy};
use camel_component_api::{Consumer, NoOpComponentContext, SecurityContext};

use camel_component_wasm::config::WasmConfig;
use camel_component_wasm::source_consumer::WasmSourceConsumer;

fn authenticated_plan() -> RouteSecurityPlan {
    RouteSecurityPlan {
        access_mode: AccessMode::Authenticated,
        provider_ref: Some("idp-wasm".to_string()),
        transport: TransportId::Wasm,
        credential_sources: vec![CredentialSource::AuthorizationHeader],
        audience_binding: None,
    }
}

fn test_consumer() -> WasmSourceConsumer {
    WasmSourceConsumer::new(
        PathBuf::from("unused.wasm"),
        "wasm:unused.wasm",
        WasmConfig::default(),
        Vec::new(),
        Arc::new(NoOpComponentContext),
    )
}

#[test]
fn set_security_context_captures_kernel() {
    let mut consumer = test_consumer();
    let sec_ctx = SecurityContext::new(RolePolicy::new(vec![], true))
        .with_plan(authenticated_plan())
        .with_providers(Arc::new(ProviderRegistry::new()));
    consumer.set_security_context(sec_ctx);
    assert!(matches!(
        consumer.plan_access_mode(),
        Some(AccessMode::Authenticated)
    ));
}

#[test]
fn set_security_context_none_without_plan() {
    let mut consumer = test_consumer();
    let sec_ctx = SecurityContext::new(RolePolicy::new(vec![], true));
    consumer.set_security_context(sec_ctx);
    assert!(consumer.plan_access_mode().is_none());
}

#[test]
fn plan_only_context_captures_classification_without_kernel() {
    let mut consumer = test_consumer();
    let sec_ctx = SecurityContext::from_plan(authenticated_plan());
    consumer.set_security_context(sec_ctx);
    assert!(matches!(
        consumer.plan_access_mode(),
        Some(AccessMode::Authenticated)
    ));
}
