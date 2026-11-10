# Proposal: mail-timeout-options

## Why

Two defects in `camel-component-mail` (bd `rc-k1n5y` P2, `rc-jpcqt` P3, from the
`rc-s8itn` audit):

- SMTP sends are **unbounded after the TCP connect**. Verified against lettre
  0.11.23 (the pinned version): the transport's `timeout` bounds only the TCP
  connect (`client/async_net.rs` wraps the per-address `TcpStream::connect`
  future); nothing bounds DNS resolution, the TLS handshake, or any
  post-connect phase (greeting, EHLO, AUTH, MAIL/DATA, QUIT). A relay that
  accepts the connection and then stalls parks the exchange indefinitely.
- The IMAP timeouts are fixed constants (`adapter/imap.rs`: 30 s whole-connect,
  60 s per I/O operation) and SMTP timeouts are not configurable at all. An
  operator cannot trade a slow server for a tighter bound, and cannot widen the
  30 s connect bound on a genuinely slow link.

## What Changes

- New URI options `connectTimeout` and `ioTimeout` on all four schemes
  (`smtp`, `smtps`, `imap`, `imaps`), in **milliseconds**, matching the
  component's existing duration convention (`delay`, `initialDelay`).
  Zero and empty values are rejected at parse time, with a message naming the
  option.
- Defaults preserve today's behaviour: IMAP 30000/60000 (the constants at
  `adapter/imap.rs:28-29`; the mission brief's "60 s / 60 s" is corrected —
  the connect constant is 30 s). SMTP `connectTimeout` 60000 (lettre's
  `DEFAULT_TIMEOUT`, which today bounds the TCP connect); SMTP `ioTimeout`
  600000 is a **new** whole-transaction bound with no predecessor — a
  documented behaviour change that turns an unbounded stall into a
  10-minute error.
- SMTP `ioTimeout` is a **whole-transaction** bound around the full
  `send_raw` conversation (DNS, TLS handshake, greeting, EHLO, AUTH,
  MAIL/DATA, up to the server's acceptance reply). Background connection
  cleanup (QUIT) is not covered. The pinned lettre offers no per-phase hook
  in the async path; per-phase granularity and bounded cleanup are filed as
  a residual bd issue, not pretended.
- A timed-out send must not leave a desynced connection in lettre's pool
  (`recycle` cannot detect a mid-DATA drop), so SMTP connection reuse is
  disabled (`PoolConfig::max_size(0)`). Each send opens a fresh connection.
  Documented as a behaviour change (performance, not correctness).
- Timeout errors name the phase and the bound that fired. IMAP timeouts map
  to `Connect`/`Store` (retryable, as today); SMTP timeouts map to
  `Transport` (non-retryable by `is_retryable()`, as today — the producer
  keeps its no-internal-resend rule).
- Docs: `docs/src/components/mail.md` gains both options for both schemes;
  the crate `CONTEXT.md` records the timeout semantics.
- Spec delta: `mail-component` requirement "Timeout bounds" with parse,
  default, and stalled-server scenarios.

Affected crates: `camel-component-mail` (the only Rust crate touched), plus
`docs/`. No dependency, wire/protocol, body-limit or attachment change. The
llm crate and other components are out of scope.

## Acceptance criteria

- Parse tests: accepted values, rejected `0` and empty (naming the option),
  defaults applied when the URI carries no option (IMAP 30000/60000,
  SMTP 60000/600000).
- Behaviour test: a stalled fake SMTP server trips `ioTimeout` with an error
  naming the phase and the bound; the same for IMAP via the existing scripted
  fake (connect and per-operation phases).
- Metadata parity tests pass (options declared in `SMTP_OPTIONS`/
  `IMAP_OPTIONS` and the descriptor structs).
- `cargo test -p camel-component-mail` green; `scripts/gates-local.sh` exit 0.

## Risk budget

- Acceptable: SMTP sends now fail after 10 minutes instead of hanging
  forever; SMTP connection reuse is gone (one TCP+TLS handshake per send).
- Out of bounds: per-phase SMTP bounds beyond the two knobs, new
  dependencies, wire/protocol changes, `maxMessageSize` (10 MiB) changes,
  changes outside the mail component.
