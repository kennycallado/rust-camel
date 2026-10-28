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

See the guide page `docs/src/components/mail.md` for the semantics, headers, disposition and limits, ADR-0093 for the decisions, and `examples/mail-example` for a runnable send-and-receive route pair against a local GreenMail fixture.
