# camel-component-mail

Mail component for rust-camel with four schemes: `smtp` and `smtps` send mail, `imap` and `imaps` poll a mailbox. Logins are configured per account in `Camel.toml`:

```toml
[components.mail.accounts.main]
username = "robot@example.com"
password = "${env:MAIL_PASSWORD}"
# caFile = "/etc/ssl/mail-ca.pem"
```

Credentials never go in the URI.

## URI options

`smtp` and `smtps` (producer):

| Option | Default |
|--------|---------|
| `account` | none |
| `from` | required |
| `to`, `cc`, `bcc` | none |
| `subject` | none |
| `contentType` | `text/plain` |
| `allowedRecipients` | none |
| `maxRecipients` | `50` |
| `allowCleartext` | `false` |

`imap` and `imaps` (consumer and `pollEnrich`):

| Option | Default |
|--------|---------|
| `account` | none |
| `allowCleartext` | `false` |
| `folder` | `INBOX` |
| `delay` | `60000` |
| `initialDelay` | `1000` |
| `maxMessagesPerPoll` | `100` |
| `maxMessageSize` | `10485760` |
| `maxAttempts` | `3` |
| `delete` | `false` |
| `moveTo` | none |
| `copyTo` | none |
| `idempotentRepository` | none |

## Behaviour notes

- Component operation metrics need `[observability.metrics] components = true`. The `b-prime:mail:*` error labels come from the error family, which that lever does not gate. Only the push Consumer counts them; a `pollEnrich` call logs and does not count.
- `Body::Empty` fails the send. `set_body` with `value: ""` sends a 0-byte `text/plain` body.
- `pollEnrich` with the default `UseEnrichedBody` copies only the body into the original Exchange. The polled `CamelMailIn*` headers are dropped.
- A relative `caFile` resolves against the process working directory, not against the directory that holds `Camel.toml`.
- With profile sections, the accounts live under `[default.components.mail.accounts.<name>]`. The flat `[components.mail.accounts.<name>]` form is for a config with no `[default]` or `[<profile>]` section.

## Exchange headers

Header names are exact and case-sensitive.

Class A headers set content: `CamelMailSubject`, `CamelMailContentType`, `CamelMailHtmlBody`, `CamelMailListUnsubscribe`, `CamelMailListUnsubscribePost`, and `X-*`.

Class B headers set recipients. The endpoint accepts them only when it sets `allowedRecipients`:

```
CamelMailTo
CamelMailCc
CamelMailBcc
```

Class C is endpoint-only. No Exchange header sets `delete`, `moveTo`, `copyTo`, or `folder`.

`CamelMailListUnsubscribePost` must equal `List-Unsubscribe=One-Click`. It needs an `https` URI in `CamelMailListUnsubscribe`. A bare `List-Unsubscribe` header is ignored, and only the `CamelMail*` name is read.

## Integration tests (Docker tier)

The GreenMail and Mailpit integration tiers are gated: they run only when
`MAIL_ITEST=1` is set and Docker is available; without the environment variable
they skip as passing no-ops. Run them locally with:

```sh
MAIL_ITEST=1 cargo test -p camel-component-mail --test greenmail_tier --test mailpit_tier
```

- `greenmail_tier` covers consumer delete/move/copy, `pollEnrich` and seen-flag
  handling over IMAP against GreenMail.
- `mailpit_tier` covers the SMTP STARTTLS round trip against Mailpit.

Each fixture starts and removes its own Docker container (`greenmail/standalone:2.1.2`
and `axllent/mailpit:v1.20.0`). The `Integration Mail` workflow
(`.github/workflows/integration-mail.yml`) runs the same command nightly at
04:30 UTC and on `workflow_dispatch`; it is deliberately not run on every push.

For IMAP connection debugging, use `log_level = "debug"` or
`RUST_LOG=camel_component_mail=debug`: the adapter logs host, port, TLS mode,
authentication attempt and session established, never credentials. Raw
`async-imap` `LOGIN` traces are capped/suppressed because the dependency logs
the password verbatim.

See the guide page `docs/src/components/mail.md` for the semantics, headers, disposition and limits, ADR-0093 for the decisions, and `examples/mail-example` for a runnable send-and-receive route pair against a local GreenMail fixture.
