# Validator

The Validator component validates message bodies against XSD, JSON Schema, YAML Schema, and protobuf schema files. It runs in routes as a `to:` Producer. Validation failure returns a `CamelError` to the route's error handler.

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

```text
validator:<schema-path>[?type=xml|json|yaml|protobuf&schemaType=<type>&message=<fully.qualified.Message>&failOnNullBody=true|false&headerName=<name>&failOnNullHeader=true|false&maxPayloadBytes=<n>&schemaCacheMaxEntries=<n>&schemaAliases=<percent-encoded JSON object>]
```

| Option | Default | Description |
| --- | --- | --- |
| `type` | from extension | Schema type: `xml`, `json`, `yaml`, or `protobuf`. `rng` and `schematron` are parsed but rejected at endpoint creation. |
| `schemaType` | from extension | Canonical alias of `type`. Both keys are accepted and must agree when both are set. |
| `message` | none | Required for `protobuf`: fully-qualified message name, for example `test.v1.Person`. |
| `failOnNullBody` | `true` | Reject exchanges with empty bodies when `true` |
| `headerName` | none | Validate this header's value instead of the body |
| `failOnNullHeader` | `true` | Reject exchanges where the named header is missing |
| `maxPayloadBytes` | none | Reject bodies larger than this many bytes before validation |
| `schemaCacheMaxEntries` | `256` | Maximum XSD schema entries the bridge cache holds before eviction |
| `schemaAliases` | none | Absolute-URI to root-relative virtual path map, as one percent-encoded JSON object. See below. |

The schema type defaults to the file extension (`.xsd`, `.json`, `.yaml`, `.yml`, `.proto`). Pass `type=xml` to override the extension when the path is ambiguous. Paths accept percent-encoded characters.

`schemaAliases` accepts one JSON object that maps absolute schema URIs to root-relative virtual paths. The value is percent-encoded once. For example, `schemaAliases=%7B%22http%3A%2F%2Fexample.com%2Fp.xsd%22%3A%22a%2Fparent.xsd%22%7D` decodes once to `{"http://example.com/p.xsd":"a/parent.xsd"}`. A malformed value, a duplicated `schemaAliases` query parameter, or more than 128 entries fail endpoint creation before the bridge starts. Alias values are root-relative virtual paths.

XSD discovery follows only the reachable `xs:import` and `xs:include` edges within the schema file's parent directory (the confined root). The bounds fail closed during discovery: 256 resource entries, 8 MiB total bytes, 128 aliases, and import depth 32.

Rust reads the confined schema files and transmits the schema set in memory. The Java bridge resolves only from the transmitted in-memory set. Its supplied-key resolver never fetches: no network and no filesystem access occur.

### Format validation

JSON and YAML schemas enforce `format` keywords by default, including `email`, `uri`, and `date-time`. The schema's `$schema` key selects draft semantics; a schema without `$schema` is treated as draft 2020-12. Unknown formats are annotations and never produce errors.

To restore annotation-only behavior, set the opt-out in `Camel.toml`:

```toml
[components.validator]
validate-formats = false
```

Payloads that passed validation before format enforcement may now be rejected. Setting `validate-formats = false` restores the previous accept behavior.

## Behavior

JSON and YAML schemas compile when the endpoint is created. A malformed schema fails endpoint creation. The compiled validator stays cached for the lifetime of the endpoint. XSD schemas defer registration to the first validation call. This lets the bridge start in an async context without blocking endpoint creation.

XSD validation delegates to the `xml-bridge` gRPC backend. The bridge starts as a child process on the first XSD validation. The `XsdBridgeBackend` caches registered schemas up to `schemaCacheMaxEntries` and re-seeds them on reconnect. JSON and YAML validation never start the bridge.

Validation failure returns a `CamelError` to the route's error handler. Empty bodies, when `failOnNullBody=true` (default), also return an error. Pass `failOnNullBody=false` to let empty bodies flow through without validation. The same toggle exists for `headerName` mode via `failOnNullHeader`. The `validator` endpoint supports Producers only. Consumer creation returns an error.

### Schema diagnostics

The native `xml-bridge` packages the Xerces message bundles, including the regular-expression bundle. An `xs:pattern` that Xerces rejects is reported with the offending pattern and the Xerces parse message, up to a 512-character bound. A very long pattern can push the parse message past that bound. Older bridge builds hid the message behind `Installation Problem??? Couldn't load messages`. If you see that text, upgrade the bridge.

## Protobuf validation

```text
validator:schemas/person.proto?message=test.v1.Person
```

The schema type can also be set explicitly with `schemaType=protobuf` (or the `type=protobuf` alias), and `.proto` paths infer protobuf without either key. `schemaType` is the canonical key; when both keys are present they must agree, and unknown values are rejected.

The `message` parameter names the fully-qualified message to validate against. It is required for protobuf. A missing or empty `message` fails endpoint or route creation with a config error that names `message`.

The validator accepts JSON-shaped bodies. `Body::Json` is the primary input; `Body::Text` and `Body::Bytes` are parsed as UTF-8 JSON first. Other body variants are rejected. The body is never encoded or decoded as protobuf and never mutated: a body that passes validation passes through byte-identical, with no JSON normalization.

Validation rejects unknown fields, wrong scalar, enum, or oneof shapes, malformed JSON, and non-UTF-8 bytes. These failures raise `CamelError::ValidationError`, which the shared transport error mapping turns into HTTP 400 (`validation_error`), the same mapping REST schema failures use. This is new-path behavior and does not change how Xml, Json, or Yaml validator results are classified. JSON `null` and missing proto3 fields follow proto3 default semantics: they are accepted and the fields fall back to their defaults.

The descriptor compiles once at endpoint creation through the hermetic proto compiler (ADR-0084). The path may point at `.proto` source or at a precompiled descriptor set (`.binpb`, `.pb`, `.desc`, `.protoset`). Include paths are empty, and the existing path validation rejects `..` traversal. A missing schema file or an uncompilable schema fails endpoint creation. The compiled descriptor stays cached for the endpoint's lifetime.

The `maxPayloadBytes`, `headerName`, `failOnNullBody`, and `failOnNullHeader` options behave as described above and apply to protobuf endpoints.

### Descriptor recompile in dev watch

Under `camel run --watch`, a `.proto` edit recompiles the protobuf descriptor in-process. The watch sources are the schema file and every resolved local import, so an import-only edit also triggers a recompile. A reload that adds an import adds that file to the watch sources.

The watcher groups rapid edits behind the `watch_debounce_ms` window before dispatch. The recompile has a fixed 5000 ms deadline. A failed or timed-out compile keeps the prior descriptor serving, logs a redacted error, and leaves the route Running. It causes no route restart, no `RouteStatus` change, and no journal events.

Reload targets commit together per route. A failed validator target blocks the same-route commit for the template target class, and a failed template target blocks the same-route protobuf commit. Fix or revert the broken file to let the route commit again. See [Template](template.md#dev-watch) and [Hot reload](../configuration/hot-reload.md#dev-watch-for-template-closures).

**Non-goal: gRPC descriptors.** Dev watch covers the [Validator protobuf descriptors](#protobuf-validation) only. gRPC endpoints keep the descriptors they resolved at startup. A `.proto` edit used by a gRPC endpoint requires a route reload or a restart, and the watcher creates no targets for gRPC `protoFile` references.

## Bridge lifecycle

`CamelContext::stop()` does not clean up the `xml-bridge` child process. The `camel run` CLI registers a `BridgeCleanup` lifecycle service that calls `XsdBridgeBackend::shutdown()` on stop. Library embedders must retain the value from `ValidatorComponent::xsd_bridge_backend()` and call `shutdown().await` when it is `Some`. This requirement only applies after an XSD route starts the bridge. JSON and YAML routes never start it.

**Reference**: [Validator crate CONTEXT](https://github.com/kennycallado/rust-camel/blob/main/crates/components/camel-validator/CONTEXT.md). Example source: [`examples/validator`](https://github.com/kennycallado/rust-camel/tree/main/examples/validator).
