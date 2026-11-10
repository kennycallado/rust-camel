## ADDED Requirements

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
