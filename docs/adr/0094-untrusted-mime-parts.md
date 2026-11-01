# ADR-0094: Untrusted MIME parts, bounded emission and no recursion

- Status: Accepted (2026-11-01; bd rc-chbb6)
- Source: openspec change `mail-attachments`
- Cross-refs: ADR-0001, ADR-0015, ADR-0025, ADR-0032, ADR-0093

## Context

DMARC aggregate reports arrive as attachments (`.xml.gz`, `.zip`) in an IMAP
message. The mail consumer ignores attachments, so a route cannot reach the
report bytes. A route needs a way to split one RFC 822 message into its
attachment parts.

Every value in a MIME part comes from the sender: the attachment name, the
declared content type and the content ID. The body bytes also come from the
sender. An attacker can control all of them.

A MIME message is also a resource decision. A single message can declare
thousands of parts, and one part can declare a body that is much larger than
the message. A splitter that emits without a bound turns a mailbox into a
memory and processing DoS.

## Decision

### Trust boundary

Attachment names, declared content types, content IDs and body bytes are
exchange data. Exchange data is untrusted ([ADR-0032](0032-exchange-data-trust-boundary.md)).
The splitter does not verify any of these values. It emits them so the route
can use them.

The splitter sanitizes the values before they become fragment headers:
control characters are removed, and the name, declared type and content ID
are capped at 255 bytes. The name is reduced to its basename. A missing or
empty name becomes `part-<index>`. Sanitizing keeps a value safe as a header
value. It does not make the value trustworthy. A route must validate a name
before it uses the name for a path, a query or another interpretable sink.

### Decoded bytes, no decompression

The splitter emits the decoded part body as `Body::Bytes`. It decodes the
MIME transfer encoding. It does not decompress the payload. A ZIP or gzip
attachment arrives compressed, and the route adds `unmarshal: zip` or
`unmarshal: gzip` to decode it.

This keeps one decision in one place. The splitter bounds the bytes it emits,
and the data format owns its own decompression bound (`max_decompressed_size`
and `max_input_size`). Automatic decompression would add a second unbounded
decode to a path that already crosses a trust boundary.

### Bounded emission

Two options bound the emission:

- `max_parts` (default 100) caps the number of emitted fragments.
- `max_part_size` (default: the input length) caps the decoded size of one
  fragment, in bytes.

A `content_types` allowlist selects the declared `type/subtype` values before
`max_parts` applies, so a filtered-out part does not use the part budget. The
allowlist compares case-insensitively and has no wildcard. An absent or empty
list matches every part.

The limits are mandatory. The splitter has no unbounded mode.

### Sanitized metadata

Each fragment carries `CamelMimePartIndex`, `CamelMimePartName`,
`CamelMimePartContentType`, `CamelMimePartSize`, `CamelMimePartDisposition`
and, when the part declares one, `CamelMimePartContentId`. The declared
content type is the sender's value, and the fragment header documents that
it is untrusted. `CamelMimePartSize` reports decoded bytes, not the declared
size.

The splitter does not emit text or HTML body parts and does not emit
multipart containers. A text or HTML attachment arrives charset-converted to
UTF-8. An inline image is emitted with `CamelMimePartDisposition` set to
`inline`; a part with no declared disposition is `attachment`.

### No recursion into `message/rfc822`

A `message/rfc822` part is one raw fragment. The splitter does not recurse
into the embedded message. Recursion would multiply the part budget per
nesting level, and a small message could expand into an unbounded tree. One
level is enough for the attachment use case. A route that needs the inner
attachments splits the fragment again as a separate step.

### Streaming semantics and partial delivery

The splitter is a stream on the existing streaming split step
([ADR-0001](0001-tower-data-plane-split-from-control-plane.md),
[ADR-0025](0025-outcome-aware-structural-eips.md)). A limit violation fails
the stream, and the parent exchange fails. The failure does not undo the
fragments that were already emitted: the sub-pipeline may already have
processed them. A route that must tolerate a later limit failure keeps its
sub-pipeline idempotent.

The splitter changes no consumer semantics. The IMAP consumer keeps its
`raw=true` opt-in ([ADR-0093](0093-mail-component-posture-and-header-classes.md)),
its disposition order, its `maxAttempts` counter and its `maxMessageSize`
check. The splitter is a route step after the consumer.

## Consequences

- A route can reach attachment bytes without a custom MIME parser.
- A sender cannot make the splitter emit an unbounded number of parts or an
  unbounded part body.
- A route that trusts `CamelMimePartName` or `CamelMimePartContentType` is
  exposed to sender-controlled values. Validation stays with the route.
- A ZIP or gzip attachment needs an explicit `unmarshal` step.
- A limit failure can arrive after some fragments were processed.
- A nested `message/rfc822` attachment needs a second split step.

## Alternatives considered

- **Automatic decompression in the splitter.** Rejected. It adds a second
  decode with its own bound, and it hides a route decision in a split step.
- **Recurse into `message/rfc822`.** Rejected. It multiplies the part budget
  per nesting level and gives an attacker an amplification path.
- **Trust the declared content type.** Rejected. The sender sets the value
  ([ADR-0032](0032-exchange-data-trust-boundary.md)); a route must not select
  a path or a decoder from it without validation.
- **Eager materialization of all parts.** Rejected. The streaming split step
  already provides backpressure and constant-memory delivery; a second eager
  splitter would duplicate the split contract.
- **Unbounded defaults.** Rejected. The limits are mandatory and bounded by
  default, so an operator must opt in to a larger budget.
