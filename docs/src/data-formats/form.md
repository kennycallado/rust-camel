# Form

The form data format converts an `application/x-www-form-urlencoded` body into a JSON object and converts a JSON object back into form text. It is built into `camel-processor`, is registered by default under the name `form`, and needs no separate crate. See [Data Formats](index.md) for the full format catalog.

Unmarshal parses the wire text and returns `Body::Json`. Marshal reverses the conversion and returns `Body::Text`. The format never touches headers or exchange metadata.

## Usage

Name the format in the route step. Both directions accept an optional `config` block:

```yaml
- unmarshal: form
  config:
    max_fields: 1000
    max_bytes: 1048576
- marshal: form
```

`unmarshal: form` accepts `Body::Text` and `Body::Bytes`. A REST operation in `binding: form` injects this step automatically; see [Top-level blocks](../yaml-dsl/top-level-blocks.md#rest-dsl).

## Limits

Both caps are on by default and fail closed. A breach returns a `CamelError::TypeConversionFailed` whose message starts with `form` and names the cap.

| Config key | Default | Applies to | Meaning |
| --- | --- | --- | --- |
| `max_fields` | `1000` | unmarshal | Maximum number of non-empty pairs, **duplicates included**. Empty `&` segments do not count. |
| `max_bytes` | `1048576` (1 MiB) | unmarshal | Maximum input length in bytes, checked before any pair is parsed. |

A payload with exactly `max_fields` non-empty pairs passes; one pair more fails. Set a key to `null` to disable that cap. Unknown configuration keys fail closed.

## Unmarshal semantics

| Input | Result |
| --- | --- |
| Pairs split on `&` | Empty segments from a leading `&`, trailing `&`, or `&&` are skipped and do not count toward `max_fields`. |
| `key=value` | Splits on the first `=`. A missing `=` yields an empty value, so `flag` becomes `{"flag": ""}`. An empty key is allowed, so `=v` becomes `{"": "v"}`. |
| Duplicate keys | Become an array with values in encounter order. `tag=a&tag=b&tag=c` becomes `{"tag": ["a", "b", "c"]}` with no lossy overwrite. |
| `+` | Decodes to a space. |
| `%XX` | Decodes strictly. A malformed escape such as `%2` or `%zz` fails and reports its byte offset. |
| UTF-8 | Decoded bytes must be valid UTF-8. Non-UTF-8 bytes are rejected; no charset transcode is attempted. |
| Empty input | Unmarshals to the empty JSON object `{}`. |
| `Body::Empty` | No-op; the body stays `Body::Empty` and no error is raised. |
| `Body::Stream` | Fails closed. The format never consumes a stream. A route must materialize or cache the stream first. |

Only a parsed JSON object can feed a schema. In REST `binding: form`, validation runs on the JSON object produced by the form unmarshal, so the order is form parse, then JSON, then schema validation.

## Marshal semantics

Marshal accepts only a top-level JSON object. Scalars inside that object map as follows:

| Value | Encoding |
| --- | --- |
| String | Passes through as-is. |
| Number, boolean, null | Stringifies to its JSON lexical form (`1`, `true`, `null`). |
| Array of scalars | Repeats the key once per element. `{"tag": ["a", "b"]}` becomes `tag=a&tag=b`. |
| Nested object or nested array | Rejected with a message that names the nested value. |
| Empty object | Marshals to `Body::Text("")`. |
| Non-object top-level value | Rejected. |
| `Body::Empty` | No-op. |
| `Body::Stream` | Fails closed. |

Encoding follows the WHATWG `application/x-www-form-urlencoded` serializer: ASCII alphanumerics and `*-._` stay unescaped, space becomes `+`, and every other byte becomes uppercase `%XX` over its UTF-8 encoding. For example, `{"q": "a b&c=d+é"}` becomes `q=a+b%26c%3Dd%2B%C3%A9`.

Object key iteration order in the produced text is unspecified. Consumers compare by the parsed pair set or JSON value, not by textual order.

## No header auto-mapping

Form fields never become headers. The format does not copy a field named `Content-Type` or any other name into the exchange headers, and it does not read headers back into the form text. Mapping stays explicit: use `set_header` or an expression step when a route needs a field in a header.

**Reference**: [form data format source](https://github.com/kennycallado/rust-camel/blob/main/crates/camel-processor/src/data_format/form.rs)
