# XML

The XML data format converts an XML document into a JSON object and converts a JSON object back into XML text. It is built into `camel-processor`, is registered by default under the name `xml`, and needs no separate crate. See [Data Formats](index.md) for the full format catalog.

Unmarshal parses the wire text and returns `Body::Json`. Marshal reverses the conversion and returns `Body::Text`. The format never touches headers or exchange metadata.

## Usage

Name the format in the route step. Both directions accept an optional `config` block:

```yaml
- unmarshal: xml
  config:
    max_depth: 100
- marshal: xml
```

`unmarshal: xml` accepts `Body::Text`, `Body::Bytes`, and `Body::Xml`. A REST operation in `binding: xml` injects this step automatically; see [Top-level blocks](../yaml-dsl/top-level-blocks.md#rest-dsl).

## Limits

`max_depth` is the only configuration key. It caps XML nesting depth during unmarshal as a denial-of-service guard and defaults to `100`. The limit is inert during marshal. Unknown configuration keys fail closed.

A document deeper than `max_depth` fails with a `CamelError::TypeConversionFailed`.

## Unmarshal semantics

The document becomes a single-key JSON object whose key is the root element name. The value follows these projection rules:

| Wire form | JSON projection |
| --- | --- |
| Root element | A top-level object with exactly one element key. `<orders>...</orders>` becomes `{"orders": ...}`. |
| Attribute | An `@`-prefixed string key. `<item id="7"/>` becomes `{"item": {"@id": "7"}}`. |
| Mixed text | A `#text` key. `<p>hello<b>world</b></p>` keeps `"#text": "hello"` beside the child element. |
| Repeated child | An array in encounter order. Repeated `<item>` children become a JSON array. |
| Empty element | `null`. `<tag/>` becomes `{"tag": null}`. |
| Namespace declaration | Dropped. `xmlns` and `xmlns:*` attributes are not projected. |
| Leaf text | Always a JSON string. `<count>42</count>` becomes `{"count": "42"}`, not a number. |

Because leaf text is a string, JSON Schema validation runs against the projection, not the wire XML. A schema that types a leaf as an integer rejects the projected string.

`Body::Json` passes through unchanged. `Body::Stream` fails closed; materialize or cache the stream first.

### Entity security

A `DOCTYPE` without entity references converts normally. A reference to an entity outside the predefined XML entity set fails closed and is never expanded; the format performs no external entity resolution and no DTD evaluation.

## Marshal semantics

Marshal accepts a top-level JSON object with exactly one element key, which becomes the root element. These values fail with a typed conversion error:

- Zero keys or more than one key.
- A top-level array or scalar.
- A value that is not an object or a supported leaf.

Within the object, these keys shape the produced XML:

| Key | Meaning |
| --- | --- |
| Ordinary key | A child element. |
| `@name` | An attribute named `name`. |
| `#text` | Text content of the current element. |
| Array value | Repeats the element once per item. |

Number and boolean leaves are stringified. A round trip through XML therefore turns them back into strings.

A `Body::Text` body passes through unchanged.

## Empty body

`Body::Empty` marshal is a no-op and keeps the body as `Body::Empty`, matching the `json` format. `Body::Empty` unmarshal fails closed with `CamelError::TypeConversionFailed`. This is a deliberate divergence from `json` and `form`, which skip empty bodies.

## REST binding

In REST `binding: xml`, the request order is XML to JSON, then JSON Schema validation, then the user steps. The response order is user JSON, then the declared response schema as OpenAPI metadata only, then XML text. XML REST binding is not multipart.

**Reference**: [xml data format source](https://github.com/kennycallado/rust-camel/blob/main/crates/camel-processor/src/data_format/xml.rs)
