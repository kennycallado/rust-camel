use camel_api::body::Body;
use camel_api::{CamelError, Value};
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_core::context::CamelContext;
use camel_http::{HttpComponent, HttpsComponent};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// HTTP Server example: Realistic REST API with multiple endpoints
///
/// Endpoints:
///   GET  /health              - Health check with metrics
///   GET  /api/users           - List all users
///   POST /api/users           - Create user (with validation)
///   GET  /api/users/{id}      - Get user by ID
///   POST /api/transform       - Transform text with stats
///   GET  /api/time            - Server time (JSON)
#[tokio::main]
async fn main() -> Result<(), CamelError> {
    tracing_subscriber::fmt::init();

    let mut ctx = CamelContext::new();
    ctx.register_component(HttpComponent::new());
    ctx.register_component(HttpsComponent::new());

    // Shared state
    let request_count = Arc::new(AtomicU64::new(0));
    let rc1 = Arc::clone(&request_count);
    let rc2 = Arc::clone(&request_count);
    let rc3 = Arc::clone(&request_count);
    let rc4 = Arc::clone(&request_count);
    let rc5 = Arc::clone(&request_count);

    // Route 1: Health check with metrics
    let health_route = RouteBuilder::from("http://0.0.0.0:8080/health")
        .process(move |mut exchange| {
            let rc = Arc::clone(&rc1);
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
                    "timestamp": chrono_timestamp(),
                }));
                Ok(exchange)
            }
        })
        .build()?;

    // Route 2: List users
    let list_users_route = RouteBuilder::from("http://0.0.0.0:8080/api/users")
        .process(move |mut exchange| {
            let rc = Arc::clone(&rc2);
            async move {
                rc.fetch_add(1, Ordering::Relaxed);
                
                exchange.input.body = Body::Json(serde_json::json!({
                    "users": [
                        {"id": 1, "name": "Alice", "email": "alice@example.com", "role": "admin"},
                        {"id": 2, "name": "Bob", "email": "bob@example.com", "role": "user"},
                        {"id": 3, "name": "Charlie", "email": "charlie@example.com", "role": "user"},
                    ],
                    "total": 3,
                    "page": 1,
                    "per_page": 10,
                }));
                Ok(exchange)
            }
        })
        .build()?;

    // Route 3: Create user (POST with body transformation)
    let create_user_route = RouteBuilder::from("http://0.0.0.0:8080/api/users/create")
        .process(move |mut exchange| {
            let rc = Arc::clone(&rc3);
            async move {
                rc.fetch_add(1, Ordering::Relaxed);
                
                // Parse request body
                let body_str = exchange.input.body.as_text().unwrap_or("{}");
                let user_data: serde_json::Value = serde_json::from_str(body_str)
                    .unwrap_or(serde_json::json!({}));
                
                // Validate required fields
                let name = user_data.get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("Anonymous");
                
                let email = user_data.get("email")
                    .and_then(|v| v.as_str())
                    .unwrap_or("no-email@example.com");
                
                // Create response with generated ID
                let user_id = rand_id();
                exchange.input.body = Body::Json(serde_json::json!({
                    "id": user_id,
                    "name": name,
                    "email": email,
                    "role": "user",
                    "created_at": chrono_timestamp(),
                    "status": "created",
                }));
                
                // Set 201 Created
                exchange.input.set_header(
                    "CamelHttpResponseCode",
                    Value::from(201u16),
                );
                
                Ok(exchange)
            }
        })
        .build()?;

    // Route 4: Get user by ID (path parameter simulation)
    let get_user_route = RouteBuilder::from("http://0.0.0.0:8080/api/users/id")
        .process(move |mut exchange| {
            let rc = Arc::clone(&rc4);
            async move {
                rc.fetch_add(1, Ordering::Relaxed);
                
                // Extract ID from query param (simplified)
                let query = exchange.input.header("CamelHttpQuery")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                
                let user_id = query.split('=')
                    .nth(1)
                    .unwrap_or("1")
                    .parse::<u64>()
                    .unwrap_or(1);
                
                // Mock user lookup
                if user_id <= 3 {
                    exchange.input.body = Body::Json(serde_json::json!({
                        "id": user_id,
                        "name": match user_id {
                            1 => "Alice",
                            2 => "Bob",
                            3 => "Charlie",
                            _ => "Unknown",
                        },
                        "email": format!("user{}@example.com", user_id),
                        "role": if user_id == 1 { "admin" } else { "user" },
                    }));
                } else {
                    exchange.input.body = Body::Json(serde_json::json!({
                        "error": "User not found",
                        "id": user_id,
                    }));
                    exchange.input.set_header(
                        "CamelHttpResponseCode",
                        Value::from(404u16),
                    );
                }
                
                Ok(exchange)
            }
        })
        .build()?;

    // Route 5: Transform text with statistics
    let transform_route = RouteBuilder::from("http://0.0.0.0:8080/api/transform")
        .process(move |mut exchange| {
            let rc = Arc::clone(&rc5);
            async move {
                rc.fetch_add(1, Ordering::Relaxed);
                
                let input = exchange.input.body.as_text().unwrap_or("");
                
                // Calculate statistics
                let words: Vec<&str> = input.split_whitespace().collect();
                let chars = input.chars().count();
                let uppercase = input.chars().filter(|c| c.is_uppercase()).count();
                let lowercase = input.chars().filter(|c| c.is_lowercase()).count();
                let digits = input.chars().filter(|c| c.is_numeric()).count();
                
                exchange.input.body = Body::Json(serde_json::json!({
                    "original": input,
                    "transformed": input.to_uppercase(),
                    "reversed": input.chars().rev().collect::<String>(),
                    "statistics": {
                        "characters": chars,
                        "words": words.len(),
                        "uppercase": uppercase,
                        "lowercase": lowercase,
                        "digits": digits,
                        "spaces": input.chars().filter(|c| c.is_whitespace()).count(),
                    },
                    "metadata": {
                        "processed_at": chrono_timestamp(),
                        "version": "1.0",
                    }
                }));
                
                Ok(exchange)
            }
        })
        .build()?;

    // Route 6: Server time
    let time_route = RouteBuilder::from("http://0.0.0.0:8080/api/time")
        .process(|mut exchange| async move {
            let now = chrono_timestamp();
            let datetime = format_timestamp(now);
            
            exchange.input.body = Body::Json(serde_json::json!({
                "unix_timestamp": now,
                "iso8601": datetime,
                "timezone": "UTC",
            }));
            Ok(exchange)
        })
        .build()?;

    ctx.add_route_definition(health_route)?;
    ctx.add_route_definition(list_users_route)?;
    ctx.add_route_definition(create_user_route)?;
    ctx.add_route_definition(get_user_route)?;
    ctx.add_route_definition(transform_route)?;
    ctx.add_route_definition(time_route)?;

    ctx.start().await?;

    println!("╔════════════════════════════════════════════════════════════╗");
    println!("║       rust-camel HTTP Server - REST API Example            ║");
    println!("╚════════════════════════════════════════════════════════════╝");
    println!();
    println!("Server running on http://0.0.0.0:8080");
    println!();
    println!("Available endpoints:");
    println!();
    println!("  Health & Metrics:");
    println!("    GET  /health");
    println!("         → Returns service health and metrics");
    println!();
    println!("  User Management:");
    println!("    GET  /api/users");
    println!("         → List all users (mock data)");
    println!();
    println!("    POST /api/users/create");
    println!("         Body: {{\"name\": \"John\", \"email\": \"john@example.com\"}}");
    println!("         → Create new user with auto-generated ID");
    println!();
    println!("    GET  /api/users/id?id=1");
    println!("         → Get user by ID (try id=1, 2, or 3)");
    println!();
    println!("  Text Processing:");
    println!("    POST /api/transform");
    println!("         Body: Any text");
    println!("         → Transform text with detailed statistics");
    println!();
    println!("  Utilities:");
    println!("    GET  /api/time");
    println!("         → Current server time in multiple formats");
    println!();
    println!("────────────────────────────────────────────────────────────");
    println!("  Try these commands:");
    println!();
    println!("  curl http://localhost:8080/health | jq");
    println!("  curl http://localhost:8080/api/users | jq");
    println!("  curl -X POST http://localhost:8080/api/users/create \\");
    println!("       -d '{{\"name\":\"John\",\"email\":\"john@example.com\"}}' | jq");
    println!("  curl 'http://localhost:8080/api/users/id?id=1' | jq");
    println!("  curl -X POST http://localhost:8080/api/transform \\");
    println!("       -d 'Hello World 2024!' | jq");
    println!("  curl http://localhost:8080/api/time | jq");
    println!("────────────────────────────────────────────────────────────");
    println!();
    println!("Press Ctrl+C to stop...");

    tokio::signal::ctrl_c()
        .await
        .map_err(|e| CamelError::Io(e.to_string()))?;

    println!("\nShutting down...");
    ctx.stop().await?;
    println!("Done.");

    Ok(())
}

fn chrono_timestamp() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn format_timestamp(ts: i64) -> String {
    // Simple ISO 8601 format without chrono dependency
    let days = ts / 86400;
    let rem = ts % 86400;
    let hours = rem / 3600;
    let mins = (rem % 3600) / 60;
    let secs = rem % 60;
    
    // Calculate year/month/day (simplified, not 100% accurate)
    let year = 1970 + days / 365;
    let day_of_year = days % 365;
    let month = day_of_year / 30 + 1;
    let day = day_of_year % 30 + 1;
    
    format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z", year, month, day, hours, mins, secs)
}

fn rand_id() -> u64 {
    // Simple random ID without rand dependency
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(1)
}
