# Proposal: mail-attachments

## Why

DMARC aggregate reports arrive as IMAP attachments (`.zip`, `.xml.gz`). The
mail consumer ignores attachments, so a route cannot reach them. A demo
route needs the report bytes. bd: `rc-chbb6` (P2).

An e_opus ruling (2026-11-01) fixed the shape: a hybrid. First an opt-in
`raw=true` on the IMAP consumer and `pollEnrich`, which keeps the
one-exchange-per-UID contract. Second a reusable MIME splitter behind the
existing streaming `split` EIP (`format: mime`), so attachment bytes reach
the route without changing consumer disposition, `maxAttempts`, or
`maxMessageSize` semantics.

## What Changes

- `camel-component-mail`: opt-in `raw` URI parameter (boolean, default
  `false`) for the IMAP consumer and `pollEnrich`. With `raw=true` the body
  is `Body::Bytes` of the exact RFC 822 bytes; `CamelMailIn*` headers stay;
  `CamelMailInHtmlBody` is omitted. Default behavior is unchanged.
- `camel-api`: `StreamSplitFormat::Mime` variant; mime-only split options
  (`max_parts`, `max_part_size`, `content_types`).
- `camel-dsl`: `format: mime` parses to the new variant; schema regenerated.
- `camel-core`: compile arm routes `mime` to the new splitter.
- `camel-processor`: new `mime_splitter` module (workspace `mail-parser`
  dependency). Emits decoded attachment parts as fragments with sanitized
  metadata headers, bounded by `max_parts`/`max_part_size`.
- Docs and spec: mail consumer page, streaming-splitter page, DMARC recipe,
  ADR for untrusted MIME parts, mail-component delta, new `mime-splitter`
  capability.

Out of scope: `attachments=split` consumer mode, producer attachments,
outbound List-Unsubscribe (`rc-dhd1k`), decompression in mail/MIME,
recursive `message/rfc822` expansion, a new crate.

## Acceptance criteria

- `raw=true` yields exact message bytes; default path still ignores
  attachments; a failing raw route leaves the message unseen.
- The splitter emits attachment parts in document order with sanitized
  headers; text/HTML bodies and containers are not emitted.
- `max_parts`/`max_part_size` excess fails the parent; on a push consumer
  route the message stays unseen and the attempt counts toward
  `maxAttempts` (`pollEnrich` disposition unchanged, no rollback); zero
  attachments succeeds with zero fragments.
- `cargo test` for camel-mail, camel-api, camel-processor, camel-core,
  camel-dsl passes; fmt, clippy `-D warnings`, xtask lints, schema check,
  rustdoc, and `openspec validate --all --strict` pass.

## Risk budget

- Untrusted MIME structure is the main risk. Mitigated by sanitizing every
  fragment header sourced from the message (filenames, declared content
  types, content IDs are control-stripped and length-capped), hard default
  limits, no decompression, and an ADR recording the trust boundary.
- Additive enum/config changes only; no existing variant or default moves.
