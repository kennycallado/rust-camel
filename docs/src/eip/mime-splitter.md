# MIME Splitter

The MIME Splitter is a specialized Splitter from Hohpe and Woolf. It decomposes one RFC 822 MIME message into one exchange per attachment. Each attachment flows through the route as its own message, so a route can reach the attachment bytes without writing a MIME parser.

## Configuration

YAML selects the format through the streaming split step:

```yaml
- id: mime-split-route
  from: "imap://mail.example.com?folder=INBOX&raw=true"
  steps:
    - split:
        streaming: true
        stream:
          format: mime
        aggregation: collect_all
        steps:
          - to: log:part?showBody=true&showHeaders=true
```

The IMAP `raw=true` option supplies the exact RFC 822 bytes as `Body::Bytes`. The splitter parses those bytes and emits the attachment parts. See the [mail component](../components/mail.md) for the consumer side.

| Option | Default | Description |
|--------|---------|-------------|
| `max_parts` | `100` | Maximum number of emitted fragments. |
| `max_part_size` | input length | Maximum decoded size in bytes of one fragment. |
| `content_types` | none | Allowlist of declared `type/subtype` values. |

`content_types` compares the declared media type case-insensitively. It has no wildcard. An absent or empty list matches every part. The allowlist runs before `max_parts`, so a filtered-out part does not use the part budget. `max_parts` and `max_part_size` must be at least 1. Both options are valid only with `format: mime`; another format rejects them at parse time.

## Emitted fragments

The splitter emits parts in document order. Each fragment carries the decoded part body as `Body::Bytes` plus these headers:

| Header | Meaning |
|--------|---------|
| `CamelMimePartIndex` | Zero-based index of the emitted fragment. |
| `CamelMimePartName` | Sanitized attachment filename. |
| `CamelMimePartContentType` | Declared `type/subtype` of the part. Untrusted. |
| `CamelMimePartSize` | Decoded body length in bytes. |
| `CamelMimePartDisposition` | `attachment` or `inline`. |
| `CamelMimePartContentId` | Content-ID, when the part declares one. |

The splitter does not emit text or HTML body parts, and multipart containers produce no fragment. A text or HTML attachment arrives charset-converted to UTF-8. An inline image is emitted with `CamelMimePartDisposition` set to `inline`; the disposition is `attachment` when the part declares none.

A `message/rfc822` part is emitted as one raw fragment. The splitter does not recurse into the embedded message, so an attachment inside that message stays inside the fragment bytes.

The splitter emits decoded bytes only. It never decompresses a part body. A part with a ZIP or gzip payload arrives compressed; add `unmarshal: zip` or `unmarshal: gzip` after the split to decode it. The splitter does not change consumer disposition, `maxAttempts` or `maxMessageSize` semantics.

The content type is the value the sender declared, not a value the splitter verified. Attachment names, declared types and content IDs are exchange data and are untrusted ([ADR-0032](../adr/0032-exchange-data-trust-boundary.md), [ADR-0094](../adr/0094-untrusted-mime-parts.md)). The splitter reduces the name to its basename, strips control characters and caps it at 255 bytes; a missing or empty name becomes `part-<index>`. The declared type and the content ID get the same control stripping and cap. A route can use these values as header values, but must validate them before it uses them for a path, a query or another interpretable sink.

DMARC aggregate reports usually arrive as an `application/octet-stream` part with a `.zip` or `.gz` name. Select them with `content_types: ["application/octet-stream"]`, then decode with `unmarshal`. The [mail component](../components/mail.md#dmarc-aggregate-reports) shows the full recipe.

## Limits and failure

The splitter is a stream. A limit violation fails the stream with a `TypeConversionFailed` error, and the parent exchange fails. Earlier fragments were already emitted before the error, so the sub-pipeline may already have processed them. Keep the sub-pipeline idempotent when the route must tolerate a later limit failure.

A part with a malformed transfer encoding fails the stream the same way. A message that does not parse as MIME fails before the splitter emits a fragment.

Per [ADR-0001](../adr/0001-tower-data-plane-split-from-control-plane.md), the splitter compiles into a `Service<Exchange>` step in the Tower middleware pipeline. The per-fragment sub-pipeline compiles into child steps on the same route channel. The processor contract is documented in [camel-processor/CONTEXT.md](https://github.com/kennycallado/rust-camel/blob/main/crates/camel-processor/CONTEXT.md).

The [Streaming Splitter](streaming-splitter.md) is the general alternative for byte streams. The [ZIP Splitter](zip-splitter.md) and [TAR Splitter](tar-splitter.md) split archives that are already uncompressed.
