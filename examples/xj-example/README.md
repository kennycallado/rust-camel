# xj-example

## What it does

Runs bidirectional XML↔JSON conversion routes using `camel-xj` through the xml-bridge.
It sends sample XML and JSON payloads and prints the converted results.

## Prerequisites

- The xml-bridge binary is auto-downloaded on first run (internet required).
- You can also provide a local bridge binary/JAR via `CAMEL_XML_BRIDGE_BINARY_PATH`.

## How to run (default)

```bash
cargo run --manifest-path examples/xj-example/Cargo.toml
```

## How to run with local binary

```bash
CAMEL_XML_BRIDGE_BINARY_PATH=/path/to/xml-bridge \
  cargo run --manifest-path examples/xj-example/Cargo.toml
```

## Expected output

You should see console/log output with XML converted to JSON and JSON converted back to XML.

## Direction parameter

This example demonstrates both `direction=xml2json` and `direction=json2xml` on `xj:` endpoints.
