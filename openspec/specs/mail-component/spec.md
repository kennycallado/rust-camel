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
`caFile`, optional `allowedHosts`), with `${env:}` placeholders allowed.
Account selection SHALL follow the rabbitmq broker rules: explicit
`?account=` must exist; omitted with exactly one account selects it;
omitted with several is an error naming `?account=`; omitted with none
means no authentication for `smtp`/`smtps`; for `imap`/`imaps` an account
with `username` and `password` is required and its absence SHALL fail
endpoint creation. Userinfo in the URI SHALL fail endpoint creation. URI
options SHALL be declared through the `UriConfig` descriptor.

Implicit single-account selection SHALL be bound to the account's
`allowedHosts` allow-list: the endpoint SHALL be created only when the URI
host exactly matches (case-insensitively) an entry in the selected account's `allowedHosts`. When the list is
absent or empty, when no entry matches, or when the URI carries no host,
implicit selection SHALL fail endpoint creation with that same typed
error (no host can be matched). Explicit `?account=<name>` selection
SHALL ignore `allowedHosts` and may target any host. The failure SHALL be
a typed endpoint-creation error whose message names the account and the
two remedies (`?account=<name>` or `allowedHosts`) and SHALL NOT contain
the host, port or server URL, in the error text or any log line.

#### Scenario: four schemes registered

- **GIVEN** a context booted with the mail bundle enabled
- **WHEN** the component registry is queried
- **THEN** `smtp`, `smtps`, `imap` and `imaps` all resolve

#### Scenario: default ports

- **GIVEN** URIs `smtp://h`, `smtps://h`, `imap://h`, `imaps://h`
- **WHEN** the endpoints resolve
- **THEN** the ports are 587, 465, 143 and 993

#### Scenario: single account resolves implicitly

- **GIVEN** one account `main` with `allowedHosts = ["h"]` and URI
  `smtp://h?from=a@x.test`
- **WHEN** the endpoint resolves
- **THEN** account `main` is selected

#### Scenario: implicit single account without allowed hosts fails

- **GIVEN** one account `main` with no `allowedHosts` and URI
  `smtp://mx.example.com?from=a@x.test`
- **WHEN** the endpoint is created
- **THEN** creation fails with an error naming `main` and both
  `?account=` and `allowedHosts`

#### Scenario: implicit single account host mismatch fails

- **GIVEN** one account `main` with `allowedHosts = ["other.example.com"]`
  and URI `smtp://mx.example.com?from=a@x.test`
- **WHEN** the endpoint is created
- **THEN** creation fails naming both remedies

#### Scenario: implicit single account host match succeeds

- **GIVEN** one account `main` with `allowedHosts = ["mx.example.com"]`
  and URI `smtp://mx.example.com?from=a@x.test`
- **WHEN** the endpoint is created
- **THEN** creation succeeds and account `main` is selected

#### Scenario: allowed hosts match is case insensitive

- **GIVEN** one account `main` with `allowedHosts = ["MX.Example.COM"]`
  and URI `smtp://mx.example.com?from=a@x.test`
- **WHEN** the endpoint is created
- **THEN** creation succeeds

#### Scenario: explicit account ignores allowed hosts

- **GIVEN** one account `a` with an empty `allowedHosts` and URI
  `smtp://mx.example.com?account=a&from=x@x.test`
- **WHEN** the endpoint is created
- **THEN** creation succeeds (documented escape hatch)

#### Scenario: implicit selection without uri host fails

- **GIVEN** one account `main` with `allowedHosts = ["mx.example.com"]`
  and URI `smtp:/x` with no authority
- **WHEN** the endpoint is created
- **THEN** creation fails with the typed endpoint-creation error naming
  `main` and both `?account=` and `allowedHosts`, with no URI error text

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

#### Scenario: host never traverses error or log

- **GIVEN** one account `main` with `allowedHosts = ["other.example.com"]`
  and URI `smtp://mx.example.com?from=a@x.test`
- **WHEN** the endpoint is created with a capture subscriber installed
- **THEN** no captured log line and no error `Display` text contains
  `mx.example.com`

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
`CamelMailContentType`, `X-*`, `CamelMailListUnsubscribe`,
`CamelMailListUnsubscribePost`) that contains CR, LF or NUL, or is longer
than 998 bytes, before any network I/O. `CamelMailHtmlBody` is body
content, not a header line: it SHALL reject NUL only and has no length
rule. `X-*` headers SHALL be emitted as message headers after validation.
`CamelMailListUnsubscribe` and `CamelMailListUnsubscribePost` SHALL be
string-only and SHALL follow the additional URI and pairing rules of the
List-Unsubscribe requirement. Both SHALL be emitted as the standard wire
headers `List-Unsubscribe` and `List-Unsubscribe-Post` after validation.

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

#### Scenario: List-Unsubscribe names are class A

- **GIVEN** exchange headers `CamelMailListUnsubscribe` and
  `CamelMailListUnsubscribePost` with valid values
- **WHEN** the producer sends
- **THEN** the message carries `List-Unsubscribe` and
  `List-Unsubscribe-Post` and no `X-` copy of either

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
HTML alternative exists. Attachments SHALL be ignored unless `raw=true`
is set. With `raw=true` (boolean URI parameter, default `false`) the body
SHALL be `Body::Bytes` holding the exact RFC 822 bytes fetched with
`BODY.PEEK[]`, all `CamelMailIn*` headers SHALL be set except
`CamelMailInHtmlBody`, and the disposition, `maxAttempts`,
`maxMessageSize`, `idempotentRepository`, and at-least-once semantics
SHALL be unchanged; no new retry state SHALL be introduced. `pollEnrich`
SHALL return the same raw mapping when the endpoint URI sets `raw=true`.
Delivery SHALL be at-least-once. The consumer SHALL stop on cancellation,
SHALL reconnect under `NetworkRetryPolicy`, and SHALL NOT restart itself.
A login the server refuses (tagged `NO` or `BAD` on `LOGIN`) SHALL be
permanent: the consumer SHALL stop with an error after one attempt and
SHALL NOT reconnect.

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

#### Scenario: raw body is the exact message bytes

- **GIVEN** `raw=true` and a multipart message with an attachment on a
  fake store
- **WHEN** the consumer polls
- **THEN** the exchange body is `Body::Bytes` equal to the exact RFC 822
  bytes the store served for `BODY.PEEK[]`, and the `CamelMailIn*`
  headers are populated

#### Scenario: raw mode omits the HTML body header

- **GIVEN** `raw=true` and a message with a text and an HTML part
- **WHEN** the consumer polls
- **THEN** no `CamelMailInHtmlBody` header is present and the other
  `CamelMailIn*` headers are populated

#### Scenario: default path ignores attachments unchanged

- **GIVEN** no `raw` parameter and a message with an attachment
- **WHEN** the consumer polls
- **THEN** the body is the text part, no attachment bytes are exposed,
  and `CamelMailInHtmlBody` behaves as before

#### Scenario: failing raw route leaves the message unseen

- **GIVEN** `raw=true` and a route that fails on the message
- **WHEN** the poll completes
- **THEN** the message is left UNSEEN, no disposition marks it seen, and
  the failure counts toward `maxAttempts`

#### Scenario: raw pollEnrich returns raw bytes

- **GIVEN** `pollEnrich` on an `imap` endpoint URI with `raw=true`
- **WHEN** the enrichment runs against one stored message
- **THEN** the enriched body is the exact RFC 822 bytes and no
  `CamelMailInHtmlBody` header is set

### Requirement: Disposition after route

The consumer SHALL set `\Seen` only after the route completes
successfully and SHALL leave the message unmodified after a failed route.
After `maxAttempts` (default 3) failed attempts for the same UIDVALIDITY
and UID (a failed route, a failed body download, or a failed disposition
step that leaves the message unseen), it SHALL skip that message for the
rest of the process lifetime, log a warning, count the skip, and leave it
unmodified on the server. A disposition failure SHALL be logged and
counted with label `b-prime:mail:dispose`. Each failed route attempt,
including the one that reaches `maxAttempts`, SHALL log one warning. That
warning SHALL carry the UID, the UIDVALIDITY, the attempt number of that
attempt, `maxAttempts`, and a redacted error kind taken from
`CamelError::variant_name`. The warning SHALL NOT contain message text,
subject, body, credentials, host, port or URL. A route that closes the
channel SHALL stop the consumer without logging a failed-attempt warning and
without counting an attempt.

#### Scenario: failed route attempt is logged

- **GIVEN** one UNSEEN message, `maxAttempts=3` and a route that fails
- **WHEN** one poll completes
- **THEN** exactly one warning names the UID, `uid_validity=1`, `attempt=1`
  and `max_attempts=3`

#### Scenario: final attempt is logged with its true number

- **GIVEN** `maxAttempts=3` and a route that always fails
- **WHEN** three polls complete
- **THEN** the third failed-attempt warning carries `attempt=3` and
  `max_attempts=3`, and a skip warning is also emitted

#### Scenario: error-bearing success is a failed attempt

- **GIVEN** one UNSEEN message and a route that returns `Ok` after setting an
  error on the exchange
- **WHEN** the poll completes
- **THEN** the attempt counter is 1, the message stays unseen, and one warning
  carries the exchange error's variant name

#### Scenario: logged cause is redacted

- **GIVEN** a failing route whose error text contains a URL, a token and
  message body text
- **WHEN** the poll completes
- **THEN** the captured warning contains the error kind only, and none of the
  URL, the token or the body text

#### Scenario: closed channel is not a failed attempt

- **GIVEN** a route that closes its reply channel without answering
- **WHEN** the poll completes
- **THEN** no failed-attempt warning is logged, no attempt is counted, and the
  consumer stops with `RouteClosed`

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

### Requirement: List-Unsubscribe headers (class A)

The producer SHALL accept exchange headers `CamelMailListUnsubscribe` and
`CamelMailListUnsubscribePost` as class-A content headers and SHALL render
them as the standard wire headers `List-Unsubscribe` and
`List-Unsubscribe-Post`. Both values SHALL be string-only and SHALL be
validated before any network I/O. A value that is not a string, contains
CR, LF or NUL, exceeds 998 bytes, or is not ASCII SHALL fail the exchange
with a header-validation error and nothing SHALL be sent.

`CamelMailListUnsubscribe` SHALL be a comma-separated list of angle-bracketed
URIs. Each element, after ASCII whitespace trimming, SHALL be `<...>` with a
non-empty inner part that contains no whitespace, `<` or `>`. Text outside
the angle brackets SHALL be rejected. EVERY element SHALL begin with
`mailto:` followed by at least one character, or with `https://` followed by
a non-empty host, compared ASCII case-insensitively. The host is the run of
characters after `https://` up to the first `/`, `?` or `#`; it SHALL NOT be
empty. An element with any other scheme SHALL be rejected.

`CamelMailListUnsubscribePost` SHALL equal `List-Unsubscribe=One-Click`
exactly, with no surrounding whitespace and no case folding. It SHALL require
`CamelMailListUnsubscribe` on the same message. When it is present, the
`List-Unsubscribe` value SHALL contain at least one `https://` URI (RFC 8058),
so a `mailto:`-only value paired with the Post header SHALL be rejected and
the error SHALL name `CamelMailListUnsubscribePost`.

Bare standard header names (`List-Unsubscribe`, `List-Unsubscribe-Post`),
case variants, and other non-`X-` non-class-A names on the exchange SHALL
remain ignored. They SHALL NOT set a wire header.

Every rejection SHALL be a header-validation error whose header field is the
Camel name that failed: a list-value rejection names
`CamelMailListUnsubscribe`, a Post rejection names
`CamelMailListUnsubscribePost`.

#### Scenario: both headers rendered

- **GIVEN** `CamelMailListUnsubscribe` =
  `<https://example.com/u/1>, <mailto:u@example.com>` and
  `CamelMailListUnsubscribePost` = `List-Unsubscribe=One-Click`
- **WHEN** the producer sends
- **THEN** the received message carries exactly one `List-Unsubscribe` header
  with that value and exactly one `List-Unsubscribe-Post` header with
  `List-Unsubscribe=One-Click`

#### Scenario: non-string value rejected

- **GIVEN** `CamelMailListUnsubscribe` set to a number
- **WHEN** the producer processes the exchange
- **THEN** the exchange fails with a header-validation error naming
  `CamelMailListUnsubscribe` and nothing is sent

#### Scenario: CR rejected

- **GIVEN** `CamelMailListUnsubscribe` containing a CR
- **WHEN** the producer processes the exchange
- **THEN** the exchange fails with a header-validation error naming
  `CamelMailListUnsubscribe` and nothing is sent

#### Scenario: LF rejected

- **GIVEN** `CamelMailListUnsubscribe` containing an LF
- **WHEN** the producer processes the exchange
- **THEN** the exchange fails with a header-validation error naming
  `CamelMailListUnsubscribe` and nothing is sent

#### Scenario: NUL rejected

- **GIVEN** `CamelMailListUnsubscribePost` containing a NUL byte
- **WHEN** the producer processes the exchange
- **THEN** the exchange fails with a header-validation error naming
  `CamelMailListUnsubscribePost` and nothing is sent

#### Scenario: over-long value rejected

- **GIVEN** `CamelMailListUnsubscribe` of 999 bytes
- **WHEN** the producer processes the exchange
- **THEN** the exchange fails with reason `longer than 998 bytes` and nothing
  is sent

#### Scenario: non-ASCII value rejected

- **GIVEN** `CamelMailListUnsubscribe` = `<mailto:a@b.test?subject=café>`
- **WHEN** the producer processes the exchange
- **THEN** the exchange fails with a header-validation error, because lettre
  would encode the URI as RFC 2047 text, and nothing is sent

#### Scenario: unbracketed URI rejected

- **GIVEN** `CamelMailListUnsubscribe` = `https://example.com/u/1`
- **WHEN** the producer processes the exchange
- **THEN** the exchange fails with a header-validation error and nothing is
  sent

#### Scenario: empty brackets rejected

- **GIVEN** `CamelMailListUnsubscribe` = `<>`
- **WHEN** the producer processes the exchange
- **THEN** the exchange fails with a header-validation error and nothing is
  sent

#### Scenario: element without a supported scheme rejected

- **GIVEN** `CamelMailListUnsubscribe` = `<ftp://example.com/u>`
- **WHEN** the producer processes the exchange
- **THEN** the exchange fails with a header-validation error and nothing is
  sent

#### Scenario: one unsupported element rejects the whole list

- **GIVEN** `CamelMailListUnsubscribe` = `<mailto:a@x.test>, <http://x.test>`
- **WHEN** the producer processes the exchange
- **THEN** the exchange fails with a header-validation error and nothing is
  sent

#### Scenario: https without a host rejected

- **GIVEN** `CamelMailListUnsubscribe` = `<https:///p>`
- **WHEN** the producer processes the exchange
- **THEN** the exchange fails with a header-validation error and nothing is
  sent

#### Scenario: multiple URIs accepted

- **GIVEN** `CamelMailListUnsubscribe` =
  `<mailto:u@example.com>, <https://example.com/u/1>, <https://example.com/u/2>`
- **WHEN** the producer sends
- **THEN** the received `List-Unsubscribe` header carries all three elements

#### Scenario: Post without the unsubscribe header

- **GIVEN** `CamelMailListUnsubscribePost` = `List-Unsubscribe=One-Click` and
  no `CamelMailListUnsubscribe`
- **WHEN** the producer processes the exchange
- **THEN** the exchange fails with a header-validation error naming
  `CamelMailListUnsubscribePost` and nothing is sent

#### Scenario: wrong one-click token

- **GIVEN** `CamelMailListUnsubscribe` with an `https://` URI and
  `CamelMailListUnsubscribePost` = `list-unsubscribe=one-click`
- **WHEN** the producer processes the exchange
- **THEN** the exchange fails with a header-validation error naming
  `CamelMailListUnsubscribePost` and nothing is sent

#### Scenario: token with whitespace rejected

- **GIVEN** `CamelMailListUnsubscribePost` = `List-Unsubscribe=One-Click `
- **WHEN** the producer processes the exchange
- **THEN** the exchange fails with a header-validation error naming
  `CamelMailListUnsubscribePost` and nothing is sent

#### Scenario: mailto-only value cannot pair with Post

- **GIVEN** `CamelMailListUnsubscribe` = `<mailto:u@example.com>` and
  `CamelMailListUnsubscribePost` = `List-Unsubscribe=One-Click`
- **WHEN** the producer processes the exchange
- **THEN** the exchange fails with a header-validation error naming
  `CamelMailListUnsubscribePost` and nothing is sent

#### Scenario: bare standard header ignored

- **GIVEN** exchange headers `List-Unsubscribe` and `List-Unsubscribe-Post`
  with standard values, and no `CamelMail*` counterpart
- **WHEN** the producer sends
- **THEN** the message carries neither standard wire header

#### Scenario: HTTP-copied header cannot set it

- **GIVEN** the HTTP-copied form `Camelmaillistunsubscribe` (answer to a
  Title-Case request header) and the lowercase `list-unsubscribe`
- **WHEN** the producer sends
- **THEN** the message carries no `List-Unsubscribe` wire header

### Requirement: Timeout bounds

Each mail endpoint SHALL accept `connectTimeout` and `ioTimeout` URI
options in milliseconds, reject a zero or empty value at parse time with an
error naming the option, and apply the configured bound to every phase the
underlying library can observe: `connectTimeout` bounds the IMAP connect
sequence (TCP connect, greeting, CAPABILITY, STARTTLS, TLS handshake,
LOGIN) and each SMTP TCP-connect attempt (per resolved address);
`ioTimeout` bounds every IMAP store operation and the SMTP send as one
whole-transaction bound over the conversation from name resolution to the
server's acceptance reply (greeting, EHLO, AUTH, MAIL/DATA); background
connection cleanup (QUIT) is not covered by the bound. A timeout error
SHALL name the phase, the bound in milliseconds and the option that set it.
IMAP timeouts SHALL map to `Connect` or `Store` (retryable, session
dropped); SMTP timeouts SHALL map to `Transport` (non-retryable; the
producer does not resend).

#### Scenario: defaults preserve today's IMAP behaviour

- **GIVEN** an `imap://` (or `imaps://`) URI without either timeout option
- **WHEN** the endpoint is created
- **THEN** the connect bound is 30000 ms and the per-operation bound is
  60000 ms, the constants used before this change

#### Scenario: SMTP defaults

- **GIVEN** an `smtp://` (or `smtps://`) URI without either timeout option
- **WHEN** the endpoint is created
- **THEN** the TCP-connect bound is 60000 ms and the whole-transaction
  bound is 600000 ms

#### Scenario: zero value rejected naming the option

- **GIVEN** a mail URI with `connectTimeout=0` or `ioTimeout=0`
- **WHEN** the endpoint is created
- **THEN** creation fails with an error naming the option

#### Scenario: empty value rejected naming the option

- **GIVEN** a mail URI with `connectTimeout=` or `ioTimeout=`
- **WHEN** the endpoint is created
- **THEN** creation fails with an error naming the option

#### Scenario: configured values are applied

- **GIVEN** a mail URI with `connectTimeout=5000&ioTimeout=10000`
- **WHEN** the endpoint is created
- **THEN** the connect bound is 5000 ms and the I/O bound is 10000 ms

#### Scenario: stalled SMTP server trips ioTimeout with the named error

- **GIVEN** a fake SMTP server that accepts the connection and then never
  replies, and an endpoint with `ioTimeout=500`
- **WHEN** the producer sends
- **THEN** the send fails with an error naming `ioTimeout`, the 500 ms
  bound and the send phase, before the test's own outer deadline

#### Scenario: stalled IMAP operation trips ioTimeout with the named error

- **GIVEN** a fake IMAP server that answers LOGIN and then goes silent, and
  an endpoint with `ioTimeout=500`
- **WHEN** the consumer runs a store operation
- **THEN** the operation fails with an error naming `ioTimeout`, the
  500 ms bound and the operation phase

#### Scenario: stalled IMAP connect trips connectTimeout with the named error

- **GIVEN** a fake IMAP server that accepts the connection and sends no
  greeting, and an endpoint with `connectTimeout=500`
- **WHEN** the consumer connects
- **THEN** the connect fails with an error naming `connectTimeout` and the
  500 ms bound

#### Scenario: a timed-out SMTP send does not poison the next send

- **GIVEN** one fake SMTP server whose first connection reaches the DATA
  phase, accepts the data and then never replies, so the send times out
  through `ioTimeout`, while its later connections are served normally
- **WHEN** a second send runs through the same transport to the same
  endpoint address
- **THEN** the second send succeeds, the server saw a fresh second
  connection and the full second message

