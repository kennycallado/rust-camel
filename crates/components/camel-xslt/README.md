# camel-xslt

> XSLT Component for rust-camel (via xml-bridge)

## Overview

The camel-xslt provides functionality for the rust-camel integration framework.

## Features

- XSLT 1.0/2.0/3.0 transformations via xml-bridge
- On-demand bridge restart on transport error
- Graceful shutdown via `Lifecycle` integration (bridge process cleaned up on context stop)

## How It Works

The component communicates with a native `xml-bridge` process via gRPC. The bridge handles XSLT processing. On transport error, the bridge is automatically restarted. The bridge process is cleaned up when the Camel context stops via the `Lifecycle` service.

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
camel-xslt = { workspace = true }
```
