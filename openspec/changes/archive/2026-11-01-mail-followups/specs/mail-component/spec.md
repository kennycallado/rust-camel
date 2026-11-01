## ADDED Requirements

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

## MODIFIED Requirements

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
