//! `rest-crud` — runnable example for the Phase-1 REST DSL.
//!
//! The `rest:` YAML block is parsed by `camel_dsl::yaml::parse_yaml`, which
//! lowers each `operations.*` entry into an `http:` consumer with
//! `set_header(CamelHttpResponseCode, default)` + `unmarshal(json)` (for body
//! verbs) + `to(direct:...)` + `marshal(json)` + `set_header(Content-Type)`.
//! The example registers handler routes that consume from those `direct:`
//! endpoints and perform CRUD against an in-memory DashMap. A static mount
//! co-hosts on the same port (ADR-0009) so the API and a static index share
//! one listener.

use std::sync::Arc;

use camel_api::{Body, Value};
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_direct::DirectComponent;
use camel_component_http::{HttpComponent, HttpStaticComponent};
use camel_core::context::CamelContext;
use camel_dsl::yaml::parse_yaml;

mod storage;
use storage::{CreateUserRequest, UserStore};

const REST_YAML: &str = r#"
rest:
  - host: 0.0.0.0
    port: 9090
    path: /api/users
    operations:
      get:
        operation_id: listUsers
        to: direct:listUsers
        produces: application/json
      post:
        operation_id: createUser
        consumes: application/json
        produces: application/json
        success_status: 201
        to: direct:createUser
      put:
        path: /{id}
        operation_id: updateUser
        consumes: application/json
        produces: application/json
        to: direct:updateUser
      delete:
        path: /{id}
        operation_id: deleteUser
        to: direct:deleteUser
        success_status: 204
"#;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt().with_target(false).init();

    let store = Arc::new(UserStore::new());

    // 1. Parse REST YAML → RouteDefinitions.
    //    `rest:` blocks expand into `http:` routes whose `to:` step targets
    //    a `direct:` endpoint — we register handler routes for each one.
    let rest_routes =
        parse_yaml(REST_YAML).map_err(|e| format!("Failed to parse REST YAML: {e}"))?;

    tracing::info!("Parsed {} REST routes from YAML", rest_routes.len());
    for route in &rest_routes {
        tracing::info!("  Route: {} from {}", route.route_id(), route.from_uri());
    }

    // 2. Build context + register components.
    let mut ctx = CamelContext::builder().build().await.unwrap(); // allow-unwrap
    ctx.register_component(HttpComponent::new());
    ctx.register_component(HttpStaticComponent::new());
    ctx.register_component(DirectComponent::new());

    // 3. Register REST routes (http: consumers lowered from rest: YAML).
    for route in rest_routes {
        ctx.add_route_definition(route).await?;
    }

    // 4. Register handler routes (direct: consumers with CRUD logic).

    // GET /api/users — list all users.
    let s = Arc::clone(&store);
    let list_route = RouteBuilder::from("direct:listUsers")
        .route_id("handle-listUsers")
        .process(move |mut exchange| {
            let s = Arc::clone(&s);
            async move {
                let users = s.list();
                let json = serde_json::to_value(&users).unwrap_or(serde_json::Value::Null); // allow-unwrap
                exchange.input.body = Body::Json(json);
                Ok(exchange)
            }
        })
        .build()?;
    ctx.add_route_definition(list_route).await?;

    // POST /api/users — create a user. Default status is 201 from REST lowering.
    let s = Arc::clone(&store);
    let create_route = RouteBuilder::from("direct:createUser")
        .route_id("handle-createUser")
        .process(move |mut exchange| {
            let s = Arc::clone(&s);
            async move {
                let body = exchange.input.body.clone();
                if let Body::Json(value) = body {
                    match serde_json::from_value::<CreateUserRequest>(value) {
                        Ok(req) => {
                            let user = s.create(req.name, req.email);
                            let json =
                                serde_json::to_value(&user).unwrap_or(serde_json::Value::Null); // allow-unwrap
                            exchange.input.body = Body::Json(json);
                        }
                        Err(e) => {
                            // Bad request — override the 201 default.
                            exchange
                                .input
                                .set_header("CamelHttpResponseCode", Value::from(400u16));
                            exchange.input.body = Body::Json(serde_json::json!({
                                "error": "invalid_request",
                                "message": e.to_string(),
                            }));
                        }
                    }
                } else {
                    // Body not parsed as JSON (Unmarshal failed upstream or
                    // request had no body). 400.
                    exchange
                        .input
                        .set_header("CamelHttpResponseCode", Value::from(400u16));
                    exchange.input.body = Body::Json(serde_json::json!({
                        "error": "invalid_request",
                        "message": "expected application/json body",
                    }));
                }
                Ok(exchange)
            }
        })
        .build()?;
    ctx.add_route_definition(create_route).await?;

    // DELETE /api/users/{id} — default status 204 from REST lowering.
    let s = Arc::clone(&store);
    let delete_route = RouteBuilder::from("direct:deleteUser")
        .route_id("handle-deleteUser")
        .process(move |mut exchange| {
            let s = Arc::clone(&s);
            async move {
                // Path param `id` injected by the HTTP consumer as
                // `CamelHttpPath_id` (Phase-1 expert guidance E2).
                let id = exchange.input.header("CamelHttpPath_id").and_then(|v| {
                    let s = match v {
                        Value::String(s) => s.clone(),
                        Value::Number(n) => n.to_string(),
                        _ => return None,
                    };
                    s.parse::<u64>().ok()
                });

                match id.and_then(|id| if s.delete(id) { Some(id) } else { None }) {
                    Some(id) => {
                        // Marshal needs a non-Stream body — emit a small
                        // confirmation envelope. 204 status stays from the
                        // default SetHeader at the head of the pipeline.
                        exchange.input.body = Body::Json(serde_json::json!({
                            "deleted": true,
                            "id": id,
                        }));
                    }
                    None => {
                        exchange
                            .input
                            .set_header("CamelHttpResponseCode", Value::from(404u16));
                        exchange.input.body = Body::Json(serde_json::json!({
                            "error": "user_not_found",
                            "message": format!("user {} not found", id.unwrap_or(0)),
                        }));
                    }
                }
                Ok(exchange)
            }
        })
        .build()?;
    ctx.add_route_definition(delete_route).await?;

    // PUT /api/users/{id} — update user. Default status 200 from REST lowering.
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
                        Value::String(s) => s.parse::<u64>().ok(),
                        Value::Number(n) => n.as_u64(),
                        _ => None,
                    });

                let body = exchange.input.body.clone();
                let req: Result<CreateUserRequest, _> = match body {
                    Body::Json(value) => serde_json::from_value(value),
                    _ => Err(serde_json::from_str::<CreateUserRequest>("").unwrap_err()),
                };

                match (id, req) {
                    (Some(id), Ok(req)) => match s.update(id, req.name, req.email) {
                        Some(updated) => {
                            let json =
                                serde_json::to_value(&updated).unwrap_or(serde_json::Value::Null); // allow-unwrap
                            exchange.input.body = Body::Json(json);
                        }
                        None => {
                            exchange
                                .input
                                .set_header("CamelHttpResponseCode", Value::from(404u16));
                            exchange.input.body = Body::Json(serde_json::json!({
                                "error": "user_not_found",
                                "message": format!("user {} not found", id),
                            }));
                        }
                    },
                    (None, _) => {
                        exchange
                            .input
                            .set_header("CamelHttpResponseCode", Value::from(400u16));
                        exchange.input.body = Body::Json(serde_json::json!({
                            "error": "invalid_request",
                            "message": "missing or non-numeric id",
                        }));
                    }
                    (_, Err(e)) => {
                        exchange
                            .input
                            .set_header("CamelHttpResponseCode", Value::from(400u16));
                        exchange.input.body = Body::Json(serde_json::json!({
                            "error": "invalid_request",
                            "message": e.to_string(),
                        }));
                    }
                }
                Ok(exchange)
            }
        })
        .build()?;
    ctx.add_route_definition(update_route).await?;

    // 5. Static mount co-hosting (ADR-0009). The `http-static:` route shares
    //    port 9090 with the REST API on the same listener. The dispatch
    //    precedence (API exact → static prefix → fallback) is defined in
    //    `HttpRouteRegistry` and validated by the spec §7.2 contract.
    let static_route = RouteBuilder::from(
        "http-static:/static?dir=examples/rest-crud/static&port=9090&host=0.0.0.0",
    )
    .route_id("static-assets")
    .build()?;
    ctx.add_route_definition(static_route).await?;

    // 6. Start.
    ctx.start().await?;
    tracing::info!("REST CRUD example running on http://0.0.0.0:9090");
    tracing::info!("Try: curl http://localhost:9090/api/users");

    print_banner();

    tokio::signal::ctrl_c().await?;
    tracing::info!("Shutting down");
    ctx.stop().await?;

    Ok(())
}

fn print_banner() {
    println!();
    println!("Endpoints:");
    println!("  GET    /api/users          list users (200)");
    println!("  POST   /api/users          create user (201)");
    println!("  PUT    /api/users/{{id}}    update user (200 / 404)");
    println!("  DELETE /api/users/{{id}}    delete user (204 / 404)");
    println!("  GET    /static/index.html  static mount (200)");
    println!();
}
