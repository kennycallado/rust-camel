# camel-xj

> XJ (XML <-> JSON) Data Format for Rust Camel (via xml-bridge)

## Overview

The camel-xj provides functionality for the rust-camel integration framework.

## Features

- XML ↔ JSON conversion via xml-bridge
- On-demand bridge restart on transport error
- Graceful shutdown via `Lifecycle` integration (bridge process cleaned up on context stop)

## How It Works

The component communicates with a native `xml-bridge` process via gRPC. The bridge handles XML/JSON conversion. On transport error, the bridge is automatically restarted. The bridge process is cleaned up when the Camel context stops via the `Lifecycle` service.

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
camel-xj = { workspace = true }
```
