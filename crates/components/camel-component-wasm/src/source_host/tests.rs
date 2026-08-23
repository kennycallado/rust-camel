use super::*;

#[test]
fn test_source_channels_new() {
    let channels = SourceChannels::new();
    assert!(!channels.request_tx.is_closed());
    assert!(!channels.exchange_tx.is_closed());
}

#[test]
fn test_source_channels_default() {
    let channels = SourceChannels::default();
    assert!(!channels.request_tx.is_closed());
}

#[test]
fn test_request_channel_close_returns_none() {
    let (tx, mut rx) = mpsc::channel::<RequestChannelItem>(1);
    drop(tx);
    let result = std::thread::spawn(move || rx.blocking_recv())
        .join()
        .unwrap();
    assert!(result.is_none());
}

#[test]
fn test_exchange_channel_close_detected() {
    let (tx, rx) = mpsc::channel::<(Exchange, oneshot::Sender<SubmitOutcome>)>(1);
    drop(rx);
    assert!(tx.is_closed());
}

#[test]
fn test_cancel_token_is_cancelled() {
    let token = CancellationToken::new();
    assert!(!token.is_cancelled());
    token.cancel();
    assert!(token.is_cancelled());
}

#[test]
fn test_submit_outcome_variants() {
    let accepted = SubmitOutcome::Accepted;
    let stopped = SubmitOutcome::Stopped;
    assert!(matches!(accepted, SubmitOutcome::Accepted));
    assert!(matches!(stopped, SubmitOutcome::Stopped));
}

#[test]
fn source_exchange_to_native_maps_fields() {
    use crate::source_bindings::camel::plugin::types as src;
    let wasm = src::WasmExchange {
        input: src::WasmMessage {
            headers: vec![("key".to_string(), "val".to_string())],
            body: src::WasmBody::Text("hello".to_string()),
        },
        output: None,
        properties: vec![("p".to_string(), "v".to_string())],
        pattern: src::WasmPattern::InOnly,
        correlation_id: "corr-1".to_string(),
        route_id: Some("route-1".to_string()),
        message_id: Some("msg-1".to_string()),
    };

    let native = source_exchange_to_native(wasm);
    assert_eq!(native.input.headers.len(), 1);
    assert_eq!(
        native.input.headers.get("key"),
        Some(&camel_api::Value::String("val".to_string()))
    );
    assert!(matches!(native.input.body, Body::Text(_)));
    assert_eq!(
        native.properties.get("p"),
        Some(&camel_api::Value::String("v".to_string()))
    );
    assert!(matches!(native.pattern, ExchangePattern::InOnly));
    assert_eq!(native.correlation_id, "corr-1");
}

// ─── Task 2.1: pending-principal threading (accept → submit) ──────────

use camel_api::security_policy::{
    AccessMode, AuthPrincipal, CredentialSource, Principal, TransportId,
};
use camel_auth::credential_source::ExtractedToken;
use camel_auth::native_auth::{
    NativeCredential, NativeCredentialSecret, NativeCredentialStore, StaticTokenAuthenticator,
};
use camel_auth::{ProviderEntry, ProviderRegistry};
use zeroize::Zeroizing;

/// Registry with two static-token providers (`idp-a`/`t-a` → `svc-a`,
/// `idp-b`/`t-b` → `svc-b`) — same shape as the camel-auth kernel tests.
fn fixture_registry() -> ProviderRegistry {
    fn register_static(registry: &ProviderRegistry, id: &str, token: &str, subject: &str) {
        let store = NativeCredentialStore::try_new(vec![NativeCredential {
            secret: NativeCredentialSecret::Plaintext {
                value: Zeroizing::new(token.to_string()),
            },
            principal: Principal {
                subject: subject.to_string(),
                issuer: "test".to_string(),
                audience: vec![],
                scopes: vec![],
                roles: vec![],
                claims: serde_json::Value::Null,
            },
        }])
        .unwrap();
        registry.register(
            id,
            ProviderEntry {
                authenticator: Arc::new(StaticTokenAuthenticator::new(store)),
                audience_binding: None,
            },
        );
    }

    let registry = ProviderRegistry::new();
    register_static(&registry, "idp-a", "t-a", "svc-a");
    register_static(&registry, "idp-b", "t-b", "svc-b");
    registry
}

fn wasm_authenticated_plan(provider_ref: &str) -> RouteSecurityPlan {
    RouteSecurityPlan {
        access_mode: AccessMode::Authenticated,
        provider_ref: Some(provider_ref.to_string()),
        transport: TransportId::Wasm,
        credential_sources: vec![CredentialSource::AuthorizationHeader],
        audience_binding: None,
    }
}

async fn mint_principal(
    providers: &ProviderRegistry,
    provider_ref: &str,
    token: &str,
) -> camel_auth::AuthenticatedPrincipal {
    let plan = wasm_authenticated_plan(provider_ref);
    let credentials = ExtractedToken {
        token: token.to_string(),
        source: CredentialSource::AuthorizationHeader,
    };
    camel_auth::kernel_authenticate(&plan, providers, &credentials)
        .await
        .unwrap()
}

/// Fresh host state (same shape the consumer builds). The channels are
/// inert here: only the pending-principal slot is under test.
fn test_host_state() -> SourceHostState {
    let (request_tx, request_rx) = mpsc::channel(REQUEST_CHANNEL_CAPACITY);
    drop(request_tx);
    let (exchange_tx, _exchange_rx) = mpsc::channel(EXCHANGE_CHANNEL_CAPACITY);
    SourceHostState {
        table: ResourceTable::new(),
        wasi: wasmtime_wasi::WasiCtxBuilder::new().build(),
        request_rx: Arc::new(tokio::sync::Mutex::new(request_rx)),
        exchange_tx,
        cancel_token: CancellationToken::new(),
        max_request_body_bytes: DEFAULT_MAX_REQUEST_BODY_BYTES,
        pending_principal: None,
        accept_outstanding: false,
    }
}

fn text_wasm_exchange() -> WasmExchange {
    use crate::source_bindings::camel::plugin::types as src;
    src::WasmExchange {
        input: src::WasmMessage {
            headers: vec![],
            body: src::WasmBody::Text("payload".to_string()),
        },
        output: None,
        properties: vec![],
        pattern: src::WasmPattern::InOnly,
        correlation_id: "corr-test".to_string(),
        route_id: None,
        message_id: None,
    }
}

#[tokio::test]
async fn pending_principal_installed_on_exchange() {
    let providers = fixture_registry();
    let principal = mint_principal(&providers, "idp-a", "t-a").await;

    let mut state = test_host_state();
    // accept_http's stash step (production calls this with
    // `meta.principal.take()` once the request metadata is received).
    state.stash_pending_principal(Some(principal)).unwrap();

    // submit_exchange's assembly + take-and-install steps.
    let mut native = source_exchange_to_native(text_wasm_exchange());
    state.install_pending_carrier(&mut native);

    // The carrier is readable off the assembled Exchange.
    let carrier = camel_auth::read_carrier(&native);
    assert_eq!(carrier.as_ref().map(|p| p.provider_id()), Some("idp-a"));
    // The slot is freed for the next request.
    assert!(state.pending_principal.is_none());
    assert!(!state.accept_outstanding);
}

#[tokio::test]
async fn no_principal_no_carrier() {
    let mut state = test_host_state();
    state.stash_pending_principal(None).unwrap();

    let mut native = source_exchange_to_native(text_wasm_exchange());
    state.install_pending_carrier(&mut native);

    assert!(camel_auth::read_carrier(&native).is_none());
}

#[tokio::test]
async fn double_accept_fails_closed() {
    let providers = fixture_registry();
    let first = mint_principal(&providers, "idp-a", "t-a").await;
    let second = mint_principal(&providers, "idp-b", "t-b").await;

    let mut state = test_host_state();
    state.stash_pending_principal(Some(first)).unwrap();

    // A second accept without an intervening submit-exchange errors…
    assert!(state.stash_pending_principal(Some(second)).is_err());
    // …and the first request's stashed principal was NOT overwritten.
    assert_eq!(
        state.pending_principal.as_ref().map(|p| p.provider_id()),
        Some("idp-a")
    );
    assert!(state.accept_outstanding);
}
