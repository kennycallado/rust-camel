# camel-xslt

> XSLT Component for rust-camel (via xml-bridge)

## Overview

The camel-xslt provides functionality for the rust-camel integration framework.

## Features

- XSLT 1.0/2.0/3.0 transformations via xml-bridge
- On-demand bridge restart on transport error
- Graceful shutdown via `Lifecycle` integration (bridge process cleaned up on context stop)
- **Health Check**: Async gRPC health probe for the xml-bridge sidecar

## How It Works

The component communicates with a native `xml-bridge` process via gRPC. The bridge handles XSLT processing. On transport error, the bridge is automatically restarted. The bridge process is cleaned up when the Camel context stops via the `Lifecycle` service.

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
camel-xslt = { workspace = true }
```

## Health Check

The `camel-xslt` component registers an async health check via `AsyncHealthCheck`.

- **Probe**: Issues a gRPC `HealthCheckRequest` to the xml-bridge sidecar process
- **Healthy**: Bridge gRPC channel reports `SERVING`
- **Degraded**: Bridge channel unreachable or reports not serving

Health checks are exposed via the health server:

```toml
[observability.health]
enabled = true
port = 8080
```
