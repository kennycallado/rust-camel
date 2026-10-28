# Proposal: mail-component

## Why

rust-camel has no native e-mail support. A route can send or receive mail
today only through custom HTTP gateways or a JVM bridge. Apache Camel's
`camel-mail` is among its most used components. bd `rc-q8zur` (P2) tracks
this gap as the second Track C1 interop connector of the `rc-ca8z`
roadmap, after rabbitmq. The owner waived the `rc-ca8z` filing gate for
this child on 2026-10-19.

## What Changes

New crate `camel-component-mail` serving four schemes from one bundle:
`smtp:`, `smtps:` (implicit TLS), `imap:`, `imaps:` (implicit TLS). It
ships in the REGULAR CLI flavor, like rabbitmq. It lands as ONE change
with THREE delivery phases and one squash-merge:

- Phase 1 — Foundation and SMTP producer: crate skeleton, config,
  secret redaction, `MailTransport` seam over `lettre`, exchange-to-MIME
  (text, HTML, alternative), header policy with a recipient fence,
  never-resend-after-DATA send semantics, full CLI/bundle/flavor/publish
  registration, in-process fake SMTP server, Docker tier, ADR-0093.
- Phase 2 — IMAP receive: `MailStore` seam over `async-imap`, message
  parsing with `mail-parser`, poll loop, disposition after the route
  completes, health and metrics, GreenMail tier.
- Phase 3 — Disposition, dedup, pull: `delete`, `moveTo`, `copyTo`,
  UIDVALIDITY tracking, Message-ID idempotency, `PollingConsumer`
  for `pollEnrich`, full guide page.
- Trust anchors: an account may add a PEM CA file (no insecure flag),
  so internal relays and the self-signed Docker fixtures verify.

Excluded (recorded in design as non-goals, "future bd under rc-ca8z",
filed only after archive while the gate allows): XOAUTH2, IMAP IDLE,
attachment model, POP3, Gmail/Graph REST, DKIM, inbound SMTP server,
SearchTerm parity.

Affected crates: `camel-component-mail` (new), `camel-bundles`,
`camel-cli` (feature `mail` in `flavor-regular` and legacy `full`,
golden deptree regeneration), workspace `Cargo.toml`,
`scripts/xtask/trustpub-registrations.toml`, `CONTEXT-MAP.md`, crate
`CONTEXT.md`, mdBook guide, new ADR-0093.

## Acceptance criteria

- SMTP producer: STARTTLS by default, PLAIN/LOGIN auth, text, HTML and
  alternative bodies, pooled transport, `CamelMailMessageId` header.
- Header policy: routing headers on an unarmed endpoint error;
  `allowedRecipients` armed rejects outside-list addresses; CR/LF/NUL
  rejected in content headers; Bcc is envelope-only.
- IMAP consumer: at-least-once delivery with defaults; `\Seen` only
  after a successful route; failed route leaves the message untouched;
  size-first cap; bounded redelivery of failing messages; cancellation
  and reconnect per ADR-0007/0013.
- Phase 3 features behave as specified; `PollingConsumer` works with
  `pollEnrich`.
- `flavor_marker_table`, `default_closure_matches_golden` and
  `full_covers_universe` green; all AGENTS.md quality gates green.
- Docker tiers gated by `MAIL_ITEST=1`: pass, or fail `infra-unavailable`
  when Docker is absent, never a silent skip.

## Risk budget

Acceptable: new dependency tree (`lettre`, `async-imap`, `mail-parser`)
audited by `cargo audit`; duplicate `thiserror` 1 from `async-imap`
(accepted); regular-flavor binary growth.
Out of bounds: a new aws-lc edge selected by the component's own
dependencies (ring only); core or api crate changes; credentials in
URIs; breaking any existing gate; `git push`.
