# mail-component Specification

## Purpose
TBD - created by archiving change mail-component. Update Purpose after archive.
## Requirements
### Requirement: Scheme registration, URI shape and accounts

The component SHALL register schemes `smtp`, `smtps`, `imap` and `imaps`
from one `MailBundle` (`config_key() == "mail"`). The URI shape SHALL be
`scheme://host[:port]?options`; omitted ports default to 587 (`smtp`),
465 (`smtps`), 143 (`imap`) and 993 (`imaps`). Credentials and trust
anchors SHALL come from named accounts in
`[components.mail.accounts.<name>]` (`username`, `password`, optional
`caFile`), with `${env:}` placeholders allowed. Account selection SHALL
follow the rabbitmq broker rules: explicit `?account=` must exist;
omitted with exactly one account selects it; omitted with several is an
error naming `?account=`; omitted with none means no authentication for `smtp`/`smtps`; for `imap`/`imaps` an account with `username` and `password` is required and its absence SHALL fail endpoint creation.
Userinfo in the URI SHALL fail endpoint creation. URI options SHALL be
declared through the `UriConfig` descriptor.

#### Scenario: four schemes registered

- **GIVEN** a context booted with the mail bundle enabled
- **WHEN** the component registry is queried
- **THEN** `smtp`, `smtps`, `imap` and `imaps` all resolve

#### Scenario: default ports

- **GIVEN** URIs `smtp://h`, `smtps://h`, `imap://h`, `imaps://h`
- **WHEN** the endpoints resolve
- **THEN** the ports are 587, 465, 143 and 993

#### Scenario: single account resolves implicitly

- **GIVEN** one account `main` and URI `smtp://h?from=a@x.test`
- **WHEN** the endpoint resolves
- **THEN** account `main` is selected

#### Scenario: ambiguous account selection errors

- **GIVEN** two accounts and a URI without `?account=`
- **WHEN** the endpoint resolves
- **THEN** resolution fails with an error naming `?account=`

#### Scenario: credentials in the URI are rejected

- **GIVEN** URI `smtp://user:secret@mail.example:587`
- **WHEN** the endpoint is created
- **THEN** creation fails with an error naming `[components.mail.accounts]`
  and the error text does not contain `secret`

#### Scenario: password never traverses Debug

- **GIVEN** an account with password `hunter2`
- **WHEN** the config is formatted with `{:?}`
- **THEN** the output does not contain `hunter2`

### Requirement: TLS posture

The component SHALL require STARTTLS on `smtp` and `imap` by default,
SHALL use implicit TLS on `smtps` and `imaps`, and SHALL use cleartext
only when the endpoint sets `allowCleartext=true`. Server certificates
SHALL verify against the webpki roots plus the account's `caFile`;
there SHALL be no option that disables verification. TLS SHALL use the
rustls ring provider; the component's own dependencies SHALL NOT enable
an aws-lc backend.

#### Scenario: smtp server without STARTTLS is refused by default

- **GIVEN** an `smtp` endpoint without `allowCleartext` and a server
  that does not advertise STARTTLS
- **WHEN** a message is sent
- **THEN** the send fails before authentication and no credential is
  transmitted

#### Scenario: imap server without STARTTLS is refused by default

- **GIVEN** an `imap` endpoint without `allowCleartext` and a server
  that does not advertise STARTTLS
- **WHEN** the consumer connects
- **THEN** the connect fails before LOGIN and no credential is transmitted

#### Scenario: implicit TLS

- **GIVEN** an `smtps` endpoint and a fake server that speaks TLS from
  the first byte
- **WHEN** a message is sent
- **THEN** the send succeeds with no STARTTLS command

#### Scenario: allowCleartext on an implicit-TLS scheme

- **GIVEN** URI `smtps://h?allowCleartext=true`
- **WHEN** the endpoint is created
- **THEN** creation fails naming `allowCleartext`

#### Scenario: explicit cleartext opt-in

- **GIVEN** an `smtp` endpoint with `allowCleartext=true`
- **WHEN** a message is sent to a plain server
- **THEN** the send succeeds

#### Scenario: private CA via caFile

- **GIVEN** a server certificate signed by a test CA listed in the
  account's `caFile`
- **WHEN** a message is sent over STARTTLS
- **THEN** the send succeeds; with the `caFile` removed it fails with a
  certificate-verification error

#### Scenario: ring-only TLS stack

- **WHEN** the component's manifest and the workspace entries of its TLS
  dependencies are inspected
- **THEN** no feature selected by the component's own dependency
  declarations enables `aws_lc_rs`, `aws-lc-rs` or rustls `default`

### Requirement: SMTP producer message construction

The producer SHALL build a MIME message from the exchange. The body SHALL
be a `String` or bytes that decode as UTF-8; any other body type or
invalid UTF-8 SHALL fail the exchange. The content type SHALL come from
header `CamelMailContentType` or option `contentType` (`text/plain` by
default, or `text/html`). When the body is `text/plain` and header
`CamelMailHtmlBody` is set, the message SHALL be `multipart/alternative`
with the text part and the HTML part. The producer SHALL set header
`CamelMailMessageId` after acceptance and SHALL use a pooled transport.

#### Scenario: text body

- **GIVEN** a string body `hello` and a fake SMTP server
- **WHEN** the producer sends
- **THEN** the server receives one `text/plain` UTF-8 message with body `hello`

#### Scenario: html body

- **GIVEN** a string body `<b>x</b>` and `contentType=text/html`
- **WHEN** the producer sends
- **THEN** the received message is `text/html`

#### Scenario: alternative body

- **GIVEN** a text body and header `CamelMailHtmlBody` = `<b>x</b>`
- **WHEN** the producer sends
- **THEN** the received message is `multipart/alternative` with a
  `text/plain` part and a `text/html` part

#### Scenario: html content type with html alternative

- **GIVEN** `contentType=text/html` and header `CamelMailHtmlBody`
- **WHEN** the producer processes the exchange
- **THEN** the exchange fails with a conflicting-body error and nothing is sent

#### Scenario: unsupported body type

- **GIVEN** a JSON-object body
- **WHEN** the producer processes the exchange
- **THEN** the exchange fails with a body-type error and no connection opens

#### Scenario: message id exposed

- **WHEN** the server accepts a message
- **THEN** the exchange carries a non-empty `CamelMailMessageId`

### Requirement: Addressing and header precedence

The endpoint SHALL require `from` at creation. The send SHALL require at
least one recipient from the endpoint or from fenced headers, else the
exchange fails. Header `CamelMailSubject` SHALL override option
`subject`. When the fence is armed, header recipients SHALL REPLACE the
endpoint's `to`/`cc`/`bcc` of the same kind; recipients configured on the
endpoint SHALL NOT be subject to `allowedRecipients`. `From`, `Sender` and
`Reply-To` SHALL come from the endpoint only; exchange headers named
`From`, `Sender` or `Reply-To` SHALL be ignored.

#### Scenario: missing from

- **GIVEN** URI `smtp://h?to=a@x.test` without `from`
- **WHEN** the endpoint is created
- **THEN** creation fails naming `from`

#### Scenario: no recipient

- **GIVEN** an endpoint without `to`, `cc`, `bcc` and no routing headers
- **WHEN** the producer processes the exchange
- **THEN** the exchange fails with a no-recipient error and nothing is sent

#### Scenario: subject header overrides option

- **GIVEN** `subject=a` and header `CamelMailSubject` = `b`
- **WHEN** the producer sends
- **THEN** the received subject is `b`

#### Scenario: spoofed From header ignored

- **GIVEN** header `From` = `ceo@corp.test` and endpoint `from=bot@corp.test`
- **WHEN** the producer sends
- **THEN** the message `From` is `bot@corp.test`

#### Scenario: header recipients replace endpoint recipients

- **GIVEN** `to=a@corp.test`, `allowedRecipients=@corp.test`, header
  `CamelMailTo` = `b@corp.test`
- **WHEN** the producer sends
- **THEN** the envelope recipient list is exactly `b@corp.test`

### Requirement: Content header validation (class A)

The producer SHALL reject any class-A header value (`CamelMailSubject`,
`CamelMailContentType`, `X-*`) that contains CR, LF or NUL, or is longer
than 998 bytes, before any network I/O. `CamelMailHtmlBody` is body
content, not a header line: it SHALL reject NUL only and has no length
rule. `X-*` headers SHALL be emitted as message headers after validation.

#### Scenario: CRLF injection rejected

- **GIVEN** header `CamelMailSubject` = `hi\r\nBcc: evil@x.test`
- **WHEN** the producer processes the exchange
- **THEN** the exchange fails with a header-validation error and the
  fake server saw no connection

#### Scenario: NUL rejected

- **GIVEN** header `X-Trace` containing a NUL byte
- **WHEN** the producer processes the exchange
- **THEN** the exchange fails and nothing is sent

#### Scenario: over-long value rejected

- **GIVEN** header `CamelMailSubject` of 999 bytes
- **WHEN** the producer processes the exchange
- **THEN** the exchange fails and nothing is sent

#### Scenario: valid X header emitted

- **GIVEN** header `X-Trace` = `abc`
- **WHEN** the producer sends
- **THEN** the received message carries `X-Trace: abc`

### Requirement: Recipient fence (class B)

Routing headers `CamelMailTo`, `CamelMailCc` and `CamelMailBcc` SHALL be
honored only when the endpoint sets `allowedRecipients`. On an unarmed
endpoint their presence SHALL fail the exchange. On an armed endpoint
every address, parsed as a mailbox, SHALL match an exact address or an
`@domain` entry (domain match case-insensitive), else the exchange fails.
The total recipient count SHALL NOT exceed `maxRecipients` (default 50).
Bcc recipients SHALL appear in the envelope only. Malformed
`allowedRecipients` entries SHALL fail endpoint creation. Header names
SHALL match exactly in CamelCase; Title-Case `To`/`Cc`/`Bcc` SHALL have
no routing effect.

#### Scenario: routing header on unarmed endpoint

- **GIVEN** an endpoint without `allowedRecipients` and header
  `CamelMailTo` = `a@x.test`
- **WHEN** the producer processes the exchange
- **THEN** the exchange fails and nothing is sent

#### Scenario: address outside the allowlist

- **GIVEN** `allowedRecipients=@corp.test` and `CamelMailTo` = `a@evil.test`
- **WHEN** the producer processes the exchange
- **THEN** the exchange fails and nothing is sent

#### Scenario: allowed domain matches case-insensitively

- **GIVEN** `allowedRecipients=@Corp.Test` and `CamelMailTo` = `a@corp.test`
- **WHEN** the producer processes the exchange
- **THEN** the message is sent

#### Scenario: Bcc is envelope-only

- **GIVEN** an armed endpoint and `CamelMailBcc` = `b@corp.test`
- **WHEN** the message is sent
- **THEN** the SMTP envelope contains `b@corp.test` and the message
  headers do not

#### Scenario: recipient cap

- **GIVEN** `maxRecipients=2` and three allowed recipients
- **WHEN** the producer processes the exchange
- **THEN** the exchange fails and nothing is sent

#### Scenario: malformed allowlist entry

- **GIVEN** `allowedRecipients=not-an-address`
- **WHEN** the endpoint is created
- **THEN** creation fails

#### Scenario: Title-Case header has no effect

- **GIVEN** an unarmed endpoint and header `To` = `a@x.test`
- **WHEN** the producer sends to the endpoint's own recipient
- **THEN** the envelope contains only the endpoint's recipient

### Requirement: Send semantics

The producer SHALL NOT resend a message after an ambiguous transport
failure past the DATA command, and SHALL have no internal retry. A
failure before DATA is returned to the route, whose error handler MAY
retry the whole exchange.

#### Scenario: failure after DATA is not resent

- **GIVEN** a fake server that drops the connection after DATA, and a
  route with no error-handler retry
- **WHEN** the producer sends
- **THEN** the exchange fails and the server saw exactly one DATA

#### Scenario: failure before DATA is a plain error

- **GIVEN** a fake server that rejects `RCPT TO` with 550
- **WHEN** the producer sends
- **THEN** the exchange fails with the server reply and the server saw
  no DATA

### Requirement: Integration tier gating

Tests that need Docker SHALL run only when `MAIL_ITEST=1`. With the gate
set and Docker unavailable they SHALL fail with an `infra-unavailable`
message and SHALL NOT skip silently.

#### Scenario: gate on, docker absent

- **GIVEN** `MAIL_ITEST=1` and no Docker daemon
- **WHEN** the tier runs
- **THEN** it fails with `infra-unavailable`

#### Scenario: gate off prints notice

- **GIVEN** `MAIL_ITEST` unset
- **WHEN** the tier runs
- **THEN** the tests do not run and print a notice naming `MAIL_ITEST=1`

### Requirement: IMAP consumer receive

The consumer SHALL poll the configured `folder` (default `INBOX`) every
`delay` after `initialDelay`, search `UNSEEN`, fetch `RFC822.SIZE` for
the candidates, skip every message larger than `maxMessageSize` before
fetching any body, then fetch up to `maxMessagesPerPoll` of the remaining
messages with `BODY.PEEK[]` and process them sequentially. It SHALL map a
message to an exchange whose body is the `text/plain` part (the HTML
source when there is no text part) and whose headers are
`CamelMailInFrom`, `CamelMailInTo`, `CamelMailInCc`,
`CamelMailInSubject`, `CamelMailInMessageId`, `CamelMailInUid`,
`CamelMailInDate`, `CamelMailInFolder`, plus `CamelMailInHtmlBody` when an
HTML alternative exists. Attachments SHALL be ignored. Delivery SHALL be
at-least-once. The consumer SHALL stop on cancellation, SHALL reconnect
under `NetworkRetryPolicy`, and SHALL NOT restart itself. A login the
server refuses (tagged `NO` or `BAD` on `LOGIN`) SHALL be permanent: the
consumer SHALL stop with an error after one attempt and SHALL NOT
reconnect.

#### Scenario: mapping

- **GIVEN** a multipart/alternative message with a text and an HTML part
- **WHEN** the consumer polls
- **THEN** the exchange body is the text part, `CamelMailInHtmlBody` is
  the HTML part, and the other `CamelMailIn*` headers are populated

#### Scenario: no duplicate after success

- **GIVEN** one UNSEEN message and a succeeding route
- **WHEN** two polls run
- **THEN** the route receives the message once

#### Scenario: oversize skipped before body fetch

- **GIVEN** `maxMessageSize=1024` and a 4 KiB message on a fake store
- **WHEN** the poll runs
- **THEN** the store never saw a body fetch for it, the route did not
  receive it, a warning and a skip metric were emitted, and the message
  is unmodified

#### Scenario: skipped messages do not starve new mail

- **GIVEN** `maxMessagesPerPoll=1`, an oversize message first in UID
  order, and a normal message after it
- **WHEN** the poll runs
- **THEN** the route receives the normal message

#### Scenario: cancellation stops polling

- **WHEN** the context is cancelled during a poll wait
- **THEN** the consumer returns without another poll

#### Scenario: reconnect after connection loss

- **GIVEN** a fake store whose first connect fails and second succeeds
- **WHEN** the consumer starts under a paused clock
- **THEN** it reconnects after the policy's initial delay and polls

#### Scenario: refused login stops the consumer

- **GIVEN** a server that answers `LOGIN` with a tagged `NO`
- **WHEN** the consumer starts
- **THEN** start returns an error naming the refused login, the server saw
  exactly one `LOGIN`, and the error text carries no server text or
  credential

#### Scenario: inbound headers cannot re-target

- **GIVEN** a mail-to-mail route
- **WHEN** a received message reaches the mail producer
- **THEN** the exchange carries `CamelMailIn*` headers only, so no class
  B or C header is present

### Requirement: Disposition after route

The consumer SHALL set `\Seen` only after the route completes
successfully and SHALL leave the message unmodified after a failed route.
After `maxAttempts` (default 3) failed attempts for the same UIDVALIDITY
and UID (a failed route, a failed body download, or a failed disposition
step that leaves the message unseen), it SHALL skip that message for the
rest of the process lifetime, log a warning, count the skip, and leave it
unmodified on the server. A disposition failure SHALL be logged and
counted with label `b-prime:mail:dispose`.

#### Scenario: refused disposition redelivery is bounded

- **GIVEN** `copyTo=Missing`, a server that refuses the COPY,
  `maxAttempts=2` and a succeeding route
- **WHEN** four polls run
- **THEN** the route received the message exactly twice, the failures
  are counted with `b-prime:mail:dispose`, a skip is counted, and the
  message is still unseen

#### Scenario: success sets Seen

- **GIVEN** one UNSEEN message and a succeeding route
- **WHEN** the poll completes
- **THEN** the store received `\Seen` for that UID

#### Scenario: failed route leaves message untouched

- **GIVEN** one UNSEEN message and a failing route
- **WHEN** the poll completes
- **THEN** the message has no `\Seen` flag

#### Scenario: poison message bounded

- **GIVEN** `maxAttempts=2` and a route that always fails
- **WHEN** four polls run
- **THEN** the route received the message exactly twice, a skip warning
  was emitted, and the message is unmodified

#### Scenario: disposition failure counted

- **GIVEN** a store that fails the `\Seen` STORE
- **WHEN** the route succeeded
- **THEN** the error is logged and counted with `b-prime:mail:dispose`

### Requirement: Health and metrics

The component SHALL emit
`camel_component_operations_total{component="mail",operation,outcome}`
for `send` and `poll`, and SHALL expose an `AsyncHealthCheck` per IMAP
consumer that reports Unhealthy while disconnected, with a 5 s probe
timeout. A consumer that stopped for good SHALL keep the check Unhealthy;
an endpoint without a push consumer SHALL NOT be probed.

#### Scenario: send outcome counted

- **WHEN** a send succeeds and another fails
- **THEN** the counter shows one `success` and one `failure` outcome for `send` (the labels the shared component metrics code emits)

#### Scenario: disconnected consumer unhealthy

- **GIVEN** an IMAP consumer whose fake store cannot connect
- **WHEN** the health check runs
- **THEN** it reports Unhealthy within the probe timeout

### Requirement: Mailbox disposition options

The consumer SHALL support `delete`, `moveTo` and `copyTo`, each applied
only after a successful route. `delete` SHALL use `UID EXPUNGE` and SHALL
require the UIDPLUS capability; without it the consumer SHALL fail to
connect with an error naming UIDPLUS and SHALL NOT issue a plain EXPUNGE.
`moveTo` SHALL use MOVE, else COPY then delete by UID. `moveTo` on a server with neither MOVE nor UIDPLUS SHALL fail closed
with an error naming the missing capability. `delete` and
`moveTo` together SHALL fail endpoint creation; `copyTo` MAY combine with
either and copies first. Folder names SHALL be validated at endpoint
creation (non-empty, no CR, LF or NUL). Disposition options SHALL be set
only on the endpoint (class C) and SHALL NOT be overridable by headers.
The consumer SHALL track UIDVALIDITY and discard cached UIDs when it
changes.

#### Scenario: delete with UIDPLUS

- **GIVEN** `delete=true` on a UIDPLUS server and a succeeding route
- **WHEN** the message is disposed
- **THEN** the server received `UID EXPUNGE` for that UID only

#### Scenario: delete without UIDPLUS fails closed

- **GIVEN** `delete=true` on a server without UIDPLUS
- **WHEN** the consumer connects
- **THEN** it fails with an error naming UIDPLUS and no message is
  fetched or expunged

#### Scenario: move with MOVE

- **GIVEN** `moveTo=Done` on a server with MOVE
- **WHEN** the message is disposed
- **THEN** the server received `UID MOVE` to `Done`

#### Scenario: move without MOVE capability

- **GIVEN** `moveTo=Done` on a server without MOVE
- **WHEN** the message is disposed
- **THEN** the message is copied to `Done` and then deleted by UID

#### Scenario: move without MOVE or UIDPLUS fails closed

- **GIVEN** `moveTo=Done` on a server with neither MOVE nor UIDPLUS
- **WHEN** the consumer connects
- **THEN** it fails with an error naming MOVE and UIDPLUS, and no message
  is fetched or modified

#### Scenario: copyTo copies first

- **GIVEN** `copyTo=Archive` and `moveTo=Done`
- **WHEN** the message is disposed
- **THEN** the server saw COPY to `Archive` before MOVE to `Done`

#### Scenario: delete with moveTo rejected

- **GIVEN** `delete=true&moveTo=Done`
- **WHEN** the endpoint is created
- **THEN** creation fails

#### Scenario: invalid folder name

- **GIVEN** `moveTo=` (empty) or a folder containing LF
- **WHEN** the endpoint is created
- **THEN** creation fails

#### Scenario: UIDVALIDITY change

- **GIVEN** a UIDVALIDITY value that differs from the last poll
- **WHEN** the next poll runs
- **THEN** the attempt counters and the skip set are discarded and no
  stale-UID command is issued

#### Scenario: header cannot re-target disposition

- **GIVEN** a header named `CamelMailMoveTo`
- **WHEN** the consumer's route completes
- **THEN** the header is ignored

### Requirement: Message-ID idempotency and pull consumption

With `idempotentRepository` set, the consumer SHALL skip a message whose
Message-ID `contains` returns true (ADR-0023), SHALL mark the skipped
message `\Seen`, SHALL `add` the Message-ID once the message is delivered
(push: after the route succeeded; pull: when the exchange is handed to the
caller), before and independent of the disposition outcome, SHALL NOT
`add` after a failed route, and SHALL propagate a `contains` error without
treating it as "not a duplicate". A message without a Message-ID SHALL be
processed and never deduplicated. The endpoint SHALL provide a
`PollingConsumer` whose `receive` returns one message and applies the
success disposition immediately after the fetch (a pull has no
route-completion signal). The `receive` timeout SHALL bound connecting,
scanning and loading; once a message is loaded, its disposition SHALL run
to completion and the message SHALL be returned, even when the timeout
expires meanwhile.

#### Scenario: duplicate Message-ID skipped

- **GIVEN** two messages with the same Message-ID
- **WHEN** the consumer polls
- **THEN** the route receives one exchange and the second is marked `\Seen`

#### Scenario: repository read failure is not a duplicate verdict

- **GIVEN** a repository whose `contains` returns an error
- **WHEN** the consumer polls
- **THEN** the message is not delivered, is left unmodified, and the
  error is reported, and nothing is added to the repository

#### Scenario: Message-ID recorded after route success

- **GIVEN** a repository, a succeeding route and a store that refuses
  the `\Seen` STORE
- **WHEN** two polls run
- **THEN** the Message-ID is added after the first route success, the
  disposition failure is counted, and the route receives the message once

#### Scenario: failed route records nothing

- **GIVEN** a repository and a failing route
- **WHEN** the consumer polls
- **THEN** nothing is added to the repository

#### Scenario: message without Message-ID

- **GIVEN** a message with no Message-ID and a repository
- **WHEN** the consumer polls
- **THEN** the route receives it and nothing is added to the repository

#### Scenario: pollEnrich receives one message

- **GIVEN** a mailbox with one UNSEEN message
- **WHEN** `pollEnrich` calls the polling consumer
- **THEN** one exchange with that message is returned, the message is
  `\Seen`, and the next call returns none within the timeout

#### Scenario: pull deadline never drops a loaded message

- **GIVEN** a `pollEnrich` timeout of 1 s and a `\Seen` STORE that takes
  10 s
- **WHEN** the polling consumer is called
- **THEN** the exchange is returned and the message is `\Seen`; when the
  body download instead outlasts the timeout, the call returns none and
  the message is untouched

