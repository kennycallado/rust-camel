# XSLT and XJ

XSLT transforms XML with a stylesheet. XJ converts between XML and JSON. Both components are producer-only and both delegate to a Java/Saxon `xml-bridge` sidecar over gRPC. The Rust side never parses XML or executes XSLT.

XJ sits on top of XSLT. The same bridge compiles every stylesheet, whether you supply your own or use the bundled identity pair.

## XSLT

`xslt:<stylesheet>` reads an XSLT stylesheet from disk and applies it to the Exchange body. The body must be XML. The Producer replaces the body with the transformation result.

```rust,ignore
let route = RouteBuilder::from("direct:in")
    .to("xslt:/etc/transforms/order.xslt?output=xml&param.locale=en")
    .build()?;
```

<details>
<summary>YAML equivalent</summary>

```yaml
routes:
  - id: xslt-transform
    from: "direct:in"
    steps:
      - to: "xslt:/etc/transforms/order.xslt?output=xml&param.locale=en"
```

</details>

The Producer bounds the body before forwarding. When `maxPayloadBytes` is absent, `XsltProducer` uses `DEFAULT_MATERIALIZE_LIMIT` (10 MiB). Bodies that exceed the limit return an error before any bytes reach the bridge.

### URI

```
xslt:<stylesheet>[?output=<method>][&param.<name>=<value>][&transformerCacheSize=<n>][&failOnNullBody=<true|false>][&maxPayloadBytes=<n>]
```

| Parameter | Required | Default | Description |
| --- | --- | --- | --- |
| `stylesheet` | yes | — | Path to the XSLT file. Accepts `file://` prefix or a plain path. |
| `output` | no | stylesheet default | Output method: `xml`, `html`, or `text`. |
| `param.<name>` | no | — | XSLT parameter. Forwarded as a string. Becomes an `<xsl:param>` value inside the stylesheet. |
| `transformerCacheSize` | no | unlimited | Maximum compiled stylesheets the bridge keeps in cache. `0` disables caching. |
| `failOnNullBody` | no | `false` | Return an error when the body is empty. When `false`, the empty body is forwarded. |
| `maxPayloadBytes` | no | 10 MiB | Reject Exchange bodies larger than this before sending to the bridge. `0` is rejected. |

No Exchange field can select or replace the stylesheet. The stylesheet is fixed at Endpoint creation. The Exchange body is the only XML the bridge sees.

## XJ

`xj:<stylesheet>?direction=<xml2json|json2xml>` converts between XML and JSON. The bundled identity stylesheet at `classpath:identity` covers the common case. A custom stylesheet covers anything else.

```rust,ignore
let route = RouteBuilder::from("timer:tick?period=1000")
    .set_body(Body::Xml("<root><name>Camel</name></root>".to_string()))
    .to("xj:classpath:identity?direction=xml2json")
    .log("JSON output: ${body}", LogLevel::Info)
    .build()?;
```

<details>
<summary>YAML equivalent</summary>

```yaml
routes:
  - id: xml-to-json
    from: "timer:tick?period=1000"
    steps:
      - set_body: "<root><name>Camel</name></root>"
      - to: "xj:classpath:identity?direction=xml2json"
      - to: "log:info"
```

</details>

`xml2json` takes an XML body and returns JSON. `json2xml` takes a JSON body and returns XML. The Producer replaces the body with the conversion result and preserves the inbound UTF-8 on the JSON side. Rust does not parse the JSON document. The bridge runs `json-to-xml()` on the XSLT 3.0 side.

The `xml2json` identity stylesheet follows the Apache Camel xj compatibility convention. Attributes become `"@name"` keys. Text content becomes `"#text"` when the element also has attributes or children. Repeated siblings become JSON arrays. A self-closing element with no attributes becomes `null`. A simple leaf with no attributes or children becomes a plain string.

### URI

```
xj:<stylesheet>?direction=<xml2json|json2xml>[&maxPayloadBytes=<n>][&retryCount=<n>][&retryDelayMs=<n>][&param.<name>=<value>]
```

| Parameter | Required | Default | Description |
| --- | --- | --- | --- |
| `stylesheet` | yes | — | `classpath:identity` for the bundled pair, or a custom `file://` path. |
| `direction` | yes | — | `xml2json` or `json2xml`. |
| `maxPayloadBytes` | no | 10 MiB | Reject Exchange bodies larger than this before sending to the bridge. |
| `retryCount` | no | `3` | Retries on transient transport failure. |
| `retryDelayMs` | no | `500` | Delay between retries. |
| `param.<name>` | no | — | XSLT parameter forwarded to the bridge. |

The parser also accepts `transformDirection` and `resourceUri`, but Endpoint creation does not pass them to the Producer. Both options are silently ignored. Do not depend on them.

## Bridge model

Both components share the `xml-bridge` sidecar. Rust does not parse XML or execute XSLT. It reads the stylesheet bytes at Endpoint creation, bounds the Exchange body, and forwards both as bytes through `proto/xml_bridge.proto`. The sidecar owns stylesheet compilation, transformation, XML parsing, DTD handling, and entity resolution.

The bridge process starts on first use. It exits when the Camel context stops. On transport failure, the runtime restarts the bridge and recompiles every cached stylesheet. Transient transport errors trigger retries with the configured backoff before the route sees a failure.

## Trust model

ADR-0032 classifies endpoint configuration as trusted operator input and the Exchange body as untrusted exchange data. The stylesheet is read from disk during Endpoint creation. The Exchange body is bounded and forwarded without Rust-side XML parsing. The sidecar is the security location for XSLT secure processing, `document()` restrictions, XXE controls, entity-expansion limits, and XML-bomb protection. `BridgeError.Kind.SECURITY_VIOLATION` reports a policy rejection. The presence of that contract does not prove every defense is enabled. Audit those controls in `bridges/xml/`.

## Error handling

The Producer logs stylesheet compilation failures and transform failures at `warn!`. The route `ErrorHandler` owns the resulting error (ADR-0012 category a). The bridge client logs reseed failures at `error!` with a matching metric. That category is outside-contract and signals transient recovery, not a handler call.

**Reference**: [camel-xslt CONTEXT](https://github.com/kennycallado/rust-camel/blob/main/crates/components/camel-xslt/CONTEXT.md), [camel-xj CONTEXT](https://github.com/kennycallado/rust-camel/blob/main/crates/components/camel-xj/CONTEXT.md).
