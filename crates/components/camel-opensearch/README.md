# camel-component-opensearch

> OpenSearch component for rust-camel integration framework

## Overview

The OpenSearch component provides comprehensive OpenSearch integration for rust-camel, supporting **7 core operations** for document indexing, search, and management. It enables producer mode for executing OpenSearch operations.

## Features

- **7 Core Operations**: INDEX, SEARCH, GET, DELETE, UPDATE, BULK, MULTIGET
- **Producer Mode**: Execute OpenSearch operations and receive responses
- **Connection Options**: Host, port, username, password, TLS support
- **Async Native**: Built on `tokio` and async HTTP client
- **Global Configuration**: Configure defaults via Camel.toml
- **Health Check**: Async OpenSearch cluster health probe

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
camel-component-opensearch = "*"
```

## URI Format

```
opensearch://host:port/index?options
opensearchs://host:port/index?options  (TLS enabled)
```

## URI Options

| Option | Default | Description |
|--------|---------|-------------|
| `operation` | `SEARCH` | OpenSearch operation to perform |
| `username` | - | Username for authentication |
| `password` | - | Password for authentication |

## Supported Operations

| Operation | Description |
|-----------|-------------|
| `INDEX` | Index a document (create or replace) |
| `SEARCH` | Search documents using query |
| `GET` | Retrieve a document by ID |
| `DELETE` | Delete a document by ID |
| `UPDATE` | Update a document (partial update) |
| `BULK` | Bulk operations for multiple documents |
| `MULTIGET` | Retrieve multiple documents by IDs |

## Usage

### Index a Document

```rust
use camel_builder::RouteBuilder;
use camel_component_opensearch::OpenSearchComponent;

let mut ctx = CamelContext::new();
ctx.register_component("opensearch", Box::new(OpenSearchComponent::new()));

// INDEX: Store a document
let index_route = RouteBuilder::from("direct:index")
    .set_header("CamelOpenSearch.Id", Value::String("doc1".into()))
    .set_body(Body::Text(r#"{"name": "Alice", "age": 30}"#.into()))
    .to("opensearch://localhost:9200/users?operation=INDEX")
    .build()?;
```

### Search Documents

```rust
// SEARCH: Query documents
let search_route = RouteBuilder::from("direct:search")
    .set_body(Body::Text(r#"{"query": {"match": {"name": "Alice"}}}"#.into()))
    .to("opensearch://localhost:9200/users?operation=SEARCH")
    .build()?;
```

### Get a Document

```rust
// GET: Retrieve by ID
let get_route = RouteBuilder::from("direct:get")
    .set_header("CamelOpenSearch.Id", Value::String("doc1".into()))
    .to("opensearch://localhost:9200/users?operation=GET")
    .build()?;
```

### Delete a Document

```rust
// DELETE: Remove by ID
let delete_route = RouteBuilder::from("direct:delete")
    .set_header("CamelOpenSearch.Id", Value::String("doc1".into()))
    .to("opensearch://localhost:9200/users?operation=DELETE")
    .build()?;
```

### Update a Document

```rust
// UPDATE: Partial update
let update_route = RouteBuilder::from("direct:update")
    .set_header("CamelOpenSearch.Id", Value::String("doc1".into()))
    .set_body(Body::Text(r#"{"doc": {"age": 31}}"#.into()))
    .to("opensearch://localhost:9200/users?operation=UPDATE")
    .build()?;
```

### Bulk Operations

```rust
// BULK: Multiple operations (body is a JSON array of action+doc pairs)
let bulk_route = RouteBuilder::from("direct:bulk")
    .set_body(serde_json::json!([
        {"index": {"_index": "users", "_id": "1"}},
        {"name": "Alice", "age": 30},
        {"index": {"_index": "users", "_id": "2"}},
        {"name": "Bob", "age": 25}
    ]))
    .to("opensearch://localhost:9200/users?operation=BULK")
    .build()?;
```

### Multi-Get

```rust
// MULTIGET: Retrieve multiple documents
let multiget_route = RouteBuilder::from("direct:multiget")
    .set_body(Body::Text(r#"{"ids": ["1", "2", "3"]}"#.into()))
    .to("opensearch://localhost:9200/users?operation=MULTIGET")
    .build()?;
```

### With Authentication

#### Basic Auth (Username/Password)

```rust
let auth_route = RouteBuilder::from("direct:auth")
    .to("opensearch://localhost:9200/secure-data?operation=GET&username=admin&password=secret")
    .build()?;
```

#### API Key Auth

```rust
// Use the opensearchs:// scheme (HTTPS) with API key credentials
let api_key_route = RouteBuilder::from("direct:apikey")
    .to("opensearchs://opensearch.example.com:9200/secure-index?operation=SEARCH&username=${OPENSEARCH_API_KEY_ID}&password=${OPENSEARCH_API_KEY_SECRET}")
    .build()?;
```

#### Global Auth Configuration

```rust
use camel_component_opensearch::{OpenSearchComponent, OpenSearchConfig, OpenSearchOperation};

// Set global authentication defaults
let global = OpenSearchConfig::default()
    .with_host("opensearch.example.com")
    .with_port(9200)
    .with_username("admin")
    .with_password("secret");

let component = OpenSearchComponent::with_config(global);
```

### With TLS

```rust
// Use opensearchs:// for HTTPS
let tls_route = RouteBuilder::from("direct:tls")
    .to("opensearchs://opensearch.example.com:9200/secure-index?operation=SEARCH")
    .build()?;
```

## Exchange Headers

| Header | Direction | Description |
|--------|-----------|-------------|
| `CamelOpenSearch.Id` | Input | Document ID (for INDEX, GET, DELETE, UPDATE) |
| `CamelOpenSearch.Operation` | Input | Operation to perform (overrides URI operation) |

## Example: Document Indexing Pipeline

```rust
use camel_builder::RouteBuilder;
use camel_component_opensearch::OpenSearchComponent;
use camel_core::context::CamelContext;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut ctx = CamelContext::builder().build().await?;
    ctx.register_component("opensearch", Box::new(OpenSearchComponent::new()));

    // Index user data
    let route = RouteBuilder::from("direct:index-user")
        .process(|ex| async move {
            // Transform user data to OpenSearch document
            let user: User = serde_json::from_str(ex.input.body.as_text().unwrap_or("{}"))?;
            let doc = serde_json::to_string(&user)?;
            let mut ex = ex;
            ex.input.body = Body::Text(doc);
            ex.input.headers.insert(
                "CamelOpenSearch.Id".into(),
                Value::String(format!("user-{}", user.id))
            );
            Ok(ex)
        })
        .to("opensearch://localhost:9200/users?operation=INDEX")
        .build()?;

    ctx.add_route(route).await?;
    ctx.start().await?;

    Ok(())
}
```

## Example: Search and Process

```rust
// Search and process results
let route = RouteBuilder::from("direct:search-users")
    .set_body(Body::Text(r#"{"query": {"range": {"age": {"gte": 18}}}}"#.into()))
    .to("opensearch://localhost:9200/users?operation=SEARCH")
    .process(|ex| async move {
        // Process search results
        let response: serde_json::Value = serde_json::from_str(ex.input.body.as_text().unwrap_or("{}"))?;
        if let Some(hits) = response["hits"]["hits"].as_array() {
            println!("Found {} users", hits.len());
            for hit in hits {
                println!("User: {}", hit["_source"]);
            }
        }
        Ok(ex)
    })
    .build()?;
```

## Global Configuration

Configure default OpenSearch connection settings in `Camel.toml` that apply to all OpenSearch endpoints:

```toml
[default.components.opensearch]
host = "localhost"       # OpenSearch host (default: localhost)
port = 9200              # OpenSearch port (default: 9200)
username = "admin"       # Optional: default username
password = "secret"      # Optional: default password
default_operation = "SEARCH"  # Optional: default operation
index_name = "default"   # Optional: default index name
```

URI parameters always override global defaults:

```rust
// Uses global host/port (localhost:9200)
.to("opensearch:///myindex?operation=GET")

// Overrides port from global config
.to("opensearch://:9300/myindex?operation=GET")

// Full override with different host
.to("opensearch://opensearch-prod:9200/myindex?operation=GET")

// Override username/password from global config
.to("opensearch:///secure-data?username=readonly&password=public")
```

### Profile-Specific Configuration

```toml
[default.components.opensearch]
host = "localhost"
port = 9200

[production.components.opensearch]
host = "opensearch-prod.internal"
port = 9200
username = "app-user"
password = "${OPENSEARCH_PASSWORD}"
```

## Health Check

The `opensearch` component registers an async health check via `AsyncHealthCheck`.

- **Probe**: OpenSearch `cluster.health()` API call
- **Healthy**: Cluster status is `green` or `yellow`
- **Unhealthy**: Cluster status is `red`, request fails, or probe times out (10s default)

Health checks are exposed via the health server:

```toml
[observability.health]
enabled = true
port = 8080
```

## Documentation

- [API Documentation](https://docs.rs/camel-component-opensearch)
- [Repository](https://github.com/kennycallado/rust-camel)

## License

Apache-2.0

## Contributing

Contributions are welcome! Please see the [main repository](https://github.com/kennycallado/rust-camel) for details.
