# camel-component-mail — CONTEXT

## Scope and architecture

camel-component-mail is the native SMTP and IMAP adapter for rust-camel. `MailBundle` registers four
schemes: `smtp`, `smtps`, `imap` and `imaps`. The producer serves `smtp` and `smtps`. The IMAP
consumer serves `imap` and `imaps`.

`MailComponent` serves one scheme per instance and reads `[components.mail]` accounts. The producer
talks to the `MailTransport` seam. The `lettre` adapter lives under `src/adapter/`, and no `lettre`
type appears in a public signature (ADR-0020, ADR-0060). Unit tests use a fake transport.

## Key design decisions

- Accounts. Host and port live in the URI. Logins and trust anchors live in named accounts under
  `[components.mail.accounts.<name>]`. `resolve_account_name` picks the account: an explicit
  `?account=` must exist, a single account is the default, several accounts need an explicit name,
  and none means no authentication for `smtp`/`smtps`; `imap`/`imaps` require an account. The
  implicit single-account default is an accepted risk with dynamic URIs (ADR-0093).
  A URI with userinfo fails endpoint creation.
- Secrets. The account password is a `SecretString` with a redacting `Debug`. No server URL, host or
  port is put into an error or a log line, so nothing needs `redact_url`. Server-derived error text
  goes through `error::server_text` (200 characters, no control characters); async-imap parse
  errors are reported by a constant.
- TLS posture. STARTTLS is required by default. `smtps` and `imaps` use implicit TLS. Cleartext needs
  `allowCleartext=true`, which also applies to private targets (stricter than ADR-0081). There is no
  verify-off flag. An account `caFile` adds trust anchors. IMAP pins the ring provider; SMTP uses the process-default provider (camel-cli installs ring); the crate reaches
  rustls through the workspace `tokio-rustls` entry. `lettre` 0.11 has no provider injection point
  and takes the process default provider (bd rc-2bofp).
- Header classes. Headers use exact CamelCase names, because the HTTP consumer copies Title-Case
  request headers. Class A (content) is validated and `X-*` passes through. Class B (routing) is
  endpoint-only unless `allowedRecipients` arms the fence. Class C (mailbox mutation) is
  endpoint-only. `SmtpEndpointConfig` holds the parsed endpoint options.
- Body. A text body, a JSON string body (what `set_body` and a DSL `constant` produce) or UTF-8 bytes
  is sent as text; any other body fails the exchange.
- Message id. After an accepted send the producer sets `CamelMailMessageId`.
- No internal retry. The producer never resends after an ambiguous failure past DATA. A route error
  handler decides about a retry of the whole exchange.

See ADR-0093 for the full record.

## Consumer design

- Public surface. `MailBundle`, `MailComponent`, `MailComponentConfig`, `MailAccountConfig`,
  `SecretString`, `MailScheme`, `MailError` and the `headers` constants. Endpoint configs
  (`SmtpEndpointConfig`, `ImapEndpointConfig`), the seams and the health check stay crate-private.
- IMAP consumer. Both consumers talk to the `MailStore` seam, which the `async-imap` adapter under
  `src/adapter/` implements, through one receive pipeline in `src/poll.rs`: `Mailbox::open`
  (connect, capability probe, fail closed), `Cycle::begin` (SELECT, SEARCH UNSEEN, size scan),
  `Cycle::load` (body, size, parse, Message-ID dedup; a typed `Loaded` outcome), `Cycle::failed`
  and `Cycle::delivered` (`add`, then disposition). `MailConsumer` (push) routes each loaded message with `send_and_wait`
  and calls `delivered` only after the route returned `Ok` (at-least-once). `MailPollingConsumer`
  (pull, `pollEnrich`) calls `delivered` right after the fetch (at-most-once).
- Session rule. `MailError::Store` and `Connect` mean the connection is broken (I/O, timeout, lost
  connection, unparsable reply): the session is dropped and the push consumer reconnects under
  `NetworkRetryPolicy` (`mail_reconnect_default`). `MailError::Refused` (tagged `NO`/`BAD`) keeps the
  session: a refused poll retries at the next `delay`. A timeout always drops the session, because
  it abandons a command mid-flight. `MailError::LoginRefused` (tagged `NO`/`BAD` on `LOGIN`) is
  permanent like `CapabilityMissing`. A proven session that breaks on the first command of a poll
  (`Session::went_stale`, an idle drop) reconnects at once without backoff; the pull retries a
  broken cached session once on a fresh connection.
- Bounded redelivery. `maxAttempts` counts failed routes, failed body downloads and failed
  disposition steps that leave the message unseen, per `(uidvalidity, uid)`. The skip set holds
  messages that exceed `maxMessageSize`, hit `maxAttempts` or fail to parse; they stay unseen and are
  not polled again; each skip is counted as `b-prime:mail:skip`. `PollState::begin` prunes both maps
  to the current unseen set and discards them on a UIDVALIDITY change.
- Disposition. `dispose_success` runs in order: `copyTo`, `\Seen`, then `moveTo` (MOVE, else COPY
  plus `delete_uid`) or `delete`. The first failing step stops the sequence and is counted as
  `b-prime:mail:dispose`. `delete` and `moveTo` are mutually exclusive; the URI parser rejects both.
  A failure before `\Seen` leaves the message unseen (redelivery, bounded); a failure after does not.
- Capabilities fail closed. `check_capabilities` runs after every connect. `delete` needs UIDPLUS
  because `delete_uid` is `UID STORE +FLAGS \Deleted` plus `UID EXPUNGE`, never a plain `EXPUNGE`
  that would remove other clients' mail. `moveTo` needs MOVE or UIDPLUS. A missing capability is
  `MailError::CapabilityMissing`, which is permanent: the consumer stops and does not reconnect.
- Idempotency. `idempotentRepository=<name>` resolves against repositories that `MailBundle` holds
  (`with_idempotent_repositories`). The key is the Message-ID: `contains` skips and sets `\Seen`,
  `add` runs as soon as the message is delivered (push: route success; pull: hand-over), before the
  disposition and whatever its outcome, a failed route adds nothing, a read error leaves the
  message untouched (`b-prime:mail:idempotent`), and a message without Message-ID is never
  deduplicated.
- Repository injection. Components cannot reach the core registry (ADR-0023). The `camel-bundles`
  boot block resolves the names `memory` and the configured `[idempotent_repo]` name (or backend)
  and passes them to `MailBundle`. No core change.
- Push and pull divergences. A pull returns one message per call, ends with an error on a repository
  read failure (the push consumer moves on), and only logs skips and failures because a
  `PollingConsumer` has no runtime metrics. `receive(timeout)` bounds connect, scan and load; a
  loaded message is always disposed and returned. `receive(Duration::ZERO)` cannot return mail from
  a real server (connecting needs I/O); only the instant fake store answers within it.
- Health. `MailHealthCheck` is registered at endpoint creation (the only hook with a
  `ComponentContext`) and stays healthy without probing until `create_consumer` attaches a push
  consumer. It then reports the consumer's cached outcome (a refused poll counts as a failure) and
  probes with a bounded connect only when the outcome is stale. A permanent stop is sticky until a
  consumer connects again.
- Example. `examples/mail-example` sends and receives against a local GreenMail fixture.
- Logging caveat. `async-imap` logs raw protocol traffic through `log` at `trace` level, including
  `LOGIN`. `camel-config` caps the `async_imap` target at `info` unless the operator names it; an
  operator must never set it to `trace`.

## Glossary

- **Armed fence:** The recipient fence of an endpoint that sets `allowedRecipients`. Only an armed
  fence accepts class B routing headers, and it rejects any address outside the list. An unarmed
  endpoint rejects the headers.
- **Class A/B/C header:** The three header classes of the mail component. A is content (subject,
  content type, HTML body, `X-*`), B is routing (`CamelMailTo`, `CamelMailCc`, `CamelMailBcc`), and C
  is mailbox mutation (endpoint-only).
