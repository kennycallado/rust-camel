# camel-xj

> XJ (XML ↔ JSON) transformation component for rust-camel, powered by xml-bridge

## Overview

The `camel-xj` component provides bidirectional XML↔JSON transformation using XSLT stylesheets executed by an external `xml-bridge` process. The bridge communicates over gRPC and handles the actual XSLT 3.0 transformation.

The component bundles built-in identity stylesheets for both directions, so you can convert between XML and JSON out of the box.

## Features

- **XML → JSON**: Converts arbitrary XML to JSON using recursive XSLT 3.0 templates
- **JSON → XML**: Converts JSON to a structured XML representation using `fn:json-to-xml()`
- **Custom stylesheets**: Supply your own XSLT for domain-specific transformations
- **Automatic retries**: Transient transport errors trigger bridge restart with configurable retry
- **Payload size limits**: Reject oversized payloads before sending to the bridge process
- **Graceful lifecycle**: Bridge process is cleaned up when the Camel context stops
- **Health Check**: Async gRPC health probe for the xml-bridge sidecar

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
camel-xj = { workspace = true }
```

## URI Format

```
xj:<stylesheet-uri>?direction=<direction>[&options]
```

### Required Parameters

| Parameter   | Description                                    | Values              |
|-------------|------------------------------------------------|---------------------|
| `direction` | Transform direction (required)                  | `xml2json`, `json2xml` |

### Optional Parameters

| Parameter            | Description                              | Default |
|----------------------|------------------------------------------|---------|
| `transformDirection` | Override: `XML2JSON` or `JSON2XML`       | auto    |
| `resourceUri`        | Additional XSLT resource URI             | none    |
| `maxPayloadBytes`    | Max payload size in bytes                | unlimited |
| `retryCount`         | Number of retries on transient error     | 3       |
| `retryDelayMs`       | Delay between retries in milliseconds    | 500     |
| `param.*`            | Custom XSLT parameters (e.g. `param.foo=bar`) | none |

### Built-in Stylesheet URI

- `classpath:identity` — Uses the bundled identity stylesheet for the given direction

### File-based Stylesheet URI

- `file:///path/to/stylesheet.xslt` — Loads from the local filesystem

## Usage

### XML to JSON

```rust,ignore
use camel_builder::RouteBuilder;

let route = RouteBuilder::from("timer:tick?period=1000")
    .set_body(camel_component_api::Body::Xml("<root><name>Camel</name></root>".to_string()))
    .to("xj:classpath:identity?direction=xml2json")
    .log("JSON output: ${body}", camel_processor::LogLevel::Info)
    .build()?;
```

### JSON to XML

```rust,ignore
let route = RouteBuilder::from("timer:tick?period=1000")
    .set_body(camel_component_api::Body::Text(r#"{"name":"Camel"}"#.to_string()))
    .to("xj:classpath:identity?direction=json2xml")
    .log("XML output: ${body}", camel_processor::LogLevel::Info)
    .build()?;
```

### Custom Stylesheet with Parameters

```rust,ignore
let route = RouteBuilder::from("direct:start")
    .to("xj:file:///etc/xslt/custom.xslt?direction=xml2json&param.rootTag=data&maxPayloadBytes=1048576")
    .to("mock:result")
    .build()?;
```

### With Retry Configuration

```rust,ignore
let route = RouteBuilder::from("direct:start")
    .to("xj:classpath:identity?direction=xml2json&retryCount=5&retryDelayMs=1000")
    .to("mock:result")
    .build()?;
```

## Body Contract

| Direction  | Input Body       | Output Body         |
|------------|------------------|---------------------|
| `xml2json` | XML (string/bytes) | JSON (bytes)       |
| `json2xml` | JSON (string/bytes) | XML (bytes)        |

## How It Works

1. On first use, the component starts an `xml-bridge` subprocess that hosts a Saxon XSLT engine
2. The requested stylesheet is compiled and cached in the bridge
3. Each exchange body is sent to the bridge via gRPC for transformation
4. On transient transport errors, the bridge is automatically restarted and stylesheets are recompiled

## Configuration

### `XjComponentConfig`

```rust
use camel_xj::XjComponentConfig;
use std::path::PathBuf;

let config = XjComponentConfig {
    bridge_binary_path: Some(PathBuf::from("/opt/xml-bridge/bin/xml-bridge")),
    bridge_start_timeout_ms: 60_000,
    bridge_version: "1.0.0".to_string(),
    bridge_cache_dir: PathBuf::from("/tmp/camel-bridge-cache"),
};
```

### Consumer Support

XJ endpoints are **producer-only** — they do not support consumers. Use a `direct:` or `timer:` endpoint as the route source and send to the XJ endpoint.

## Health Check

The `camel-xj` component registers an async health check via `AsyncHealthCheck`.

- **Probe**: Issues a gRPC `HealthCheckRequest` to the xml-bridge sidecar process
- **Healthy**: Bridge gRPC channel reports `SERVING`
- **Degraded**: Bridge channel unreachable or reports not serving

Health checks are exposed via the health server:

```toml
[observability.health]
enabled = true
port = 8080
```
