## MODIFIED Requirements

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
