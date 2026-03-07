//! # Bean Demo Example
//!
//! This example demonstrates the bean/registry system in rust-camel.
//! It shows how to:
//! - Define beans with handlers using the #[bean_impl] and #[handler] attributes
//! - Register beans in a BeanRegistry
//! - Invoke bean methods directly
//! - Process messages through the bean system

use camel_api::body::Body;
use camel_api::{CamelError, Exchange, Message};
use camel_bean::{BeanRegistry, bean_impl, handler};
use camel_component_log::LogComponent;
use camel_core::context::CamelContext;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

// =============================================================================
// Data Structures
// =============================================================================

#[derive(Debug, Serialize, Deserialize)]
pub struct Order {
    pub id: String,
    pub customer: String,
    pub items: Vec<OrderItem>,
    pub total: f64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct OrderItem {
    pub product_id: String,
    pub name: String,
    pub quantity: u32,
    pub price: f64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ProcessedOrder {
    pub order: Order,
    pub status: String,
    pub processed_at: String,
    pub discount_applied: Option<f64>,
}

// =============================================================================
// OrderService Bean
// =============================================================================

pub struct OrderService;

#[bean_impl]
impl OrderService {
    /// Process an order and apply business logic
    #[handler]
    pub async fn process(&self, body: Order) -> Result<ProcessedOrder, String> {
        println!(
            "   Processing order {} for customer {}",
            body.id, body.customer
        );

        // Apply discount logic
        let discount_applied = if body.total > 1000.0 {
            Some(body.total * 0.1) // 10% discount for orders over $1000
        } else {
            None
        };

        let processed_order = ProcessedOrder {
            order: body,
            status: "PROCESSED".to_string(),
            processed_at: chrono::Utc::now().to_rfc3339(),
            discount_applied,
        };

        println!("   Order processed successfully");
        Ok(processed_order)
    }

    /// Validate an order and return validation results
    #[handler]
    pub async fn validate(&self, body: Order) -> Result<Order, String> {
        println!("   Validating order {}", body.id);

        let mut errors = Vec::new();

        // Basic validation
        if body.id.is_empty() {
            errors.push("Order ID is required".to_string());
        }

        if body.customer.is_empty() {
            errors.push("Customer name is required".to_string());
        }

        if body.items.is_empty() {
            errors.push("Order must contain at least one item".to_string());
        }

        // Business validation
        if body.total <= 0.0 {
            errors.push("Order total must be positive".to_string());
        }

        // Validate items
        for (index, item) in body.items.iter().enumerate() {
            if item.product_id.is_empty() {
                errors.push(format!("Item {} has no product ID", index + 1));
            }

            if item.quantity == 0 {
                errors.push(format!("Item {} has zero quantity", index + 1));
            }

            if item.price <= 0.0 {
                errors.push(format!("Item {} has invalid price", index + 1));
            }
        }

        if errors.is_empty() {
            println!("   Order validation passed");
            Ok(body)
        } else {
            println!("   Order validation failed with {} errors", errors.len());
            Err(format!("Validation failed: {}", errors.join(", ")))
        }
    }

    /// Get order statistics (demonstrates method without body parameter)
    #[handler]
    pub async fn get_stats(&self) -> Result<String, String> {
        println!("   Getting order stats");

        let stats =
            "Order Statistics: Total Orders: 1,234 | Pending: 45 | Completed: 1,189 | Revenue: $123,456.78".to_string();

        Ok(stats)
    }
}

/// Helper function to extract typed result from exchange body
fn extract_json<T: for<'de> Deserialize<'de>>(body: &Body) -> Result<T, String> {
    match body {
        Body::Json(value) => serde_json::from_value(value.clone())
            .map_err(|e| format!("Failed to deserialize: {}", e)),
        _ => Err("Expected JSON body".to_string()),
    }
}

// =============================================================================
// Main Application
// =============================================================================

#[tokio::main]
async fn main() -> Result<(), CamelError> {
    println!("=== Bean Demo Example ===");
    println!("This example demonstrates bean registration and invocation in rust-camel\n");

    // Create Camel context
    let mut ctx = CamelContext::new();
    ctx.register_component(LogComponent::new());

    // Create and configure bean registry
    let mut bean_registry = BeanRegistry::new();

    // Register our OrderService bean
    bean_registry.register("orderService", OrderService);

    println!("Registered {} beans:", bean_registry.len());
    println!("   - orderService (with handlers: process, validate, get_stats)\n");

    // Wrap the registry in an Arc for sharing
    let bean_registry = Arc::new(bean_registry);

    // Start the context
    ctx.start().await?;
    println!("Context started\n");

    // =========================================================================
    // Demo 1: Validate a valid order
    // =========================================================================
    println!("--- Demo 1: Order Validation ---");

    let valid_order = Order {
        id: "ORD-001".to_string(),
        customer: "Alice Smith".to_string(),
        items: vec![OrderItem {
            product_id: "PROD-001".to_string(),
            name: "Widget".to_string(),
            quantity: 2,
            price: 49.99,
        }],
        total: 99.98,
    };

    let mut exchange = Exchange::new(Message::default());
    exchange.input.body =
        Body::Json(serde_json::to_value(&valid_order).expect("Failed to serialize order"));

    match bean_registry
        .invoke("orderService", "validate", &mut exchange)
        .await
    {
        Ok(_) => {
            let result: Order =
                extract_json(&exchange.input.body).expect("Failed to deserialize result");
            println!("   [OK] Validation passed for order {}", result.id);
        }
        Err(e) => {
            println!("   [FAIL] Validation failed: {}", e);
        }
    }
    println!();

    // =========================================================================
    // Demo 2: Validate an invalid order
    // =========================================================================
    println!("--- Demo 2: Invalid Order Validation ---");

    let invalid_order = Order {
        id: "".to_string(),       // Missing ID
        customer: "".to_string(), // Missing customer
        items: vec![],            // No items
        total: -10.0,             // Negative total
    };

    let mut exchange = Exchange::new(Message::default());
    exchange.input.body =
        Body::Json(serde_json::to_value(&invalid_order).expect("Failed to serialize order"));

    match bean_registry
        .invoke("orderService", "validate", &mut exchange)
        .await
    {
        Ok(_) => {
            println!("   [WARN] Unexpected success");
        }
        Err(e) => {
            println!("   [EXPECTED FAIL] Validation failed: {}", e);
        }
    }
    println!();

    // =========================================================================
    // Demo 3: Process an order (with discount)
    // =========================================================================
    println!("--- Demo 3: Order Processing (High Value with Discount) ---");

    let high_value_order = Order {
        id: "ORD-002".to_string(),
        customer: "Bob Johnson".to_string(),
        items: vec![OrderItem {
            product_id: "PROD-100".to_string(),
            name: "Premium Widget".to_string(),
            quantity: 5,
            price: 299.99,
        }],
        total: 1499.95, // Over $1000, should get 10% discount
    };

    let mut exchange = Exchange::new(Message::default());
    exchange.input.body =
        Body::Json(serde_json::to_value(&high_value_order).expect("Failed to serialize order"));

    match bean_registry
        .invoke("orderService", "process", &mut exchange)
        .await
    {
        Ok(_) => {
            let result: ProcessedOrder =
                extract_json(&exchange.input.body).expect("Failed to deserialize result");
            println!("   [OK] Order {} processed", result.order.id);
            println!("   Customer: {}", result.order.customer);
            println!("   Total: ${:.2}", result.order.total);
            println!("   Status: {}", result.status);
            if let Some(discount) = result.discount_applied {
                println!("   Discount applied: ${:.2}", discount);
            }
            println!("   Processed at: {}", result.processed_at);
        }
        Err(e) => {
            println!("   [FAIL] Processing failed: {}", e);
        }
    }
    println!();

    // =========================================================================
    // Demo 4: Get order statistics
    // =========================================================================
    println!("--- Demo 4: Order Statistics ---");

    let mut exchange = Exchange::new(Message::default());
    match bean_registry
        .invoke("orderService", "get_stats", &mut exchange)
        .await
    {
        Ok(_) => {
            let stats: String =
                extract_json(&exchange.input.body).expect("Failed to deserialize stats");
            println!("   [OK] {}", stats);
        }
        Err(e) => {
            println!("   [FAIL] Failed to get stats: {}", e);
        }
    }
    println!();

    // =========================================================================
    // Demo 5: Process a regular order
    // =========================================================================
    println!("--- Demo 5: Regular Order Processing ---");

    let regular_order = Order {
        id: "ORD-003".to_string(),
        customer: "Charlie Brown".to_string(),
        items: vec![OrderItem {
            product_id: "PROD-050".to_string(),
            name: "Standard Gadget".to_string(),
            quantity: 1,
            price: 199.99,
        }],
        total: 199.99,
    };

    let mut exchange = Exchange::new(Message::default());
    exchange.input.body =
        Body::Json(serde_json::to_value(&regular_order).expect("Failed to serialize order"));

    match bean_registry
        .invoke("orderService", "process", &mut exchange)
        .await
    {
        Ok(_) => {
            let result: ProcessedOrder =
                extract_json(&exchange.input.body).expect("Failed to deserialize result");
            println!("   [OK] Order {} processed", result.order.id);
            println!("   Customer: {}", result.order.customer);
            println!("   Total: ${:.2}", result.order.total);
            println!("   Status: {}", result.status);
        }
        Err(e) => {
            println!("   [FAIL] Processing failed: {}", e);
        }
    }
    println!();

    // =========================================================================
    // Cleanup
    // =========================================================================
    println!("Stopping context...");
    ctx.stop().await?;

    println!("\n=== Demo Complete ===");
    println!("All bean operations completed successfully!");

    Ok(())
}
