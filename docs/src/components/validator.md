# Validator

The Validator component validates message bodies against XSD, JSON Schema, and YAML Schema files. It runs in routes as a `to:` Producer. Validation failure returns a `CamelError` to the route's error handler.

The validator-example wires timer-driven producers against local schemas for each format:

```rust,ignore
{{#include ../../../examples/validator/src/main.rs:validator-route}}
```

<details>
<summary>YAML equivalent</summary>

```yaml
routes:
  - id: xsd-valid
    from: "timer:xsd-valid?period=3000&repeatCount=2"
    steps:
      - set_body: "<order><id>A1</id><amount>5</amount></order>"
      - log: "Validating XML order against XSD"
      - to: "validator:schemas/order.xsd"
      - log: "XML is valid"
      - to: "log:info?showBody=true"
```

The Rust example resolves the schema path with `CARGO_MANIFEST_DIR`. Substitute your own path in the URI.

</details>

## URI

```
validator:<schema-path>[?type=xml|json|yaml&failOnNullBody=true|false&headerName=<name>&failOnNullHeader=true|false&maxPayloadBytes=<n>&schemaCacheMaxEntries=<n>]
```

| Option | Default | Description |
| --- | --- | --- |
| `type` | from extension | Schema type: `xml`, `json`, or `yaml`. `rng` and `schematron` are parsed but rejected at endpoint creation. |
| `failOnNullBody` | `true` | Reject exchanges with empty bodies when `true` |
| `headerName` | none | Validate this header's value instead of the body |
| `failOnNullHeader` | `true` | Reject exchanges where the named header is missing |
| `maxPayloadBytes` | none | Reject bodies larger than this many bytes before validation |
| `schemaCacheMaxEntries` | `256` | Maximum XSD schema entries the bridge cache holds before eviction |

The schema type defaults to the file extension (`.xsd`, `.json`, `.yaml`, `.yml`). Pass `type=xml` to override the extension when the path is ambiguous. Paths accept percent-encoded characters.

## Behavior

JSON and YAML schemas compile when the endpoint is created. A malformed schema fails endpoint creation. The compiled validator stays cached for the lifetime of the endpoint. XSD schemas defer registration to the first validation call. This lets the bridge start in an async context without blocking endpoint creation.

XSD validation delegates to the `xml-bridge` gRPC backend. The bridge starts as a child process on the first XSD validation. The `XsdBridgeBackend` caches registered schemas up to `schemaCacheMaxEntries` and re-seeds them on reconnect. JSON and YAML validation never start the bridge.

Validation failure returns a `CamelError` to the route's error handler. Empty bodies, when `failOnNullBody=true` (default), also return an error. Pass `failOnNullBody=false` to let empty bodies flow through without validation. The same toggle exists for `headerName` mode via `failOnNullHeader`. The `validator` endpoint supports Producers only. Consumer creation returns an error.

## Bridge lifecycle

`CamelContext::stop()` does not clean up the `xml-bridge` child process. The `camel run` CLI registers a `BridgeCleanup` lifecycle service that calls `XsdBridgeBackend::shutdown()` on stop. Library embedders must retain the value from `ValidatorComponent::xsd_bridge_backend()` and call `shutdown().await` when it is `Some`. This requirement only applies after an XSD route starts the bridge. JSON and YAML routes never start it.

**Reference**: [Validator crate CONTEXT](https://github.com/kennycallado/rust-camel/blob/main/crates/components/camel-validator/CONTEXT.md). Example source: [`examples/validator`](https://github.com/kennycallado/rust-camel/tree/main/examples/validator).
