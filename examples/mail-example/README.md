# mail-example

Timer-driven SMTP producer and IMAP consumer against a local GreenMail fixture.

- Producer: `timer:tick?period=10000` → `set_body` → `smtp://127.0.0.1:3025?...&to=user@local.test`.
- Consumer: `imap://127.0.0.1:3143?...&delay=5000` → `log:info?showHeaders=true`. The Consumer sets
  `\Seen` on each mail after the log step succeeds, so every mail is logged once.

Both endpoints select the `demo` account explicitly with `?account=demo` and set
`allowCleartext=true`, because the fixture listens on plain loopback ports. Without that flag the
component requires STARTTLS.

## Run it

Start GreenMail on loopback with one fixture user:

```bash
docker run -d --rm --name greenmail-example \
  -p 127.0.0.1:3025:3025 -p 127.0.0.1:3143:3143 \
  -e GREENMAIL_OPTS='-Dgreenmail.setup.test.smtp -Dgreenmail.setup.test.imap -Dgreenmail.hostname=0.0.0.0 -Dgreenmail.users=user:pass@local.test' \
  greenmail/standalone:2.1.2
```

Then run the example:

```bash
cargo run -p mail-example
```

The `user`/`pass` login belongs to the ephemeral local fixture. It is not a production secret. A
real deployment keeps STARTTLS and reads the password from `Camel.toml`:

```toml
[components.mail.accounts.main]
username = "robot@example.com"
password = "${env:MAIL_PASSWORD}"
```

Press Ctrl+C to stop.
