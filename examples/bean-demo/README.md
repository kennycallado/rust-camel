# Bean Demo Example

This example demonstrates the bean/registry system in rust-camel, showing how to define, register, and invoke beans in integration routes.

## What This Example Demonstrates

- **Bean Definition**: Creating a service with `#[bean_impl]` and `#[handler]` attributes
- **Bean Registration**: Using `BeanRegistry` to register and manage beans
- **Method Invocation**: Calling bean methods from Camel routes
- **Parameter Handling**: Different types of method parameters (body, headers)
- **Return Value Processing**: Handling different return types from bean methods

## Key Concepts

### Bean Definition

Beans are defined using the `#[bean_impl]` attribute on an `impl` block, with individual methods marked as handlers using `#[handler]`:

```rust
#[bean_impl]
impl OrderService {
    #[handler]
    pub async fn process(&self, order: Order) -> Result<ProcessedOrder, String> {
        // Business logic here
    }
    
    #[handler]
    pub async fn validate(&self, order: Order) -> Result<ValidationResult, String> {
        // Validation logic here
    }
    
    #[handler]
    pub async fn get_stats(&self, headers: Value) -> Result<String, String> {
        // Statistics logic here
    }
}
```

### Bean Registration

Beans are registered in a `BeanRegistry`:

```rust
let mut bean_registry = BeanRegistry::new();
bean_registry.register("orderService", OrderService);
```

### Route Integration

Bean methods are invoked from routes using the registry:

```rust
.process(move |mut exchange| {
    let registry = bean_registry.clone();
    async move {
        // Validate the order first
        registry.invoke("orderService", "validate", &mut exchange).await?;
        
        // Then process the order
        registry.invoke("orderService", "process", &mut exchange).await?;
        
        Ok(exchange)
    }
})
```

## Running the Example

```bash
cargo run --example bean-demo
```

## Expected Output

When you run the example, you'll see:

1. **Startup messages** showing bean registration
2. **Timer-triggered order processing** every 3 seconds (3 times)
3. **Order validation** with detailed results
4. **Order processing** with business logic (discounts, timestamps)
5. **Statistics requests** every 5 seconds (2 times)

Sample output:
```
🚀 Starting Bean Demo Example
📋 This example demonstrates bean registration and invocation in rust-camel

📝 Registered 1 beans:
   - orderService (with handlers: process, validate, get_stats)

🌟 Bean demo is running!
📊 Watch for order processing and statistics in the logs
⏹️  Press Ctrl+C to stop

🔄 Processing order ORD-1646675123 for customer Demo Customer
✅ Order validation passed
✅ Order processed successfully
📊 Getting order stats
📈 Stats: Order Statistics: Total Orders: 1,234 | Pending: 45 | Completed: 1,189 | Revenue: $123,456.78
```

## Implementation Details

### OrderService Bean

The example includes a realistic `OrderService` with three handlers:

1. **`process`**: Applies business logic to orders (discounts, timestamps)
2. **`validate`**: Validates order data and returns validation results
3. **`get_stats`**: Demonstrates header parameter handling

### Data Structures

The example uses realistic data structures with serde serialization:
- `Order`: Represents a customer order
- `OrderItem`: Items within an order
- `ProcessedOrder`: Order with processing metadata
- `ValidationResult`: Validation output with errors and warnings

### Route Structure

Two routes demonstrate different patterns:

1. **Order Processing Route**: Sequential bean method calls (validate → process)
2. **Statistics Route**: Simple bean method call with headers

## Learning Points

1. **Bean Isolation**: Each bean is independent and can be tested separately
2. **Type Safety**: Strong typing ensures data consistency
3. **Error Handling**: Results are properly propagated through the Camel system
4. **Flexibility**: Beans can have different parameter and return types
5. **Integration**: Beans integrate seamlessly with existing Camel components

## Extending the Example

You can extend this example by:

1. Adding more beans (PaymentService, InventoryService)
2. Creating more complex routes with conditional logic
3. Adding database persistence
4. Implementing error handling strategies
5. Adding metrics and monitoring

## Related Components

- `camel-bean`: Core bean functionality
- `camel-bean-macros`: Macros for bean definition
- `camel-api`: Exchange and message handling
- `camel-builder`: Route building DSL