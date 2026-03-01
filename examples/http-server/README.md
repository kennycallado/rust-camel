# HTTP Server Example

A complete HTTP server example demonstrating multiple routes sharing a single port.

## Routes

- **POST /echo** - Echo server that returns the request body unchanged
- **GET /api/status** - JSON API returning health status
- **GET /proxy** - Reverse proxy to httpbin.org
- **POST /transform** - Transforms text to uppercase JSON

## Running

```bash
cargo run -p http-server
```

## Testing

```bash
# Echo
curl -X POST http://localhost:8080/echo -d "hello world"

# Health check
curl http://localhost:8080/api/status

# Proxy
curl http://localhost:8080/proxy

# Transform
curl -X POST http://localhost:8080/transform -d "hello"
```

## Architecture

This example demonstrates:

- Multiple HTTP consumers on the same port (via `ServerRegistry`)
- HTTP consumer (inbound) + producer (outbound)
- Exchange transformation
- Camel HTTP headers (`CamelHttpMethod`, `CamelHttpResponseCode`)
- JSON processing
