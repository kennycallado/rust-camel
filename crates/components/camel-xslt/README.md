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

## Stylesheet parameters

An `xslt:` endpoint accepts three parameter forms. All three address a stylesheet `<xsl:param>` by the same `<name>`.

| Form | URI option | Description |
| --- | --- | --- |
| Static | `param.<name>=<value>` | Fixed at Endpoint creation. |
| Header-driven | `paramFrom.<name>=<header-key>` | Reads the value from the exchange header `<header-key>` at transform time. |
| Expression-driven | `paramExpr.<name>=<language>:<expression>` | Evaluates `<expression>` with the registered language `<language>` on each exchange. |

```text
xslt:/etc/transforms/order.xslt?param.locale=en
xslt:/etc/transforms/order.xslt?paramFrom.sequenceNumber=seqHeader
xslt:/etc/transforms/order.xslt?paramExpr.sequenceNumber=simple:${header.seq}
```

The language name is required. `simple` is a registered language name. The value splits at the first colon only, so the expression may contain colons.

### Precedence

When more than one form sets the same name, the highest layer with a non-null value wins.

| Layer (highest wins) | Source | Null/absent behavior |
| --- | --- | --- |
| 1 | `paramFrom` header present (non-null) | — |
| 2 | `paramExpr` evaluates non-null | Null/evaluation-omitted ⇒ falls to layer 3 |
| 3 | static `param` | — |

Rules:

- An absent or `Null` header falls through to the next layer.
- An absent, `Null`, or omitted expression result falls through to the static `param` value.
- `String` headers are forwarded verbatim. `Number` and `Bool` values become their canonical text.
- Array, map, and other unsupported values fail closed with a payload-free error that names the parameter.
- An expression evaluation error propagates as a transform error.
- An unknown language name or an expression compile error fails Endpoint creation. The error names the parameter and the language.

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
