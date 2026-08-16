//! Credential-source extraction example (rc-5f9v).
//!
//! Browser tile services (`<img src>` from Leaflet, MapLibre, OpenLayers)
//! cannot set the `Authorization` header. This example serves one
//! `http://` route whose `security_policy` declares a credential-source
//! fallback chain: cookie first, then query parameter, then custom
//! header, then the Bearer default.
//!
//! Run:
//! ```bash
//! cargo run -p credential-sources
//! ```
//!
//! Then try:
//! ```bash
//! # cookie (the browser tile-service transport)
//! curl -H "Cookie: session=demo-tile-token-0f1e2d3c" http://127.0.0.1:8090/tiles
//! # query parameter
//! curl "http://127.0.0.1:8090/tiles?token=demo-tile-token-0f1e2d3c"
//! # custom API-key header
//! curl -H "X-Api-Key: demo-tile-token-0f1e2d3c" http://127.0.0.1:8090/tiles
//! # default Bearer header
//! curl -H "Authorization: Bearer demo-tile-token-0f1e2d3c" http://127.0.0.1:8090/tiles
//! # no credential anywhere -> 401
//! curl -i http://127.0.0.1:8090/tiles
//! ```
//!
//! The demo token is static for the example only. Production services
//! replace `StaticTokenAuthenticator` with an introspection endpoint or a
//! JWT validator (see `docs/src/services/auth.md`).

use std::sync::Arc;

use camel_api::CamelError;
use camel_api::security_policy::Principal;
use camel_auth::native_auth::NativeCredentialStore;
use camel_auth::{
    NativeCredential, NativeCredentialSecret, StaticTokenAuthenticator, TokenAuthenticator,
};
use camel_component_http::HttpComponent;
use camel_core::context::CamelContext;
use camel_dsl::{SecurityCompileContext, parse_yaml_with_threshold_and_security};

const ROUTES_YAML: &str = include_str!("../routes.yaml");
const DEMO_TOKEN: &str = "demo-tile-token-0f1e2d3c";
const TILE_ROLE: &str = "tile-user";

#[tokio::main]
async fn main() -> Result<(), CamelError> {
    tracing_subscriber::fmt().with_target(false).init();

    // One static token mapped to a principal holding the required role.
    // Store lookups run in constant time; the secret never appears in
    // diagnostics (ADR-0051).
    let principal = Principal {
        subject: "tiles-demo-user".into(),
        issuer: "native".into(),
        audience: vec![],
        scopes: vec![],
        roles: vec![TILE_ROLE.to_string()],
        claims: serde_json::json!({}),
    };
    let store = NativeCredentialStore::try_new(vec![NativeCredential {
        secret: NativeCredentialSecret::Plaintext {
            value: DEMO_TOKEN.to_string().into(),
        },
        principal,
    }])?;
    let authenticator: Arc<dyn TokenAuthenticator> = Arc::new(StaticTokenAuthenticator::new(store));

    // Compile the YAML with the authenticator so `security_policy` blocks
    // resolve against it at load time.
    let security = SecurityCompileContext::new(Some(authenticator), None);
    let definitions = parse_yaml_with_threshold_and_security(ROUTES_YAML, 1024, security)?;

    let mut ctx = CamelContext::builder().build().await?;
    ctx.register_component(HttpComponent::new());
    for def in definitions {
        ctx.add_route_definition(def).await?;
    }
    ctx.start().await?;

    print_banner();

    tokio::signal::ctrl_c()
        .await
        .map_err(|e| CamelError::Io(e.to_string()))?;
    ctx.stop().await?;
    println!("Stopped.");
    Ok(())
}

fn print_banner() {
    println!("==========================================================");
    println!(" credential-sources example");
    println!("==========================================================");
    println!("GET http://127.0.0.1:8090/tiles  (role required: {TILE_ROLE})");
    println!();
    println!("  cookie    : curl -H 'Cookie: session={DEMO_TOKEN}' http://127.0.0.1:8090/tiles");
    println!("  query     : curl 'http://127.0.0.1:8090/tiles?token={DEMO_TOKEN}'");
    println!("  header    : curl -H 'X-Api-Key: {DEMO_TOKEN}' http://127.0.0.1:8090/tiles");
    println!(
        "  bearer    : curl -H 'Authorization: Bearer {DEMO_TOKEN}' http://127.0.0.1:8090/tiles"
    );
    println!("  anonymous : curl -i http://127.0.0.1:8090/tiles   (401)");
    println!("==========================================================");
}
