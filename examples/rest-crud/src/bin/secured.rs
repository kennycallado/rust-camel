//! `rest-crud` secured variant (`unify-transport-auth`, Task 2.10).
//!
//! Same CRUD API as `src/main.rs`, but the `rest:` block in
//! `routes/secured.yaml` declares a block-level `security_policy`:
//! lowering copies it onto every lowered route, so all four endpoints
//! authenticate and authorize through the named provider before any
//! `direct:` handler runs.
//!
//! Run:
//! ```bash
//! cargo run -p rest-crud --bin secured
//! ```
//!
//! Then try:
//! ```bash
//! # no credential -> 401 (the body never runs)
//! curl -i http://127.0.0.1:9090/api/users
//! # valid demo token -> the API answers
//! curl -H "Authorization: Bearer demo-rest-token-4a5b6c7d" http://127.0.0.1:9090/api/users
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
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_direct::DirectComponent;
use camel_component_http::HttpComponent;
use camel_core::context::CamelContext;
use camel_dsl::{SecurityCompileContext, parse_yaml_with_threshold_and_security};

#[path = "../storage.rs"]
mod storage;
use storage::{CreateUserRequest, UserStore};

const SECURED_YAML: &str = include_str!("../../routes/secured.yaml");
const DEMO_TOKEN: &str = "demo-rest-token-4a5b6c7d";
const PROVIDER: &str = "native-demo";
const REQUIRED_ROLE: &str = "user";

#[tokio::main]
async fn main() -> Result<(), CamelError> {
    tracing_subscriber::fmt().with_target(false).init();

    let store = Arc::new(UserStore::new());

    // One static token mapped to a principal holding the required role.
    // The provider is registered under the name the YAML block declares
    // (`provider: native-demo`), so the compiled routes carry the plan
    // inputs exactly like a hand-declared secured `http:` route.
    let principal = Principal {
        subject: "rest-demo-user".into(),
        issuer: "native".into(),
        audience: vec![],
        scopes: vec![],
        roles: vec![REQUIRED_ROLE.to_string()],
        claims: serde_json::json!({}),
    };
    let store_creds = NativeCredentialStore::try_new(vec![NativeCredential {
        secret: NativeCredentialSecret::Plaintext {
            value: DEMO_TOKEN.to_string().into(),
        },
        principal,
    }])?;
    let authenticator: Arc<dyn TokenAuthenticator> =
        Arc::new(StaticTokenAuthenticator::new(store_creds));

    let security =
        SecurityCompileContext::default().with_named_authenticator(PROVIDER, authenticator);
    let rest_routes = parse_yaml_with_threshold_and_security(SECURED_YAML, 1024, security)
        .map_err(|e| CamelError::RouteError(format!("Failed to parse secured REST YAML: {e}")))?;
    tracing::info!("Parsed {} secured REST routes", rest_routes.len());

    let mut ctx = CamelContext::builder().build().await?;
    ctx.register_component(HttpComponent::new());
    ctx.register_component(DirectComponent::new());

    for route in rest_routes {
        ctx.add_route_definition(route).await?;
    }

    // The CRUD handlers are identical to the public variant; security is
    // enforced upstream of them, at the lowered route boundary.
    let s = Arc::clone(&store);
    let list_route = RouteBuilder::from("direct:listUsers")
        .route_id("handle-listUsers")
        .process(move |mut exchange| {
            let s = Arc::clone(&s);
            async move {
                let users = s.list();
                exchange.input.body =
                    camel_api::Body::Json(serde_json::to_value(&users).unwrap_or(
                        serde_json::Value::Null, // allow-unwrap
                    ));
                Ok(exchange)
            }
        })
        .build()?;
    ctx.add_route_definition(list_route).await?;

    let s = Arc::clone(&store);
    let create_route = RouteBuilder::from("direct:createUser")
        .route_id("handle-createUser")
        .process(move |mut exchange| {
            let s = Arc::clone(&s);
            async move {
                if let camel_api::Body::Json(value) = exchange.input.body.clone()
                    && let Ok(req) = serde_json::from_value::<CreateUserRequest>(value)
                {
                    let user = s.create(req.name, req.email);
                    exchange.input.body = camel_api::Body::Json(
                        serde_json::to_value(&user).unwrap_or(serde_json::Value::Null), // allow-unwrap
                    );
                    return Ok(exchange);
                }
                exchange
                    .input
                    .set_header("CamelHttpResponseCode", camel_api::Value::from(400u16));
                exchange.input.body = camel_api::Body::Json(serde_json::json!({
                    "error": "invalid_request",
                    "message": "expected application/json body",
                }));
                Ok(exchange)
            }
        })
        .build()?;
    ctx.add_route_definition(create_route).await?;

    let s = Arc::clone(&store);
    let update_route = RouteBuilder::from("direct:updateUser")
        .route_id("handle-updateUser")
        .process(move |mut exchange| {
            let s = Arc::clone(&s);
            async move {
                let id = exchange
                    .input
                    .header("CamelHttpPath_id")
                    .and_then(|v| match v {
                        camel_api::Value::String(s) => s.parse::<u64>().ok(),
                        camel_api::Value::Number(n) => n.as_u64(),
                        _ => None,
                    });
                let req = match exchange.input.body.clone() {
                    camel_api::Body::Json(value) => {
                        serde_json::from_value::<CreateUserRequest>(value)
                    }
                    _ => Err(serde_json::from_str::<CreateUserRequest>("").unwrap_err()), // allow-unwrap
                };
                match (id, req) {
                    (Some(id), Ok(req)) => match s.update(id, req.name, req.email) {
                        Some(updated) => {
                            exchange.input.body = camel_api::Body::Json(
                                serde_json::to_value(&updated).unwrap_or(serde_json::Value::Null), // allow-unwrap
                            );
                        }
                        None => {
                            exchange.input.set_header(
                                "CamelHttpResponseCode",
                                camel_api::Value::from(404u16),
                            );
                            exchange.input.body = camel_api::Body::Json(serde_json::json!({
                                "error": "user_not_found",
                                "message": format!("user {id} not found"),
                            }));
                        }
                    },
                    _ => {
                        exchange
                            .input
                            .set_header("CamelHttpResponseCode", camel_api::Value::from(400u16));
                        exchange.input.body = camel_api::Body::Json(serde_json::json!({
                            "error": "invalid_request",
                            "message": "missing id or invalid body",
                        }));
                    }
                }
                Ok(exchange)
            }
        })
        .build()?;
    ctx.add_route_definition(update_route).await?;

    let s = Arc::clone(&store);
    let delete_route = RouteBuilder::from("direct:deleteUser")
        .route_id("handle-deleteUser")
        .process(move |mut exchange| {
            let s = Arc::clone(&s);
            async move {
                let id = exchange
                    .input
                    .header("CamelHttpPath_id")
                    .and_then(|v| match v {
                        camel_api::Value::String(s) => s.parse::<u64>().ok(),
                        camel_api::Value::Number(n) => n.as_u64(),
                        _ => None,
                    });
                match id.filter(|&id| s.delete(id)) {
                    Some(id) => {
                        exchange.input.body = camel_api::Body::Json(serde_json::json!({
                            "deleted": true,
                            "id": id,
                        }));
                    }
                    None => {
                        exchange
                            .input
                            .set_header("CamelHttpResponseCode", camel_api::Value::from(404u16));
                        exchange.input.body = camel_api::Body::Json(serde_json::json!({
                            "error": "user_not_found",
                            "message": "user not found",
                        }));
                    }
                }
                Ok(exchange)
            }
        })
        .build()?;
    ctx.add_route_definition(delete_route).await?;

    ctx.start().await?;
    tracing::info!("Secured REST CRUD example running on http://0.0.0.0:9090");

    print_banner();

    tokio::signal::ctrl_c()
        .await
        .map_err(|e| CamelError::Io(e.to_string()))?;
    tracing::info!("Shutting down");
    ctx.stop().await?;

    Ok(())
}

fn print_banner() {
    println!();
    println!("Endpoints (all require role '{REQUIRED_ROLE}' via provider '{PROVIDER}'):");
    println!("  GET    /api/users          list users (200)");
    println!("  POST   /api/users          create user (201)");
    println!("  PUT    /api/users/{{id}}    update user (200 / 404)");
    println!("  DELETE /api/users/{{id}}    delete user (204 / 404)");
    println!();
    println!(
        "  authorized : curl -H 'Authorization: Bearer {DEMO_TOKEN}' http://localhost:9090/api/users"
    ); // allow-secret: banner prints the demo token, never a real secret
    println!("  anonymous  : curl -i http://localhost:9090/api/users   (401)");
    println!();
}
