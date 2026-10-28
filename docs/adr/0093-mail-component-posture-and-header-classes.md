# ADR-0093: Mail component TLS posture, header classes and send semantics

- Status: Accepted (2026-10-25; bd rc-q8zur)
- Source: openspec change `mail-component`
- Cross-refs: ADR-0007, ADR-0012, ADR-0013, ADR-0015, ADR-0020, ADR-0023,
  ADR-0032, ADR-0034, ADR-0051, ADR-0060, ADR-0063, ADR-0066, ADR-0071,
  ADR-0076, ADR-0081

## Context

The mail component (`camel-component-mail`) sends mail over SMTP and polls
mailboxes over IMAP. Mail is an exfiltration and spoofing surface in three
ways:

1. A route can send exchange data to any recipient. Exchange data is
   untrusted (ADR-0032).
2. A mail header value can carry CR or LF and inject further headers or
   recipients.
3. The HTTP consumer copies request headers into the exchange with
   Title-Case names. A component that read bare `To` or `From` headers
   would let any HTTP client choose the recipients of a mail sent by an
   http-to-mail route.

The component also handles logins, so it must follow the credential
redaction rules (ADR-0051, ADR-0076).

## Decision

### TLS posture

STARTTLS is required by default on `smtp` and `imap`. The schemes `smtps`
and `imaps` use implicit TLS. The only cleartext path is an explicit
`allowCleartext=true` on the endpoint. `allowCleartext=true` on `smtps` or
`imaps` fails endpoint creation, because there is no cleartext mode to
allow. A server that does not advertise STARTTLS is refused before
authentication, so no login is sent.

This is stricter than ADR-0081. ADR-0081 allows cleartext to private
targets by default and restricts only public targets. For mail, the flag
covers private targets too: a login sent in cleartext to an internal host
is still a login on the wire.

Certificate verification has no off switch. Servers verify against the
webpki roots. An account's `caFile` (PEM) adds roots for a private CA; it
does not replace the defaults.

IMAP TLS uses rustls with the ring provider; SMTP (lettre) uses the process-default rustls provider and falls back to ring. The component adds no aws-lc
dependency edge.

Known limitation. `lettre` 0.11 cannot take an explicit rustls provider
or client configuration: `TlsParametersBuilder::build_rustls` reads
`CryptoProvider::get_default()` and has no injection point (verified
against 0.11.23). The SMTP side therefore uses the process default
provider and falls back to ring. `camel-cli` installs ring as the default at startup. A process
that installs aws-lc as the default (for example through camel-grpc) can
make the SMTP provider differ. The IMAP side builds its own ring-only
configuration and is not affected. The process-level fix is tracked in
bd rc-2bofp.

### Header classes

The component reads and writes only headers with exact CamelCase names.
The names do not collide with the Title-Case headers the HTTP consumer
copies.

**Class A, content.** `CamelMailSubject`, `CamelMailContentType`,
`CamelMailHtmlBody` and `X-*` headers.

- The component rejects a value that contains CR, LF or NUL, or that is
  longer than 998 bytes, in its own code.
- `CamelMailHtmlBody` is body content, not a header line, so it rejects
  NUL only.
- `CamelMailSubject` overrides the `subject` option.

**Class B, routing.** `CamelMailTo`, `CamelMailCc`, `CamelMailBcc`.

- Endpoint-only by default. A routing header on an endpoint without
  `allowedRecipients` is an error.
- Setting `allowedRecipients` arms the fence. The value is a list of exact
  addresses or `@domain` entries; domains compare case-insensitively. An
  armed fence rejects any header address outside the list.
- `maxRecipients` (default 50) caps the total recipient count (ADR-0032).
- Header recipients replace the endpoint's `to`, `cc` or `bcc` of the same
  kind. Addresses configured on the endpoint are operator-owned and are not
  fenced.
- Bcc goes to the SMTP envelope only and never into the message headers.
- A display name in a routing header cannot add a header line or an
  envelope recipient: the address parser rejects it or the renderer
  encodes it.
- `From`, `Sender` and `Reply-To` are endpoint-only. No header sets them
  (anti-spoofing).

**Class C, mailbox mutation.** `delete`, `moveTo`, `copyTo` and the folder.
These are endpoint-only with no opt-in. The IMAP consumer (a later phase)
emits `CamelMailIn*` headers, so a mail-to-mail route cannot re-target a
mutation.

### `X-*` pass-through caveat

`X-*` headers pass through to the outgoing message. An http-to-mail route
therefore also emits inbound request headers such as `X-Api-Key`. This is
an owner-fixed decision. The mitigation is a `removeHeaders` step in the
route before the mail producer. It is flagged for owner review and is not
re-decided here.

### Send semantics

The producer accepts a text body, a JSON string body (the shape that
`set_body` and a DSL `constant` produce) or UTF-8 bytes; any other body
fails the exchange. The producer never resends after an ambiguous failure
past the SMTP DATA command. A send is a non-idempotent write, and the same rule applies as in
ADR-0023 Contract C1 and ADR-0063 Decision 7. The producer has no internal
retry. A route error handler may retry the whole exchange, which is a
decision of the route author. After an accepted send the producer sets
`CamelMailMessageId`.

### Accounts

Host and port live in the URI. Logins and trust anchors live in named
accounts under `[components.mail.accounts.<name>]`. A URI with userinfo
fails endpoint creation. Passwords use `SecretString` with a redacting
`Debug` (ADR-0051). The component never puts a server URL, host or port
into an error or a log line, so no URL needs `redact_url` (ADR-0076); a
log-capture test guards this. Text derived from a server reply or an IMAP
library error is cut to 200 characters with control characters removed,
and a parse error never embeds the raw response. A refused login yields a
constant message. Account selection follows the rabbitmq model: an
explicit `?account=` must exist; with one account and no `?account=`, that
account is used; with several, the endpoint fails and names `?account=`.
With no account, `smtp` and `smtps` send without authentication; `imap`
and `imaps` require an account and fail endpoint creation without one.

**Accepted risk: implicit single-account selection.** With exactly one
account, every endpoint without `?account=` logs in with it to whatever
host its URI names. A dynamic URI (`toD`, a URI assembled from exchange
data) can therefore forward the account credentials to a host the
operator did not intend. The owner accepted this for configuration
convenience (rabbitmq parity); operators who build mail URIs dynamically
should set `?account=` explicitly. A follow-up is tracked in bd.

### IMAP delivery

IMAP delivery is at-least-once. The consumer disposes of a message only
after the route completes: success runs the disposition, failure leaves the
message untouched. A crash between route success and `\Seen` redelivers
the message. A disposition failure is a side-effect failure with metric
label `b-prime:mail:dispose` (ADR-0012, ADR-0066). The consumer never
restarts itself (ADR-0007), and reconnects use `NetworkRetryPolicy`
(ADR-0013). `maxAttempts` counts failed attempts per `(UIDVALIDITY, UID)`:
a failed route, a failed body download, and a failed disposition step
that leaves the message unseen. The last one bounds redelivery after a
successful route, for example to a `copyTo` folder the server refuses. A
message that reaches `maxAttempts`, exceeds `maxMessageSize` or cannot be
parsed is skipped for the process lifetime (`b-prime:mail:skip`). The
memory only keeps UIDs that are still unseen, so it is bounded by the
folder's unseen set, and a UIDVALIDITY change discards it. A message
expunged between the search and the body download is ignored for that
poll and is not an attempt.

**Connection versus refusal.** A tagged `NO` or `BAD` reply leaves the
IMAP session usable: a refused poll keeps the session, marks the health
check unhealthy and retries at the next `delay`, and a refused disposition
step is counted as above. A connection-level failure (I/O error, lost
connection, unparsable reply, or a timeout, which abandons a command
mid-flight) drops the session, also when it happens in a disposition
step, and the consumer reconnects with backoff. Only connection-level
failures are retryable for the connect policy, so a refusal never drives
a reconnect loop.

**Refused login.** A tagged `NO` or `BAD` on `LOGIN` (wrong credentials)
is permanent, like a missing capability: the push consumer stops with an
error after one login (ADR-0007), the health check stays unhealthy, and a
pull returns the error without caching a session. Other login failures
(a broken connection) stay retryable.

**Idle drop.** A server may drop a session between polls. When a session
that completed a poll fails at the connection level on the first command
of the next poll, the push consumer reconnects at once, without a backoff
step, a failure count or an unhealthy report; a second failure takes the
normal backoff. A pull whose cached session fails at the connection level
retries once on a fresh connection within the same call.

**Health.** The check is registered at endpoint creation, before it is
known whether a push consumer runs. It reports healthy without probing
until a push consumer is created, so a `pollEnrich`-only endpoint never
logs in periodically. A consumer that stopped for good (missing
capability, refused login, exhausted reconnects) keeps the check
unhealthy; a stale-state probe does not clear it.

**Protocol logging.** `async-imap` logs raw protocol traffic, `LOGIN` with
the password included, at `trace`. The `camel-config` log filter caps the
`async_imap` target at `info` unless the operator names that target
explicitly; an operator must never set it to `trace`.

**Disposition order.** `copyTo`, then `\Seen`, then `moveTo` or `delete`.
The first failing step stops the sequence. `delete` and `moveTo` are
mutually exclusive. A failure before `\Seen` redelivers the message (a
`copyTo` that succeeded may then be repeated); a failure after `\Seen`
leaves a seen message in the source folder, which is never refetched.

**UIDPLUS fail-closed.** `delete=true` needs UIDPLUS. The consumer deletes
one UID with `UID STORE +FLAGS \Deleted` and `UID EXPUNGE`. A plain
`EXPUNGE` removes every `\Deleted` message in the folder, including mail
that other clients flagged, so without UIDPLUS the consumer refuses to run
instead of choosing a destructive fallback. The capability check runs after
each connect; a missing capability is permanent and stops the consumer.

**MOVE fallback.** `moveTo` uses `MOVE` when offered. Otherwise it copies
and then deletes the original with the UID-scoped delete above, which needs
UIDPLUS. With neither MOVE nor UIDPLUS the consumer fails closed. If the
delete of the fallback fails, the original stays `\Seen` and the copy
exists, so no mail is lost.

**Message-ID duplicate suppression.** `idempotentRepository=<name>` keys
on the Message-ID. A `contains` hit sets `\Seen` and does not deliver.
`add` runs as soon as the message is delivered (push: the route
succeeded; pull: the exchange is handed to the caller), before the
disposition and whatever its outcome, so a failed `\Seen`, copy, move or
delete cannot cause a redelivery that the repository lets through. A
failed route adds nothing. A repository read error leaves the message
untouched and counts `b-prime:mail:idempotent`; a failed `add` is logged
and counted the same way and does not stop the disposition. A message without
Message-ID is never deduplicated. The mail component cannot reach the core
registry (ADR-0023), so the `camel-bundles` boot block resolves the
repositories `memory` and the configured `[idempotent_repo]` name (or
backend) and passes them to `MailBundle`. An unknown name fails endpoint
creation. This needs no core change.

**Pull consumption (`pollEnrich`).** A pull has no route-completion
signal, so the polling consumer disposes right after the fetch. Delivery is
at-most-once: a later route failure does not redeliver. The Message-ID is
added on hand-over. If a disposition step fails, the message is still
returned; a failure before `\Seen` leaves it unseen, so a later pull
returns it again (unless the repository now knows it), bounded by
`maxAttempts`. The pull shares the push consumer's
receive pipeline. It diverges in three ways: `maxMessagesPerPoll` does not
apply (one message per call), a repository read error ends the pull with
an error instead of moving to the next message, and skips and failures
are logged but not counted, because a `PollingConsumer` has no runtime
metrics. The receive timeout bounds connecting, scanning and loading.
Once a message is loaded, its disposition runs to completion, bounded by
the per-command I/O timeout, and the message is returned, so the deadline
never drops a loaded message. `receive(Duration::ZERO)` therefore cannot
return mail from a real server.

### Boundaries

`lettre`, `async-imap` and `mail-parser` types stay under `src/adapter/` and
never appear in a public signature (ADR-0020, ADR-0060). The producer talks
to a `MailTransport` seam. The public surface is `MailBundle`,
`MailComponent`, `MailComponentConfig`, `MailAccountConfig`,
`SecretString`, `MailScheme`, `MailError` and the header-name constants;
endpoint configuration types stay crate-private.

## Consequences

- A route cannot reach an unlisted recipient through request headers unless
  the endpoint owner arms and lists the allowed recipients.
- Cleartext mail needs an explicit, visible flag on every endpoint that
  uses it.
- A private mail server needs an account `caFile`. There is no way to skip
  verification.
- Mail routes fed by HTTP leak `X-*` request headers unless the route
  removes them.
- A failed `\Seen` after a successful route redelivers the message, up
  to `maxAttempts` deliveries, unless `idempotentRepository` is set: the
  Message-ID is already recorded, so the redelivery is suppressed.
  Operators should make routes idempotent or set `idempotentRepository`.
- Wrong credentials stop an IMAP consumer; it does not retry them.
- With a single account, a dynamically built URI can send its credentials
  to an unintended host (accepted risk, above).
- A `pollEnrich` pull loses a message when the caller fails after the fetch.
- Under a non-ring process default provider, SMTP TLS follows that
  provider until bd rc-2bofp lands.
- An ambiguous send failure surfaces as an error. The operator, not the
  component, decides whether a duplicate mail is acceptable.

## Alternatives considered

- **Bare `To`/`From` headers (Camel parity).** Rejected. The HTTP consumer
  copies Title-Case request headers, so any HTTP client could set the
  recipients.
- **Opt-in class C (header-driven mailbox mutation).** Rejected. A
  mutation driven by exchange data is a trust-boundary break (ADR-0032), and
  no use case needs it.
- **JVM bridge (Jakarta Mail).** Rejected. SMTP and IMAP have mature Rust
  clients, and a bridge adds a process and a Java supply chain (ADR-0060
  favors a native component).
- **Cleartext default for private targets (ADR-0081 shape).** Rejected for
  mail, as described under TLS posture.
- **ADR number.** The change was drafted as ADR-0085. That number is taken by the
  legacy SHA-1 inbound opt-in; 0089 was then taken by the self-exec watchdog
  (mission 417), so this record is ADR-0093.
