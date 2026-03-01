# Log EIP vs Log Component Demo

This example demonstrates the difference between the **Log EIP** and the **Log Component** in rust-camel.

## Two Ways to Log

| Feature | Log EIP `.log("msg", LogLevel)` | Log Component `.to("log:...")` |
|---------|--------------------------------|-------------------------------|
| **Purpose** | Human-readable messages | Exchange inspection |
| **Output** | Just the string | Body, headers, properties |
| **Use case** | Status, progress, milestones | Debugging, tracing |
| **Performance** | Minimal overhead | Slightly more overhead |
| **Configuration** | Simple (message + level) | Rich options |

## Log EIP

Simple logging for human-readable messages with explicit log levels:

```rust
RouteBuilder::from("timer:demo")
    .log("Starting processing", LogLevel::Info)
    .log("Processing order", LogLevel::Info)
    .log("Done", LogLevel::Info)
```

**All log levels available:**

```rust
RouteBuilder::from("timer:demo")
    .log("Entering method", LogLevel::Trace)        // Most verbose
    .log("Variable x = 42", LogLevel::Debug)        // Debugging info
    .log("Order received", LogLevel::Info)          // Business events
    .log("Rate limit approaching", LogLevel::Warn)  // Warnings
    .log("Payment failed", LogLevel::Error)         // Errors
```

**Output:**
```
TRACE Entering method
DEBUG Variable x = 42
INFO Starting processing
INFO Processing order
WARN Rate limit approaching
ERROR Payment failed
INFO Done
```

## Log Component

Full exchange inspection with configurable output:

```rust
RouteBuilder::from("timer:demo")
    .to("log:exchange?showBody=true&showHeaders=true")
```

**Output:**
```
Exchange[Id: 123, Pattern: InOnly, 
  Headers: {source=timer, priority=high},
  Body: {"order_id": 12345, "items": [...]}
]
```

## Running

```bash
cargo run -p log-eip
```

## Example Output

```
INFO === Starting processing cycle ===
INFO exchange-full: Exchange[Id:ID-timer-demo-1, Pattern:InOnly, 
       Headers:{source=timer, priority=high}, 
       Body:{"order_id":12345,"items":["widget","gadget"],"total":99.99}]
INFO Order data prepared
INFO after-processing: Exchange[...]
DEBUG Processing high priority order
INFO === Processing cycle complete ===
```

## When to Use Each

### Use Log EIP (`.log()`) for:
- Route progress markers ("Starting...", "Done")
- Business milestones ("Order processed", "Payment received")
- Error context ("Validation failed")
- Simple status updates

### Use Log Component (`.to("log:...")`) for:
- Debugging exchange contents
- Inspecting headers during routing
- Viewing body transformations
- Development troubleshooting

## Log Component Options

```rust
.to("log:category?showBody=true&showHeaders=false&level=DEBUG&multiline=true")
```

| Option | Description |
|--------|-------------|
| `showBody=true` | Include message body |
| `showHeaders=true` | Include all headers |
| `showProperties=true` | Include exchange properties |
| `showAll=true` | Show everything |
| `level=DEBUG` | Set log level |
| `multiline=true` | Format across multiple lines |
| `maxChars=1000` | Limit output length |
