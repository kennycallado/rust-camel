# Design: mail-attachments

## Approach

Two phases, one change. Phase 1 is a mapping-only change inside the camel-mail
inbound path. Phase 2 adds a `mime` streaming split format across the standard
DSL-to-processor chain. The shape follows the e_opus ruling of 2026-11-01:
opt-in raw transport first, reusable splitter second.

Phase 1: add `raw` (boolean, default `false`) to `IMAP_OPTIONS`,
`ImapEndpointConfig`, and `ImapMetadataDescriptor` (parity test keeps them in
lockstep). `parse_inbound` gains a raw arm: body is `Body::Bytes` of the exact
`BODY.PEEK[]` bytes, all `CamelMailIn*` headers are set except
`CamelMailInHtmlBody`. Header extraction keeps using the parsed message, so
`Message-ID`/UID/date/folder mapping and the idempotency key do not move.
Because `pollEnrich` reaches the same inbound path through the endpoint-created
`PollingConsumer` (ADR-0015), the same flag covers it with no new plumbing.
Disposition, `maxAttempts`, `maxMessageSize`, and `idempotentRepository` code
is untouched.

Phase 2: `StreamSplitFormat` gains `Mime` (the enum is `#[non_exhaustive]`).
`StreamSplitConfig` gains `max_parts`, `max_part_size`, `content_types`
(optional; positivity-checked; rejected with a non-`mime` format so
mime-only options fail closed per ADR-0033). The DSL mirrors them as
snake_case YAML keys (ADR-0017) and maps `format: mime` to the new variant.
The camel-core compile arm calls `camel_processor::mime_splitter::
split_mime_bytes(parent, bytes, config)`, following the `zip_splitter`
signature: a pinned stream of `Result<Exchange, CamelError>`, fragments built
with `fragment_exchange` so parent headers copy over. `CamelSplitIndex` and
`CamelSplitSize` (last fragment) carry the standard count.

The splitter materializes the input (`Body::Bytes` or `Body::Text`), walks the
`mail-parser` message, and emits leaf parts reported as attachments in
document order, including inline images. Text/HTML bodies and multipart
containers are not emitted. A `message/rfc822` part is one fragment of raw
bytes; the splitter does not recurse, so a route may split it again. Transfer
encodings (base64, quoted-printable, binary) decode via `mail-parser`; there
is no decompression. An unparseable message or undecodable part is a stream
error, never a zero-fragment success. Per-part headers:
`CamelMimePartIndex` (counts emitted fragments after filtering),
`CamelMimePartName` (sanitized), `CamelMimePartContentType` (declared,
untrusted), `CamelMimePartSize` (decoded), `CamelMimePartDisposition`,
`CamelMimePartContentId` when present. Inherited `CamelMimePart*` headers
are replaced or removed so a fragment reflects only its own part; the
declared content-type and content-id values are control-stripped, capped at
255 bytes on a UTF-8 boundary, and never interpolated unsanitized into logs
or error messages. Limits: `max_parts` default 100, `max_part_size` default
input length. The splitter consumes an already-materialized input whose size
the transport bounds (`maxMessageSize` on mail sources); the options bound
emission and fragment construction — both checked before a fragment body is
built — not transport input. Excess is a stream error, which the existing
streaming-split outcome path turns into a parent failure: for a push
consumer route the message stays unseen and the attempt counts under
`maxAttempts`; `pollEnrich` disposition is unchanged, the error propagating
to the enriching route without rollback. An optional `content_types`
allowlist applies before `max_parts`, matches case-insensitively on
`type/subtype` ignoring parameters (no wildcard; a part without a declared
type never matches a non-empty list), and drops non-matching parts
silently. Filenames are untrusted exchange data (ADR-0032): RFC 2231-decoded
`filename`, else Content-Type `name`; basename after the last `/` or `\`;
C0/C1/NUL/DEL removed; spaces and dots trimmed; empty/`.`/`..` becomes
`part-{index}`; capped at 255 bytes on a UTF-8 boundary. The raw sender
value never reaches a header. Canonical compatibility: the new
`StreamSplitConfig` fields are optional with absent-serializing defaults, so
existing route YAML and canonical specs deserialize unchanged (ADR-0011).

## Affected crates

- `camel-component-mail`: `raw` param, raw inbound mapping, tests, docs.
- `camel-api`: `StreamSplitFormat::Mime`, `StreamSplitConfig` fields.
- `camel-dsl`: YAML keys, `mime` format mapping.
- `camel-core`: `DeclarativeStreamSplit` compile arm.
- `camel-processor`: `mime_splitter` module, `mail-parser` dependency.
- schemas: regenerated (`cargo xtask schema`).
- docs: mail page, streaming-splitter page, `mime-splitter` page, DMARC
  recipe, ADR-0094.

## Architecture boundaries

- API Contracts (camel-api) defines the enum and config types only.
- DSL parses; camel-core compiles; camel-processor executes — the standard
  split chain (ADR-0011/0016 canonical spec gains additive optional fields).
- Components stay transport-shaped: the mail component emits bytes and
  already-extracted envelope headers; MIME structure logic lives in the
  processor data plane (ADR-0001 data/control split, ADR-0032 trust
  boundary). A component must never be a processor dependency, which is why
  the splitter lives in camel-processor.

## Phases

### Phase 1: raw message access on IMAP consumption
- **Goal:** opt-in `raw=true` on the IMAP consumer and `pollEnrich` with an
  unchanged default path.
- **Dependencies:** none.
- **Externally-visible types/interfaces:** `raw` URI parameter on the `imap`
  scheme; raw body mapping.
- **Deliverable:** code + tests + mail-component spec delta + mail docs.
- **Exit-criteria:** camel-mail tests pass; default path unchanged
  (`attachments_ignored`); raw bytes exact; `CamelMailInHtmlBody` absent in
  raw mode; failing raw route leaves the message unseen; parity and clippy
  clean.

### Phase 2: reusable MIME splitter
- **Goal:** `format: mime` streaming split emitting decoded attachment
  fragments with sanitized metadata.
- **Dependencies:** Phase 1 (DMARC story consumes raw bytes).
- **Externally-visible types/interfaces:** `StreamSplitFormat::Mime`,
  `max_parts`/`max_part_size`/`content_types` split options, `CamelMimePart*`
  fragment headers, `split_mime_bytes`.
- **Deliverable:** code + tests + schema regen + mime-splitter spec capability
  + ADR-0094 + docs/recipe.
- **Exit-criteria:** splitter tests pass (0/1/N parts, alternative not
  emitted, inline image, `message/rfc822` raw fragment, base64/quoted-
  printable decode, sanitization table, limit errors, allowlist, parent
  header copy, end-to-end failure leaves message unseen); unparseable
  input fails rather than succeeding empty; existing routes deserialize
  and round-trip unchanged; schema check green; mdbook test green.

## Alternatives considered

- Consumer-side `attachments=split`: rejected (e_opus ruling) — it changes
  the one-exchange-per-UID contract, retry state, and disposition timing.
- New `camel-mime` crate: rejected — the workspace rule is no new crate;
  components must not be a dependency of the processor, so the splitter
  belongs in camel-processor next to the ZIP/TAR splitters.
- Decompression inside the splitter: rejected — non-goal; the DMARC recipe
  composes with the existing `unmarshal zip|gzip`.
- Recursive `message/rfc822` expansion: rejected — one raw fragment keeps the
  splitter total; a route can split the fragment again.
