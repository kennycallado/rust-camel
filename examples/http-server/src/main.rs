mod errors;
mod models;
mod storage;

use camel_api::body::Body;
use camel_api::{CamelError, Value};
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_http::HttpComponent;
use camel_core::context::CamelContext;
use camel_core::route::RouteDefinition;
use camel_processor::LogLevel;
use errors::ApiError;
use models::{CreateUserRequest, UpdateUserRequest};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use storage::UserStorage;

#[tokio::main]
async fn main() -> Result<(), CamelError> {
    tracing_subscriber::fmt().with_target(false).init();

    let mut ctx = CamelContext::new();
    ctx.register_component(HttpComponent::new());

    let storage = Arc::new(UserStorage::new());
    let request_count = Arc::new(AtomicU64::new(0));

    let health_route = create_health_route(Arc::clone(&storage), Arc::clone(&request_count))?;
    let list_users_route =
        create_list_users_route(Arc::clone(&storage), Arc::clone(&request_count))?;
    let create_user_route = create_user_route(Arc::clone(&storage), Arc::clone(&request_count))?;
    let get_user_route = get_user_route(Arc::clone(&storage), Arc::clone(&request_count))?;
    let update_user_route = update_user_route(Arc::clone(&storage), Arc::clone(&request_count))?;
    let delete_user_route = delete_user_route(Arc::clone(&storage), Arc::clone(&request_count))?;

    ctx.add_route_definition(health_route).await?;
    ctx.add_route_definition(list_users_route).await?;
    ctx.add_route_definition(create_user_route).await?;
    ctx.add_route_definition(get_user_route).await?;
    ctx.add_route_definition(update_user_route).await?;
    ctx.add_route_definition(delete_user_route).await?;

    ctx.start().await?;

    print_banner();

    tokio::signal::ctrl_c()
        .await
        .map_err(|e| CamelError::Io(e.to_string()))?;

    println!("\nShutting down...");
    ctx.stop().await?;
    println!("Done.");

    Ok(())
}

fn create_health_route(
    storage: Arc<UserStorage>,
    request_count: Arc<AtomicU64>,
) -> Result<RouteDefinition, CamelError> {
    RouteBuilder::from("http://0.0.0.0:8080/health")
        .route_id("health-check")
        .to("log:health?showHeaders=true&showBody=true&showCorrelationId=true")
        .process(move |mut exchange| {
            let storage = Arc::clone(&storage);
            let rc = Arc::clone(&request_count);

            async move {
                let uptime = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);

                exchange.input.body = Body::Json(serde_json::json!({
                    "status": "UP",
                    "service": "rust-camel-api",
                    "version": "1.0.0",
                    "uptime_seconds": uptime,
                    "requests_processed": rc.load(Ordering::Relaxed),
                    "users_count": storage.count(),
                    "timestamp": current_timestamp(),
                    "correlation_id": exchange.correlation_id,
                }));
                Ok(exchange)
            }
        })
        .build()
}

fn create_list_users_route(
    storage: Arc<UserStorage>,
    request_count: Arc<AtomicU64>,
) -> Result<RouteDefinition, CamelError> {
    RouteBuilder::from("http://0.0.0.0:8080/api/users")
        .route_id("list-users")
        .process(move |mut exchange| {
            let storage = Arc::clone(&storage);
            let rc = Arc::clone(&request_count);
            async move {
                rc.fetch_add(1, Ordering::Relaxed);

                let query = exchange
                    .input
                    .header("CamelHttpQuery")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");

                let (page, per_page) = parse_pagination(query);

                let result = storage.list(page, per_page);
                exchange.input.body = Body::Json(serde_json::to_value(result).unwrap());
                Ok(exchange)
            }
        })
        .build()
}

fn create_user_route(
    storage: Arc<UserStorage>,
    request_count: Arc<AtomicU64>,
) -> Result<RouteDefinition, CamelError> {
    RouteBuilder::from("http://0.0.0.0:8080/api/users/create")
        .route_id("create-user")
        .log("Received create user request", LogLevel::Info)
        .process(move |mut exchange| {
            let storage = Arc::clone(&storage);
            let rc = Arc::clone(&request_count);
            async move {
                rc.fetch_add(1, Ordering::Relaxed);

                let body_str = exchange.input.body.as_text().unwrap_or("{}");

                let req: CreateUserRequest = match serde_json::from_str(body_str) {
                    Ok(r) => r,
                    Err(e) => {
                        let error = ApiError::bad_request(format!("Invalid JSON: {}", e));
                        exchange.input.body = Body::Json(serde_json::to_value(error).unwrap());
                        exchange
                            .input
                            .set_header("CamelHttpResponseCode", Value::from(400u16));
                        return Ok(exchange);
                    }
                };

                if let Err(e) = req.validate() {
                    let error = ApiError::bad_request(e);
                    exchange.input.body = Body::Json(serde_json::to_value(error).unwrap());
                    exchange
                        .input
                        .set_header("CamelHttpResponseCode", Value::from(400u16));
                    return Ok(exchange);
                }

                if storage.email_exists(&req.email) {
                    let error = ApiError::conflict("Email already exists");
                    exchange.input.body = Body::Json(serde_json::to_value(error).unwrap());
                    exchange
                        .input
                        .set_header("CamelHttpResponseCode", Value::from(409u16));
                    return Ok(exchange);
                }

                let user = storage.create(req);
                exchange.input.body = Body::Json(serde_json::to_value(user).unwrap());
                exchange
                    .input
                    .set_header("CamelHttpResponseCode", Value::from(201u16));

                Ok(exchange)
            }
        })
        .log("Create user request completed", LogLevel::Info)
        .build()
}

fn get_user_route(
    storage: Arc<UserStorage>,
    request_count: Arc<AtomicU64>,
) -> Result<RouteDefinition, CamelError> {
    RouteBuilder::from("http://0.0.0.0:8080/api/users/id")
        .route_id("get-user")
        .process(move |mut exchange| {
            let storage = Arc::clone(&storage);
            let rc = Arc::clone(&request_count);
            async move {
                rc.fetch_add(1, Ordering::Relaxed);

                let query = exchange
                    .input
                    .header("CamelHttpQuery")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");

                let user_id = parse_user_id(query);

                match storage.get(user_id) {
                    Some(user) => {
                        exchange.input.body = Body::Json(serde_json::to_value(user).unwrap());
                    }
                    None => {
                        let error = ApiError::not_found(format!("User {} not found", user_id));
                        exchange.input.body = Body::Json(serde_json::to_value(error).unwrap());
                        exchange
                            .input
                            .set_header("CamelHttpResponseCode", Value::from(404u16));
                    }
                }

                Ok(exchange)
            }
        })
        .build()
}

fn update_user_route(
    storage: Arc<UserStorage>,
    request_count: Arc<AtomicU64>,
) -> Result<RouteDefinition, CamelError> {
    RouteBuilder::from("http://0.0.0.0:8080/api/users/update")
        .route_id("update-user")
        .process(move |mut exchange| {
            let storage = Arc::clone(&storage);
            let rc = Arc::clone(&request_count);
            async move {
                rc.fetch_add(1, Ordering::Relaxed);

                let query = exchange
                    .input
                    .header("CamelHttpQuery")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");

                let user_id = parse_user_id(query);

                if storage.get(user_id).is_none() {
                    let error = ApiError::not_found(format!("User {} not found", user_id));
                    exchange.input.body = Body::Json(serde_json::to_value(error).unwrap());
                    exchange
                        .input
                        .set_header("CamelHttpResponseCode", Value::from(404u16));
                    return Ok(exchange);
                }

                let body_str = exchange.input.body.as_text().unwrap_or("{}");

                let req: UpdateUserRequest = match serde_json::from_str(body_str) {
                    Ok(r) => r,
                    Err(e) => {
                        let error = ApiError::bad_request(format!("Invalid JSON: {}", e));
                        exchange.input.body = Body::Json(serde_json::to_value(error).unwrap());
                        exchange
                            .input
                            .set_header("CamelHttpResponseCode", Value::from(400u16));
                        return Ok(exchange);
                    }
                };

                if let Err(e) = req.validate() {
                    let error = ApiError::bad_request(e);
                    exchange.input.body = Body::Json(serde_json::to_value(error).unwrap());
                    exchange
                        .input
                        .set_header("CamelHttpResponseCode", Value::from(400u16));
                    return Ok(exchange);
                }

                if !req.has_updates() {
                    let error = ApiError::bad_request("No fields to update");
                    exchange.input.body = Body::Json(serde_json::to_value(error).unwrap());
                    exchange
                        .input
                        .set_header("CamelHttpResponseCode", Value::from(400u16));
                    return Ok(exchange);
                }

                match storage.update(user_id, req) {
                    Some(user) => {
                        exchange.input.body = Body::Json(serde_json::to_value(user).unwrap());
                    }
                    None => {
                        let error = ApiError::internal("Failed to update user");
                        exchange.input.body = Body::Json(serde_json::to_value(error).unwrap());
                        exchange
                            .input
                            .set_header("CamelHttpResponseCode", Value::from(500u16));
                    }
                }

                Ok(exchange)
            }
        })
        .build()
}

fn delete_user_route(
    storage: Arc<UserStorage>,
    request_count: Arc<AtomicU64>,
) -> Result<RouteDefinition, CamelError> {
    RouteBuilder::from("http://0.0.0.0:8080/api/users/delete")
        .route_id("delete-user")
        .process(move |mut exchange| {
            let storage = Arc::clone(&storage);
            let rc = Arc::clone(&request_count);
            async move {
                rc.fetch_add(1, Ordering::Relaxed);

                let query = exchange
                    .input
                    .header("CamelHttpQuery")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");

                let user_id = parse_user_id(query);

                match storage.delete(user_id) {
                    Some(user) => {
                        exchange.input.body = Body::Json(serde_json::json!({
                            "message": "User deleted successfully",
                            "user": user,
                            "correlation_id": exchange.correlation_id,
                        }));
                    }
                    None => {
                        let error = ApiError::not_found(format!("User {} not found", user_id));
                        exchange.input.body = Body::Json(serde_json::to_value(error).unwrap());
                        exchange
                            .input
                            .set_header("CamelHttpResponseCode", Value::from(404u16));
                    }
                }

                Ok(exchange)
            }
        })
        .build()
}

fn parse_pagination(query: &str) -> (u32, u32) {
    let mut page = 1;
    let mut per_page = 10;

    for pair in query.split('&') {
        let parts: Vec<&str> = pair.splitn(2, '=').collect();
        if parts.len() == 2 {
            match parts[0] {
                "page" => page = parts[1].parse().unwrap_or(1).max(1),
                "per_page" => per_page = parts[1].parse().unwrap_or(10).clamp(1, 100),
                _ => {}
            }
        }
    }

    (page, per_page)
}

fn parse_user_id(query: &str) -> u64 {
    for pair in query.split('&') {
        let parts: Vec<&str> = pair.splitn(2, '=').collect();
        if parts.len() == 2 && parts[0] == "id" {
            return parts[1].parse().unwrap_or(0);
        }
    }
    0
}

fn current_timestamp() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn print_banner() {
    println!("╔════════════════════════════════════════════════════════════╗");
    println!("║       rust-camel HTTP Server - Realistic REST API          ║");
    println!("╚════════════════════════════════════════════════════════════╝");
    println!();
    println!("Server running on http://0.0.0.0:8080");
    println!();
    println!("API Endpoints:");
    println!();
    println!("  GET  /health");
    println!("       → Health check with metrics");
    println!();
    println!("  GET  /api/users?page=1&per_page=10");
    println!("       → List users with pagination");
    println!();
    println!("  POST /api/users/create");
    println!("       Body: {{\"name\":\"Alice\",\"email\":\"alice@example.com\",\"age\":30}}");
    println!("       → Create user (validates, checks email uniqueness)");
    println!();
    println!("  GET  /api/users/id?id=1");
    println!("       → Get user by ID");
    println!();
    println!("  PUT  /api/users/update?id=1");
    println!("       Body: {{\"name\":\"New Name\",\"age\":35}}");
    println!("       → Update user (partial updates supported)");
    println!();
    println!("  DELETE /api/users/delete?id=1");
    println!("       → Delete user by ID");
    println!();
    println!("────────────────────────────────────────────────────────────");
    println!("  Example workflow:");
    println!();
    println!("  # Create a user");
    println!("  curl -X POST http://localhost:8080/api/users/create \\");
    println!("       -H 'Content-Type: application/json' \\");
    println!("       -d '{{\"name\":\"Alice\",\"email\":\"alice@example.com\"}}' | jq");
    println!();
    println!("  # List users");
    println!("  curl 'http://localhost:8080/api/users?page=1&per_page=10' | jq");
    println!();
    println!("  # Get user");
    println!("  curl 'http://localhost:8080/api/users/id?id=1' | jq");
    println!();
    println!("  # Update user");
    println!("  curl -X PUT 'http://localhost:8080/api/users/update?id=1' \\");
    println!("       -H 'Content-Type: application/json' \\");
    println!("       -d '{{\"age\":35}}' | jq");
    println!();
    println!("  # Delete user");
    println!("  curl -X DELETE 'http://localhost:8080/api/users/delete?id=1' | jq");
    println!("────────────────────────────────────────────────────────────");
    println!();
    println!("Press Ctrl+C to stop...");
}
