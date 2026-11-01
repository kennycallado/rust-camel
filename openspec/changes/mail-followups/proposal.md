# Proposal: mail-followups

## Why

Three tracked follow-ups from the GreenMail/Mailpit end-to-end demo remain open in the mail
zone. They are small and independent, so one change ships them together instead of three
missions.

- `rc-dhd1k`: a bulk sender cannot emit `List-Unsubscribe` or `List-Unsubscribe-Post`. The
  producer passes only `X-*` headers, and bare standard header names must stay ignored because
  the HTTP consumer copies Title-Case request headers (ADR-0093). Bulk-sender deliverability
  therefore has no supported path.
- `rc-luh4q`: `crates/components/camel-mail/src/consumer.rs` drops the cause of a failed route.
  The failed-attempt branch calls `cycle.failed(uid)` and logs nothing, so the only visible
  signal is the final `maxAttempts` warning. An operator cannot see why a message was
  redelivered, including an HTTP SSRF-guard rejection.
- `rc-ulfzi`: five statements in `docs/src/components/mail.md` and the crate README do not match
  the shipped behavior (component metrics, empty body, `pollEnrich` headers, relative `caFile`,
  profile layout).

## What Changes

- New class-A exchange headers `CamelMailListUnsubscribe` and `CamelMailListUnsubscribePost`.
  The producer validates them and renders them as the standard wire headers
  `List-Unsubscribe` and `List-Unsubscribe-Post`. Bare standard header names remain ignored.
- One `warn` line per failed delivery attempt in the IMAP push consumer, carrying the UID, the
  attempt count and a redacted error kind. No retry, disposition or `maxAttempts` change.
- Documentation corrections for the five drifted statements, plus the two new headers on the
  producer page and an amendment note in ADR-0093's class-A list. No runtime change.

Excluded: producer attachments, consumer `attachments=split`, recipient/From changes,
decompression, any new crate, DKIM signing.

## Acceptance criteria

- Both headers render on the wire; a bare `List-Unsubscribe` exchange header is ignored.
- CR, LF, NUL, over-998-byte and non-ASCII values are rejected before any network I/O.
- `List-Unsubscribe-Post` without `List-Unsubscribe`, and a value other than the exact token
  `List-Unsubscribe=One-Click`, fail the exchange.
- Pairing with a `mailto:`-only `List-Unsubscribe` fails, per RFC 8058.
- Each failed delivery attempt logs one `warn` line with UID, attempt, `maxAttempts` and an
  error kind, and never logs message text, subject, body, credentials or a server URL.
- `cargo test -p camel-component-mail`, clippy `-D warnings`, all xtask lints, rustdoc
  `-D warnings`, `mdbook test` and `openspec validate --all --strict` pass.

## Risk budget

- Accepted risk: RFC 8058 one-click requires a DKIM signature over both headers. The component
  does not sign mail (DKIM stays out of scope); the docs state the requirement.
- Accepted risk: one extra `warn` line per failed attempt. `maxAttempts` bounds the volume per
  message.
- Out of bounds: any change to the retry, disposition or `maxAttempts` semantics, and any new
  public surface beyond the two header constants.
