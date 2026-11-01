## ADDED Requirements

### Requirement: MIME split format selection

The system SHALL accept `format: mime` on a streaming split
(`split: { streaming: true, stream: { format: mime } }`) and resolve it
to a MIME splitter implemented in `camel-processor`. The splitter input
SHALL be `Body::Bytes` or `Body::Text`; the splitter SHALL materialize
the input and emit fragments sequentially. The options `max_parts`,
`max_part_size`, and `content_types` (snake_case, ADR-0017) SHALL be
accepted only with the `mime` format; setting any of them with another
format SHALL be a validation error. With `mime`, `chunk_size` and a
`batch_size` other than `1` SHALL also be rejected.

#### Scenario: mime format resolves to the MIME splitter

- **GIVEN** a route with `split: { streaming: true, stream: { format:
  mime } }`
- **WHEN** the route compiles
- **THEN** the split resolves to the MIME splitter and no unknown-format
  error is raised

#### Scenario: mime-only options rejected on other formats

- **GIVEN** `format: zip` with `max_parts: 5`
- **WHEN** the route configuration validates
- **THEN** validation fails with an error naming the mime-only option

#### Scenario: zero or negative limits rejected

- **GIVEN** `format: mime` with `max_parts: 0`
- **WHEN** the route configuration validates
- **THEN** validation fails naming `max_parts`

#### Scenario: undefined generic options rejected with mime

- **GIVEN** `format: mime` with `chunk_size: 1024`
- **WHEN** the route configuration validates
- **THEN** validation fails naming `chunk_size`

#### Scenario: routes without the new fields deserialize unchanged

- **GIVEN** an existing route YAML with `stream: { format: zip }` and no
  mime options, and a canonical spec serialized before this change
- **WHEN** both are deserialized
- **THEN** both succeed, the new fields default to absent, and a
  YAML-to-canonical round trip of an existing route is byte-stable

### Requirement: Attachment fragment emission

The splitter SHALL emit the leaf parts that `mail-parser` reports as
attachments, in document order, including inline images. The message's
own text and HTML body parts SHALL NOT be emitted; media type alone
SHALL NOT exclude a part, so a `text/csv` or `application/xml` part
declared with attachment disposition SHALL be emitted. The splitter
SHALL NOT emit multipart containers. A `message/rfc822` part SHALL be
one fragment carrying the exact raw embedded bytes; the splitter SHALL
NOT recurse into it and its descendants SHALL NOT be emitted as outer
fragments. Zero attachments SHALL emit zero fragments and succeed, and
the parent message SHALL be disposed normally.

#### Scenario: one and many parts

- **GIVEN** a MIME message with one attachment, and another with three
- **WHEN** the splitter runs on each
- **THEN** the first emits one fragment and the second emits three, in
  document order

#### Scenario: alternative container is not emitted

- **GIVEN** a multipart/alternative message with text and HTML bodies and
  no attachments
- **WHEN** the splitter runs
- **THEN** zero fragments are emitted and the split succeeds

#### Scenario: text attachment is emitted

- **GIVEN** a message with a `text/csv` part declared with
  `Content-Disposition: attachment`
- **WHEN** the splitter runs
- **THEN** the CSV part is emitted as a fragment

#### Scenario: inline image is emitted

- **GIVEN** a multipart/related message with an `image/png` part
  referenced by `Content-ID`
- **WHEN** the splitter runs
- **THEN** the image part is emitted as a fragment with disposition
  `inline` and its `CamelMimePartContentId` header set

#### Scenario: message/rfc822 is one raw fragment

- **GIVEN** a message with an `message/rfc822` attachment part that
  itself contains attachments
- **WHEN** the splitter runs
- **THEN** exactly one fragment is emitted whose body is byte-equal to
  the raw embedded message, and none of the embedded attachments appear
  as outer fragments

#### Scenario: zero attachments succeed with zero fragments

- **GIVEN** a plain text message with no attachments
- **WHEN** the splitter runs
- **THEN** zero fragments are emitted, the split succeeds, and the
  parent message is disposed normally

### Requirement: Fragment decoding

The splitter SHALL decode transfer encodings (base64,
quoted-printable, binary) through `mail-parser`, and each fragment body
SHALL be `Body::Bytes` of the decoded bytes. The splitter SHALL NOT
decompress payloads. An unparseable message, a malformed transfer
encoding, or a part the parser reports as undecodable SHALL be an error
that fails the parent exchange; it SHALL NOT be reported as a
successful zero-fragment split.

#### Scenario: base64 decode

- **GIVEN** an attachment with `Content-Transfer-Encoding: base64` and
  payload `AAEC`
- **WHEN** the splitter runs
- **THEN** the fragment body bytes are `[0, 1, 2]`

#### Scenario: quoted-printable decode

- **GIVEN** an attachment with `Content-Transfer-Encoding:
  quoted-printable`
- **WHEN** the splitter runs
- **THEN** the fragment body bytes are the decoded octets

#### Scenario: unparseable message fails the parent

- **GIVEN** input bytes that are not a MIME message at all
- **WHEN** the splitter runs
- **THEN** the split fails with a parse error and the parent exchange
  fails; zero fragments and success are not observed

### Requirement: Fragment metadata headers

Each fragment SHALL carry the copied parent headers plus
`CamelMimePartIndex` (0-based, counting emitted fragments after
filtering), `CamelMimePartName` (sanitized),
`CamelMimePartContentType` (the declared type, untrusted),
`CamelMimePartSize` (decoded byte length),
`CamelMimePartDisposition` (`attachment` or `inline`), and
`CamelMimePartContentId` when the part declares one. Inherited
`CamelMimePart*` headers SHALL be replaced or removed so each fragment
reflects only its own part. `CamelMimePartContentType` and
`CamelMimePartContentId` values SHALL have control characters removed
and SHALL be capped at 255 bytes on a UTF-8 boundary; neither value
SHALL be interpolated unsanitized into logs or error messages. The
fragment count SHALL be the standard `CamelSplitSize` on the last
fragment and SHALL count emitted fragments.

#### Scenario: parent headers copied and metadata present

- **GIVEN** a parent exchange with a custom header and a two-attachment
  message
- **WHEN** the splitter runs
- **THEN** each fragment carries the custom header, the index headers
  `0` and `1`, and name, content type, size, and disposition headers

#### Scenario: inherited content id does not leak

- **GIVEN** a parent exchange that already carries a
  `CamelMimePartContentId` header and a part without a Content-ID
- **WHEN** the splitter runs
- **THEN** the fragment carries no `CamelMimePartContentId` value from
  the parent

### Requirement: Split limits fail the parent

`max_parts` (default 100) SHALL bound the emitted fragment count, and
`max_part_size` (default: the input length) SHALL bound each decoded
part size. The splitter operates on an already-materialized input whose
size the transport bounds (`maxMessageSize` on mail sources); these
options bound emission and fragment construction, not transport input.
Both limits SHALL be checked before a fragment body is constructed.
Exceeding either limit SHALL be an error that fails the parent
exchange. For a push consumer route this leaves the consumed message
unseen and counts toward `maxAttempts`; `pollEnrich` disposition SHALL
be unchanged, the error propagating to the enriching route without
rollback.

#### Scenario: maxParts excess fails

- **GIVEN** `max_parts: 2` and a message with three matching attachments
- **WHEN** the splitter emits the third fragment
- **THEN** the split fails with a limit error and the parent exchange
  fails

#### Scenario: filtered parts do not consume the limit

- **GIVEN** `content_types: [application/pdf]`, `max_parts: 2`, and a
  message whose first three attachments are zips followed by one PDF
- **WHEN** the splitter runs
- **THEN** the non-matching parts are dropped before the limit applies
  and the PDF fragment is emitted with index `0`

#### Scenario: maxPartSize excess fails

- **GIVEN** `max_part_size: 10` and an attachment decoding to 11 bytes
- **WHEN** the splitter runs
- **THEN** the split fails with a limit error naming the part

#### Scenario: limit failure leaves the message unseen

- **GIVEN** a raw IMAP consumer route whose `split mime` hits a limit
- **WHEN** the route fails
- **THEN** the message stays UNSEEN and the attempt counts toward
  `maxAttempts`

#### Scenario: pollEnrich limit failure does not roll back

- **GIVEN** a `pollEnrich` route whose `split mime` hits a limit
- **WHEN** the split fails
- **THEN** the error propagates to the enriching route and the source
  message's disposition is unchanged

### Requirement: Content type allowlist

An optional `content_types` list SHALL match against the declared
Content-Type of a part using a case-insensitive comparison of
`type/subtype`, ignoring parameters; the value `*` SHALL NOT be
supported. A part with no declared Content-Type SHALL NOT match a
non-empty allowlist. Non-matching parts SHALL be dropped from the
fragment stream without error, and the allowlist SHALL be applied
before `max_parts`. An absent or empty list SHALL match everything.
Documentation SHALL note that DMARC reports often declare
`application/octet-stream`.

#### Scenario: allowlist drops non-matching parts

- **GIVEN** `content_types: [application/zip]` and a message with a zip
  and a PDF attachment
- **WHEN** the splitter runs
- **THEN** only the zip part is emitted and the split succeeds

#### Scenario: all parts filtered succeeds with zero fragments

- **GIVEN** `content_types: [application/zip]` and a message with only
  PDF attachments
- **WHEN** the splitter runs
- **THEN** zero fragments are emitted and the split succeeds

#### Scenario: matching is case-insensitive and parameter-blind

- **GIVEN** `content_types: [application/zip]` and a part declaring
  `Application/ZIP; name="a.zip"`
- **WHEN** the splitter runs
- **THEN** the part matches and is emitted

### Requirement: Fragment filename sanitization

The fragment filename SHALL come from the RFC 2231-decoded
`Content-Disposition` `filename`, else the Content-Type `name`
parameter. The splitter SHALL keep only the basename after the last `/`
or `\`, SHALL remove NUL and C0/C1 control characters and DEL, SHALL
trim leading and trailing spaces and dots, SHALL replace an empty
result, `.`, or `..` with `part-{index}`, and SHALL cap the result at
255 bytes without splitting a UTF-8 sequence. The raw sender-supplied
value SHALL NOT be exposed.

#### Scenario: sanitization table

- **GIVEN** parts declaring `../../etc/passwd`, `a\x00b.txt`,
  `  .hidden.  `, `..`, and ` report .zip `
- **WHEN** the splitter sanitizes each
- **THEN** the names are `passwd`, `ab.txt`, `hidden`, `part-{index}`,
  and `report .zip`

#### Scenario: multibyte name capped at a UTF-8 boundary

- **GIVEN** a filename of 300 multi-byte characters
- **WHEN** the splitter caps at 255 bytes
- **THEN** the result is at most 255 bytes and is valid UTF-8
