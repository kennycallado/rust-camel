//! Cross-transport substitution E2E (`unify-transport-auth`, Task 3.3).
//!
//! Two JWT providers share the SAME issuer (`https://shared`) and audience
//! (`api`) binding but trust DISJOINT signing keysets (A knows only key-a,
//! B knows only key-b) — the shared binding isolates the per-provider key
//! axis, so a denial can never be masked by issuer or audience rejection.
//!
//! The four scenarios pin the Phase-3 enforcement over real routes:
//! - cross-provider: key-A token on B's route dies in B's OWN signature
//!   verification (kid unknown to B's keyset); B stores no cache entry;
//! - issuer isolation: a token SIGNED by B's key but CARRYING
//!   `iss https://attacker` dies in the issuer-set check — the signature
//!   proves the key, never the issuer;
//! - cross-transport: the same token+provider grants on http AND ws routes
//!   while the shared authn cache keeps TWO entries (transport keys them
//!   apart);
//! - audience-distinguished: a route-level audience override (`api-2`)
//!   denies an `api-1` token even though the same provider+token has a
//!   live cache entry under the accepting route's binding.
//!
//! Cache assertions read the `Arc<AuthnCache>` the fixture itself attached
//! via `ProviderRegistry::with_authn_cache` (Task 3.2 public surface) —
//! no production API was added for these tests.
//!
//! Requires `integration-tests` feature to compile and run.

#![cfg(feature = "integration-tests")]

mod support;
use support::install_crypto_provider;
use support::{stage_http_listener, stage_ws_listener};

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use camel_api::Value;
use camel_api::security_policy::{AudienceBinding, CredentialSource, SecurityPolicyConfig};
use camel_auth::types::AuthError;
use camel_auth::{
    AuthnCache, AuthnCacheOptions, ClaimPaths, JsonPointerClaimsMapper, Jwk, JwksProvider,
    LocalJwtValidator, ProviderEntry, ProviderRegistry, RolePolicy, TokenAuthenticator,
};
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_http::HttpComponent;
use camel_component_ws::WsComponent;
use camel_test::CamelTestContext;
use futures::SinkExt;
use serde_json::json;
use tokio_tungstenite::MaybeTlsStream;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::ClientRequestBuilder;
use tokio_tungstenite::tungstenite::protocol::Message as WsMessage;

const SHARED_ISSUER: &str = "https://shared";
const ATTACKER_ISSUER: &str = "https://attacker";
const AUD_API: &str = "api";
const KID_A: &str = "key-a";
const KID_B: &str = "key-b";

static KEY_A_PRIVATE: &[u8] = include_bytes!("fixtures/subst_key_a_private.pem");
static KEY_A_PUBLIC: &[u8] = include_bytes!("fixtures/subst_key_a_public.pem");
static KEY_B_PRIVATE: &[u8] = include_bytes!("fixtures/subst_key_b_private.pem");
static KEY_B_PUBLIC: &[u8] = include_bytes!("fixtures/subst_key_b_public.pem");

/// JWKS serving exactly ONE public key — provider A's keyset holds only
/// key-a, provider B's only key-b (disjoint by construction).
struct SingleKeyJwks {
    kid: &'static str,
    public_pem: &'static [u8],
}

#[async_trait::async_trait]
impl JwksProvider for SingleKeyJwks {
    async fn get_signing_keys(&self) -> Result<Vec<Jwk>, AuthError> {
        Ok(vec![Jwk {
            kid: self.kid.to_string(),
            kty: "RSA".to_string(),
            alg: Some("RS256".to_string()),
            r#use: None,
            n: String::from_utf8_lossy(self.public_pem).into_owned(),
            e: "AQAB".to_string(),
        }])
    }

    async fn refresh(&self) -> Result<(), AuthError> {
        Ok(())
    }
}

/// JWT provider resolving `kid` against one public key (make_token/
/// jwt_validator precedent: `crates/services/camel-auth/src/token_authenticator.rs`).
fn jwt_provider(kid: &'static str, public_pem: &'static [u8]) -> Arc<dyn TokenAuthenticator> {
    let mapper = Arc::new(JsonPointerClaimsMapper::new(ClaimPaths {
        subject: "/sub".into(),
        roles: vec!["/groups".into()],
        scopes: Some("/scope".into()),
    }));
    Arc::new(LocalJwtValidator::new(
        vec![AUD_API.to_string()],
        SHARED_ISSUER.to_string(),
        Arc::new(SingleKeyJwks { kid, public_pem }),
        mapper,
    ))
}

/// Mint an RS256 JWT whose issuer and audience are payload claims — an
/// attacker controls them; only the signature is trustworthy.
fn make_token(private_pem: &[u8], kid: &str, iss: &str, aud: &str) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock after unix epoch") // allow-unwrap
        .as_secs();
    let claims = json!({
        "sub": "substitution-user",
        "iss": iss,
        "aud": aud,
        "groups": ["test-role"],
        "iat": now,
        "exp": now + 3600,
    });
    let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256);
    header.kid = Some(kid.to_string());
    let encoding = jsonwebtoken::EncodingKey::from_rsa_pem(private_pem).expect("fixture key"); // allow-unwrap
    jsonwebtoken::encode(&header, &claims, &encoding).expect("encode jwt") // allow-unwrap
}

/// Task 3.3 step 1 fixture: two providers, SAME issuer+audience binding,
/// DISJOINT keysets. The returned `Arc<AuthnCache>` is the one attached to
/// the registry — entry counts are observable without new production API.
fn substitution_registry() -> (Arc<AuthnCache>, Arc<ProviderRegistry>) {
    let cache = Arc::new(AuthnCache::new(AuthnCacheOptions::default()));
    let binding = AudienceBinding {
        issuers: vec![SHARED_ISSUER.to_string()],
        audiences: vec![AUD_API.to_string()],
    };
    let registry = ProviderRegistry::new().with_authn_cache(cache.clone());
    registry.register(
        "idp-a",
        ProviderEntry {
            authenticator: jwt_provider(KID_A, KEY_A_PUBLIC),
            audience_binding: Some(binding.clone()),
        },
    );
    registry.register(
        "idp-b",
        ProviderEntry {
            authenticator: jwt_provider(KID_B, KEY_B_PUBLIC),
            audience_binding: Some(binding),
        },
    );
    (cache, Arc::new(registry))
}

/// Secured HTTP route over the shared registry (build_secured_route
/// precedent: `kernel_fail_closed_test.rs`). The provider is DECLARED
/// because the registry holds two entries; `route_audiences` overrides the
/// provider binding at the route level (plan compilation, Task 1.8).
async fn build_http_route(
    registry: &Arc<ProviderRegistry>,
    provider: &str,
    route_audiences: Option<Vec<String>>,
) -> (CamelTestContext, u16) {
    install_crypto_provider();
    let port = stage_http_listener("127.0.0.1").await;

    let h = CamelTestContext::builder()
        .with_component(HttpComponent::new())
        .with_mock()
        .build()
        .await;

    let entry = registry.resolve(provider).expect("fixture provider"); // allow-unwrap
    let policy = RolePolicy::new(vec!["test-role".to_string()], true);
    let config = SecurityPolicyConfig::new(policy)
        .with_credential_sources(vec![CredentialSource::AuthorizationHeader]);

    let mut definition = RouteBuilder::from(&format!("http://127.0.0.1:{port}/sub/{provider}"))
        .route_id(format!("substitution-http-{provider}-{port}"))
        .security_policy(config)
        .security_authenticator(Arc::clone(&entry.authenticator))
        .provider_registry(Arc::clone(registry))
        .set_body(Value::String("ok".into()))
        .set_header("CamelHttpResponseCode", Value::Number(200.into()))
        .to("mock:result".to_string())
        .build()
        .unwrap()
        .with_security_provider(provider.to_string());
    if let Some(audiences) = route_audiences {
        definition = definition.with_security_audiences(audiences);
    }

    h.add_route(definition).await.unwrap();
    h.start().await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    (h, port)
}

/// Secured WS route over the shared registry: the compiled plan + provider
/// registry ride the route's `SecurityContext` into the ws server state
/// (Task 2.6/2.8), so the upgrade handshake authenticates through the same
/// kernel — and the same attached authn cache — as the HTTP routes.
async fn build_ws_route(
    registry: &Arc<ProviderRegistry>,
    provider: &str,
) -> (CamelTestContext, u16) {
    install_crypto_provider();
    let port = stage_ws_listener("127.0.0.1").await;

    let h = CamelTestContext::builder()
        .with_component(WsComponent::new())
        .with_mock()
        .build()
        .await;

    let entry = registry.resolve(provider).expect("fixture provider"); // allow-unwrap
    let policy = RolePolicy::new(vec!["test-role".to_string()], true);
    let config = SecurityPolicyConfig::new(policy)
        .with_credential_sources(vec![CredentialSource::AuthorizationHeader]);

    let definition = RouteBuilder::from(&format!("ws://127.0.0.1:{port}/sub/{provider}"))
        .route_id(format!("substitution-ws-{provider}-{port}"))
        .security_policy(config)
        .security_authenticator(Arc::clone(&entry.authenticator))
        .provider_registry(Arc::clone(registry))
        .to("mock:result".to_string())
        .build()
        .unwrap()
        .with_security_provider(provider.to_string());

    h.add_route(definition).await.unwrap();
    h.start().await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    (h, port)
}

type WsClient = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

/// Open a ws upgrade carrying the Bearer token (kernel_auth_test precedent:
/// `crates/components/camel-ws/tests/kernel_auth_test.rs`).
async fn ws_connect(
    port: u16,
    path: &str,
    token: &str,
) -> (
    WsClient,
    tokio_tungstenite::tungstenite::http::Response<Option<Vec<u8>>>,
) {
    let uri: tokio_tungstenite::tungstenite::http::Uri = format!("ws://127.0.0.1:{port}{path}")
        .parse()
        .expect("valid ws uri"); // allow-unwrap
    let builder =
        ClientRequestBuilder::new(uri).with_header("Authorization", format!("Bearer {token}"));
    tokio_tungstenite::connect_async(builder)
        .await
        .expect("ws connect must not fail at transport level") // allow-unwrap
}

/// Poll the mock inbox until it holds `want` exchanges (2 s ceiling).
async fn await_mock_count(h: &CamelTestContext, want: usize) {
    for _ in 0..40 {
        if let Some(inbox) = h.mock().get_endpoint("result")
            && inbox.received_count().await >= want
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let inbox = h.mock().get_endpoint("result").expect("mock endpoint"); // allow-unwrap
    panic!(
        "mock:result did not reach {want} exchanges, got {}",
        inbox.received_count().await
    );
}

async fn http_get(port: u16, path: &str, token: &str) -> reqwest::Response {
    let client = reqwest::Client::new();
    client
        .get(format!("http://127.0.0.1:{port}{path}"))
        .header("Authorization", format!("Bearer {token}"))
        .send()
        .await
        .unwrap()
}

/// Case 2: a token signed with key-A (trusted only by A) is replayed on
/// B's route. B denies it in its OWN signature verification — the kid is
/// unknown to B's keyset — so the denial cannot be an issuer or audience
/// artifact (the binding is identical on both providers). B's cache stays
/// empty: the miss stored nothing, and even A's later grant for the SAME
/// token does not become a B entry.
#[tokio::test(flavor = "multi_thread")]
async fn cross_provider_substitution_rejected_e2e() {
    let (cache, registry) = substitution_registry();
    let token = make_token(KEY_A_PRIVATE, KID_A, SHARED_ISSUER, AUD_API);

    let (h_b, port_b) = build_http_route(&registry, "idp-b", None).await;
    let resp = http_get(port_b, "/sub/idp-b", &token).await;
    assert_eq!(
        resp.status(),
        401,
        "key-A token must die in B's own signature verification"
    );
    let inbox_b = h_b.mock().get_endpoint("result").expect("mock endpoint"); // allow-unwrap
    assert_eq!(
        inbox_b.received_count().await,
        0,
        "denied request must never reach the body"
    );
    assert_eq!(
        cache.len(),
        0,
        "B's denial stored nothing — denials are never cached"
    );

    // Control: the SAME token is valid on A's own route.
    let (h_a, port_a) = build_http_route(&registry, "idp-a", None).await;
    let resp = http_get(port_a, "/sub/idp-a", &token).await;
    assert_eq!(resp.status(), 200, "the token itself is valid for A");
    let inbox_a = h_a.mock().get_endpoint("result").expect("mock endpoint"); // allow-unwrap
    assert_eq!(inbox_a.received_count().await, 1);
    assert_eq!(
        cache.len(),
        1,
        "only A's grant is cached — provider is part of the cache key"
    );
}

/// Case 3: a token SIGNED by B's trusted key but CARRYING
/// `iss https://attacker`. The signature proves the key, never the issuer
/// (issuer is a payload claim the attacker controls). B's binding accepts
/// only `https://shared`, so the issuer-set check denies — after signature
/// verification succeeded. The honest-token control grants first, proving
/// the key+route pair is otherwise fine; the forged denial neither reaches
/// the body nor disturbs the honest cache entry.
#[tokio::test(flavor = "multi_thread")]
async fn issuer_isolation_e2e() {
    let (cache, registry) = substitution_registry();
    let (h, port) = build_http_route(&registry, "idp-b", None).await;

    // Control: key-B signature + shared issuer grants on B's route.
    let honest = make_token(KEY_B_PRIVATE, KID_B, SHARED_ISSUER, AUD_API);
    let resp = http_get(port, "/sub/idp-b", &honest).await;
    assert_eq!(resp.status(), 200, "honest key-B token must grant");
    assert_eq!(cache.len(), 1, "the honest grant is cached once");

    // Attack: same key-B signature, attacker-controlled issuer claim.
    let forged = make_token(KEY_B_PRIVATE, KID_B, ATTACKER_ISSUER, AUD_API);
    let resp = http_get(port, "/sub/idp-b", &forged).await;
    assert_eq!(
        resp.status(),
        401,
        "valid signature under an unaccepted issuer must be denied by the issuer-set check"
    );
    let inbox = h.mock().get_endpoint("result").expect("mock endpoint"); // allow-unwrap
    assert_eq!(
        inbox.received_count().await,
        1,
        "only the honest request reached the body"
    );
    assert_eq!(
        cache.len(),
        1,
        "the forged denial stored nothing and the honest entry is untouched"
    );
}

/// Case 4: the SAME token+provider grants on an http route AND a ws route
/// sharing one registry (hence one authn cache). Transport is part of the
/// cache key, so the two grants are TWO entries — and each subsequent
/// request on its transport is served from its own entry (counts freeze).
#[tokio::test(flavor = "multi_thread")]
async fn same_audience_cross_transport_isolated_cache_e2e() {
    let (cache, registry) = substitution_registry();
    let token = make_token(KEY_A_PRIVATE, KID_A, SHARED_ISSUER, AUD_API);

    // HTTP: grant, then a second request served from the cache — one entry.
    let (h_http, http_port) = build_http_route(&registry, "idp-a", None).await;
    let resp = http_get(http_port, "/sub/idp-a", &token).await;
    assert_eq!(resp.status(), 200, "http grant");
    assert_eq!(cache.len(), 1, "the http grant cached exactly one entry");
    let resp = http_get(http_port, "/sub/idp-a", &token).await;
    assert_eq!(resp.status(), 200, "second http request reuses the entry");
    assert_eq!(
        cache.len(),
        1,
        "same transport+binding+token reuses the single entry"
    );

    // WS: same registry, same token+provider — the upgrade authorizes.
    let (h_ws, ws_port) = build_ws_route(&registry, "idp-a").await;
    let (mut ws, response) = ws_connect(ws_port, "/sub/idp-a", &token).await;
    assert_eq!(
        response.status(),
        101,
        "ws upgrade must authorize the same token+provider"
    );

    // One message through the ws pipeline: the dispatch check passes with
    // the ws-minted carrier and the exchange reaches the body.
    ws.send(WsMessage::Text("ping".into())).await.unwrap();
    await_mock_count(&h_ws, 1).await;
    assert_eq!(
        cache.len(),
        2,
        "transport is part of the cache key — the ws grant is a SECOND entry"
    );

    // A second ws upgrade is served from the ws entry: still two.
    let (_ws2, response2) = ws_connect(ws_port, "/sub/idp-a", &token).await;
    assert_eq!(
        response2.status(),
        101,
        "second ws upgrade reuses the ws entry"
    );
    assert_eq!(cache.len(), 2, "no third entry for the repeated ws request");

    let _ = h_http;
}

/// Case 5: one provider accepting `api-1` (provider binding); a second
/// route on the SAME issuer+provider declares route-level audiences
/// `["api-2"]`. An `api-1` token grants on the first route and is denied
/// on the second — with the SAME token, provider, and issuer, the denial
/// proves the audience override both keys the cache apart AND is enforced:
/// the live `api-1` cache entry is NOT reused for the `api-2` route.
#[tokio::test(flavor = "multi_thread")]
async fn audience_distinguished_no_cache_reuse_e2e() {
    let cache = Arc::new(AuthnCache::new(AuthnCacheOptions::default()));
    let registry = ProviderRegistry::new().with_authn_cache(cache.clone());
    registry.register(
        "idp-a",
        ProviderEntry {
            authenticator: jwt_provider(KID_A, KEY_A_PUBLIC),
            audience_binding: Some(AudienceBinding {
                issuers: vec![SHARED_ISSUER.to_string()],
                audiences: vec!["api-1".to_string()],
            }),
        },
    );
    let registry = Arc::new(registry);
    let token = make_token(KEY_A_PRIVATE, KID_A, SHARED_ISSUER, "api-1");

    // R1 accepts api-1 via the provider binding: grant, one cache entry.
    let (h1, port1) = build_http_route(&registry, "idp-a", None).await;
    let resp = http_get(port1, "/sub/idp-a", &token).await;
    assert_eq!(resp.status(), 200, "api-1 token grants on the api-1 route");
    assert_eq!(cache.len(), 1, "the api-1 grant is cached once");

    // R2: same provider+issuer, route-level audiences ["api-2"].
    let (h2, port2) = build_http_route(&registry, "idp-a", Some(vec!["api-2".to_string()])).await;
    let resp = http_get(port2, "/sub/idp-a", &token).await;
    assert_eq!(
        resp.status(),
        401,
        "api-1 token must be denied on the api-2 route — audience override enforced"
    );
    let inbox2 = h2.mock().get_endpoint("result").expect("mock endpoint"); // allow-unwrap
    assert_eq!(
        inbox2.received_count().await,
        0,
        "denied request never runs the body"
    );
    assert_eq!(
        cache.len(),
        1,
        "no cache reuse: the api-1 entry did not satisfy the api-2 route, and the denial stored nothing"
    );

    let _ = h1;
}
