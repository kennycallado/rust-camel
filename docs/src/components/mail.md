# Mail (SMTP and IMAP)

The mail component sends mail over SMTP and receives mail over IMAP. It serves the schemes `smtp` (STARTTLS, port 587), `smtps` (implicit TLS, port 465), `imap` (STARTTLS, port 143) and `imaps` (implicit TLS, port 993). The Producer builds a MIME message from the Exchange body and sends it. The Consumer polls an IMAP folder and routes each unread message. The component is native Rust and does not use a Java bridge.

## Example

The mail-example sends one mail every 10 seconds and logs each mail it receives, against a local GreenMail fixture:

```rust,ignore
{{#include ../../../examples/mail-example/src/main.rs:mail-producer-route}}
```

```rust,ignore
{{#include ../../../examples/mail-example/src/main.rs:mail-consumer-route}}
```

The fixture listens on plain loopback ports, so both URIs set `allowCleartext=true`. Run it with `cargo run -p mail-example`; the example README has the Docker command for the fixture.

## Accounts

Host and port go in the URI. Logins and trust anchors go in named accounts in `Camel.toml`. Credentials never go in the URI; a URI with `user:pw@host` fails at endpoint creation.

```toml
[components.mail.accounts.main]
username = "robot@example.com"
password = "${env:MAIL_PASSWORD}"
# caFile = "/etc/ssl/mail-ca.pem"
```

Select an account with `?account=<name>`. With exactly one account, every endpoint without `?account=` uses it, so its credentials are sent to whatever host the URI names; keep the account selection explicit when URIs are built dynamically. With several accounts, `?account=` is required. With no account, `smtp` and `smtps` send without authentication; `imap` and `imaps` need an account and fail at endpoint creation without one.

## Sending mail (smtp, smtps)

```yaml
routes:
  - id: notify
    from: "timer:tick?period=3600000"
    steps:
      - set_body:
          value: "hello"
      - to: "smtp://mail.example.com?from=robot@example.com&to=ops@example.com&subject=Report"
```

| Option | Default | Description |
|--------|---------|-------------|
| `account` | none | Account name from `[components.mail.accounts]`. |
| `from` | required | Sender address. Set on the endpoint only. |
| `to` | none | Recipient list, comma separated. |
| `cc` | none | Copy recipients. |
| `bcc` | none | Blind copy recipients. Sent in the envelope only. |
| `subject` | none | Subject line. |
| `contentType` | `text/plain` | Body type: `text/plain` or `text/html`. |
| `allowedRecipients` | none | Exact addresses or `@domain` entries. Arms the recipient fence. |
| `maxRecipients` | `50` | Maximum total recipient count. |
| `allowCleartext` | `false` | Allow a server without TLS. Not valid on `smtps`. |

The body must be a string (a text body, or a JSON string such as the one `set_body` and a DSL `constant` produce) or UTF-8 bytes. Other JSON values, XML and an empty body fail the Exchange. The send needs a `from` address and at least one recipient. After an accepted send, the Producer sets the header `CamelMailMessageId`. The Producer does not retry. It never resends a message after an ambiguous failure past the SMTP DATA command.

## Receiving mail (imap, imaps)

```yaml
routes:
  - id: inbox
    from: "imap://mail.example.com?folder=INBOX&delay=30000"
    steps:
      - log: "${header.CamelMailInSubject}"
```

The Consumer polls the folder for unread messages. It routes each message as one Exchange. The body is the plain-text part, or the HTML part when the message has no plain-text part.

| Option | Default | Description |
|--------|---------|-------------|
| `account` | none | Account name from `[components.mail.accounts]`. |
| `allowCleartext` | `false` | Allow a server without TLS. Not valid on `imaps`. |
| `folder` | `INBOX` | Folder to poll. |
| `delay` | `60000` | Milliseconds between polls. |
| `initialDelay` | `1000` | Milliseconds before the first poll. |
| `maxMessagesPerPoll` | `100` | Maximum messages whose body is fetched in one poll. |
| `maxMessageSize` | `10485760` | Maximum message size in bytes. Larger messages are skipped. |
| `maxAttempts` | `3` | Failed attempts (route, body download, or a disposition that leaves the message unseen) before the message is skipped. |
| `delete` | `false` | Delete the message after a successful route. Needs UIDPLUS. |
| `moveTo` | none | Move the message to this folder after a successful route. |
| `copyTo` | none | Copy the message to this folder before it is marked `\Seen`. |
| `idempotentRepository` | none | Name of a registered idempotent repository for Message-ID duplicate suppression. |

`delay`, `maxMessagesPerPoll`, `maxMessageSize` and `maxAttempts` must be at least 1. `initialDelay` may be 0. `folder`, `moveTo` and `copyTo` must not be empty or contain CR, LF or NUL. The component sends a folder name to the server as given, so a folder with non-ASCII characters needs the modified UTF-7 name that the server lists.

Each Exchange carries these headers. A header is absent when the message does not have the value.

| Header | Value |
|--------|-------|
| `CamelMailInFrom` | From addresses, comma separated. |
| `CamelMailInTo` | To addresses, comma separated. |
| `CamelMailInCc` | Cc addresses, comma separated. |
| `CamelMailInSubject` | Subject line. |
| `CamelMailInMessageId` | Message-ID without the angle brackets. |
| `CamelMailInUid` | IMAP UID of the message in the folder. |
| `CamelMailInDate` | Date header in RFC 3339 format. |
| `CamelMailInFolder` | Folder the message came from. |
| `CamelMailInHtmlBody` | HTML part, set only when the message has both a text and an HTML part. |

Delivery is at-least-once. The Consumer sets the `\Seen` flag only after the route finishes without an error. A failed route leaves the message unseen, and the next poll delivers it again. If the route succeeds but setting `\Seen` fails, the message is delivered again as well. Make the route idempotent, for example by keying on `CamelMailInMessageId`.

The Consumer checks the size of every unread message before it downloads the body. A message larger than `maxMessageSize` is skipped. A message that fails `maxAttempts` times, and a message that cannot be parsed, are skipped too. A failed attempt is a failed route, a failed body download, or a failed disposition step that leaves the message unseen (see [Disposition](#disposition)). The last case bounds how often a message whose route succeeds is delivered again, for example when `copyTo` names a folder that does not exist. A skipped message stays unseen in the mailbox, and the Consumer does not poll it again while it stays unseen, until the process restarts. Each skip is logged at `warn` level and counted in the error metric with the label `b-prime:mail:skip`. The skip set and the attempt counters only hold messages that are still unseen in the folder, so their size never exceeds the folder's unseen count.

A message that another client removes between the search and the body download is ignored for that poll. It is not a failed attempt.

When the server refuses a poll command with `NO` or `BAD` (for example a `folder` that does not exist), the Consumer keeps the connection, logs a warning, reports the health check as unhealthy and tries again at the next poll. When the server refuses the login (wrong credentials), the Consumer stops with an error and does not retry; fix the account and restart the route. When the connection itself fails (I/O error, timeout, lost connection), the Consumer drops it and reconnects with backoff. A connection that the server closed while the Consumer waited between polls is reopened at once, without a backoff step. The default policy has no attempt limit and waits 5 s, doubling up to 60 s. Change it in `Camel.toml`:

```toml
[components.mail.reconnect]
max_attempts = 0
initial_delay_ms = 5000
max_delay_ms = 60000
multiplier = 2.0
```

Never enable `trace` logging for the `async_imap` target. That library logs the raw protocol traffic at `trace` level, and the traffic includes the `LOGIN` command with the password. The log filter caps `async_imap` at `info` by default, also under `RUST_LOG=trace`; a directive that names `async_imap` replaces the cap, so never set it to `trace`.

A failed `\Seen` update after a successful route makes the Consumer deliver the same message again on the next poll, up to `maxAttempts` deliveries, unless `idempotentRepository` is set (see [Duplicate suppression](#duplicate-suppression)). Each failure is counted with the label `b-prime:mail:dispose`; alert on that counter.

An IMAP endpoint registers a health check named `mail`. It reports the outcome of the latest poll connection. When that outcome is stale, it opens a short probe connection.

## Disposition

After a route finishes without an error, the Consumer runs these steps in order. The first failing step stops the sequence.

1. `copyTo`: copy the message to the target folder.
2. Set the `\Seen` flag. This step always runs.
3. `moveTo`: move the message to the target folder.
4. `delete`: delete the message.

`delete` and `moveTo` exclude each other; the endpoint fails at creation when both are set. `copyTo` combines with either. All four options are class C options. They are endpoint-only, and no Exchange header can set them.

The Consumer checks the server capabilities when it connects and fails closed:

- `delete=true` needs UIDPLUS. The Consumer deletes with `UID STORE +FLAGS \Deleted` followed by `UID EXPUNGE` for that UID only. A plain `EXPUNGE` would also remove messages that other clients marked `\Deleted`, so the Consumer refuses to run without UIDPLUS.
- `moveTo` uses the IMAP `MOVE` command when the server offers it. Without `MOVE`, it copies the message and then deletes the original with `UID STORE` and `UID EXPUNGE`. That fallback needs UIDPLUS. A server with neither `MOVE` nor UIDPLUS is refused.

A capability error is permanent. The Consumer logs it and stops; it does not reconnect. A failure in a disposition step is logged at `warn` level and counted with the label `b-prime:mail:dispose`. If step 1 or 2 fails, the message stays unseen and a later poll delivers it again, up to `maxAttempts` deliveries in total; a `copyTo` that already succeeded can then create a second copy. If step 3 or 4 fails, the message is already `\Seen`, so no later poll fetches it again. It stays in the source folder, and in the `moveTo` fallback a copy already exists in the target folder. A disposition step that fails because the connection broke also makes the Consumer reconnect.

## UIDVALIDITY

IMAP identifies a message by the pair UIDVALIDITY and UID. The Consumer keeps its attempt counters and its skip set under that pair, also across reconnects. When the server reports a new UIDVALIDITY for the folder, the old UIDs mean nothing. The Consumer discards the counters and the skip set and logs the change at `info` level.

## Duplicate suppression

Set `idempotentRepository` to the name of an idempotent repository. The mail component cannot reach the core registry. The `camel-bundles` boot block hands it two kinds of names: `memory`, and the name configured in `[idempotent_repo]` (its `name` key, or the backend name when `name` is not set). An unknown name fails at endpoint creation, and the error lists the available names.

```toml
[idempotent_repo]
backend = "redb"
name = "mail-seen"
```

```yaml
routes:
  - id: inbox
    from: "imap://mail.example.com?idempotentRepository=mail-seen"
    steps:
      - log: "${header.CamelMailInSubject}"
```

The key is the Message-ID. For each unread message the Consumer does this:

- The repository `contains` the Message-ID: the Consumer sets `\Seen` and does not deliver the message. If setting `\Seen` fails, the Consumer counts `b-prime:mail:dispose`.
- The repository does not contain it: the Consumer delivers the message. As soon as the route succeeds, and before the disposition, the Consumer calls `add`. A failed route adds nothing. If a disposition step then fails and leaves the message unseen, the next poll finds the Message-ID and only sets `\Seen`, so the route does not see the message twice. If `add` fails, the Consumer logs it, counts `b-prime:mail:idempotent` and still runs the disposition.
- The repository read fails: the Consumer leaves the message untouched and counts `b-prime:mail:idempotent`. The next poll tries again.
- The message has no Message-ID: the Consumer never deduplicates it.

A message that the repository already knows only gets `\Seen`. The Consumer does not copy, move or delete it, even when `copyTo`, `moveTo` or `delete` is set, so it stays in the source folder.

## pollEnrich

The `imap` and `imaps` schemes also serve `poll_enrich`. Each call returns at most one message and waits up to the step timeout. The timeout covers connecting, searching and downloading the message. Once a message is downloaded, its disposition always finishes (each server command has its own I/O timeout) and the message is returned, so the timeout never loses a downloaded message. A timeout of 0 cannot return mail from a real server, because connecting needs network round trips.

```yaml
routes:
  - id: pull-mail
    from: "timer:tick?period=60000"
    steps:
      - poll_enrich:
          uri: "imap://mail.example.com?folder=INBOX&moveTo=Done"
          timeout: 5000
```

A pull has no signal that the route finished. The Consumer therefore runs the disposition right after it fetches the message, and calls the repository `add` when it hands the message over, before the disposition. Delivery is at-most-once: if the route fails later, the message is not delivered again. If a disposition step fails, the message is still returned. A failure in step 1 or 2 leaves it unseen, so a later pull without a repository returns it again, up to `maxAttempts` times; with a repository, the later pull only sets `\Seen`. A failure in step 3 or 4 leaves a seen message in the source folder.

A pull shares the receive pipeline of the push Consumer, with these differences:

- `maxMessagesPerPoll` does not apply. A pull returns at most one message.
- If the repository read fails, the pull stops and returns an error. The push Consumer leaves that message untouched and continues with the next message.
- A pull logs skips and failures but does not count them in metrics, because a polling consumer has no runtime metrics.
- On a timeout or a broken connection, the session is closed without `LOGOUT` and the next call reconnects. A kept session that turns out broken is replaced once by a fresh connection within the same call. A refused command returns an error and keeps the session. A refused login returns an error.

## Metrics and health

- `camel_component_operations_total{component="mail",operation="send"}` counts SMTP sends. The `operation="poll"` series counts IMAP poll cycles. Both split into success and failure.
- Error labels: `b-prime:mail:skip` (a message skipped), `b-prime:mail:dispose` (a failed disposition), `b-prime:mail:idempotent` (a failed repository read or `add`). Only the push Consumer counts them.
- Each IMAP endpoint registers one health check named `mail`. It reports healthy until a push Consumer is created for the endpoint, so an endpoint used only by `poll_enrich` is never probed. It then reports the outcome of the latest poll, including a poll the server refused. When that outcome is older than twice the `delay`, the check opens a probe connection with a 5 s timeout. After the Consumer stopped for good (a missing capability, a refused login or exhausted reconnects), the check stays unhealthy until the route starts again and connects.

## Not supported

- XOAUTH2 and other token logins.
- IMAP IDLE. The Consumer polls.
- Attachments. The body is text only.
- POP3.
- The Gmail and Microsoft Graph APIs.
- DKIM signing.
- An inbound SMTP server.
- Full SearchTerm parity with Apache Camel. The Consumer reads unseen messages only.

## Headers

Header names are exact and case-sensitive.

Class A headers set content:

- `CamelMailSubject` overrides the `subject` option.
- `CamelMailContentType` selects `text/plain` or `text/html`.
- `CamelMailHtmlBody` adds an HTML alternative to a `text/plain` body.
- `X-*` headers pass through to the message (the `X-` prefix is matched case-insensitively).

The component rejects a value with CR, LF or NUL, and a value longer than 998 bytes. `CamelMailHtmlBody` rejects NUL only.

Class B headers set recipients: `CamelMailTo`, `CamelMailCc` and `CamelMailBcc`. They are accepted only on an endpoint that sets `allowedRecipients`. Any address outside the list is rejected. Header recipients replace the endpoint's recipients of the same kind. `From`, `Sender` and `Reply-To` cannot be set from a header.

An http-to-mail route also passes request headers such as `X-Api-Key` into the mail. Add a `removeHeaders` step before the mail endpoint to drop them.

## TLS

STARTTLS is required by default. A server that does not offer STARTTLS is refused before any login is sent. `smtps` uses implicit TLS. Set `allowCleartext=true` to allow cleartext on `smtp`; this applies to private hosts as well. Certificates verify against the webpki roots plus the account `caFile`. There is no option to turn verification off. Cleartext is stricter here than in ADR-0081: that ADR allows cleartext to private targets by default, and the mail flag covers them too.

The IMAP side builds its own TLS configuration with the ring provider only. The SMTP library (`lettre` 0.11) cannot take an explicit provider or TLS configuration. It uses the process default rustls provider and falls back to ring. `camel-cli` installs ring at startup. A process that installs aws-lc as the default, for example through the gRPC component, makes SMTP use that provider. The process-level fix is tracked in bd rc-2bofp.

See ADR-0093 for the decision record.
