# xslt-example

## What it does

Runs a simple XML transformation route using `camel-xslt` through the xml-bridge.
The example sends `<order><id>42</id></order>` to `direct:in` and prints the transformed XML.

## Prerequisites

- The xml-bridge binary is auto-downloaded on first run (internet required).
- You can also provide a local bridge binary/JAR via `CAMEL_XML_BRIDGE_BINARY_PATH`.

## How to run (default)

```bash
cargo run --manifest-path examples/xslt-example/Cargo.toml
```

## How to run with local binary

```bash
CAMEL_XML_BRIDGE_BINARY_PATH=/path/to/xml-bridge \
  cargo run --manifest-path examples/xslt-example/Cargo.toml
```

## Expected output

You should see logs/console output with the transformed XML payload.

## XSLT stylesheet

The stylesheet used by this example is `stylesheets/transform.xslt`.
Edit it to customize the transformation result.
