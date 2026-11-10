# Design: mail-timeout-options

## Approach

Two endpoint options, `connectTimeout` and `ioTimeout`, parsed in
`config.rs` and applied in `adapter/imap.rs` and `adapter/smtp.rs`.

### Options, unit, validation

- Names: `connectTimeout`, `ioTimeout` (camelCase, like `maxRecipients`).
- Unit: **milliseconds** (the component's existing duration unit: `delay`,
  `initialDelay` are ms; `Duration::from_millis`).
- Parsing: a `parse_timeout_ms` helper beside `parse_min_one`. Absent →
  default. `""` → `CamelError::InvalidUri` naming the option ("must not be
  empty"). `0` or non-numeric → `CamelError::InvalidUri`
  `invalid {option} '{raw}': expected positive milliseconds` (mirrors the
  `imap_rejects_zero_limits` pattern). No upper bound: `u64` ms into
  `Duration` cannot overflow meaningfully.
- Both options join `SMTP_OPTIONS`/`IMAP_OPTIONS` and the `metadata.rs`
  descriptor structs with `default = "..."`; the parity tests enumerate the
  new defaults.
- Fields: `SmtpEndpointConfig`/`ImapEndpointConfig` gain
  `connect_timeout: Duration`, `io_timeout: Duration`.

### IMAP (async-imap, our own timeouts)

Replace the constants with the parsed values threaded through
`AsyncImapConnector::new`/`from_endpoint` (`from_endpoint` reads them from
`ImapEndpointConfig`; the `#[cfg(test)] with_timeouts` constructor stays).

- `connectTimeout` bounds `MailStoreConnector::connect` — TCP connect,
  greeting, CAPABILITY, STARTTLS, TLS handshake, LOGIN (one bound, as
  today, default 30000).
- `ioTimeout` bounds every `MailStore` operation through the existing
  `with_timeout` helper (default 60000).
- Honest errors: `with_timeout` gains a phase label —
  `MailError::Store("IMAP {phase} timed out after {n} ms (ioTimeout)")`;
  the connect timeout reads
  `MailError::Connect("IMAP connect timed out after {n} ms (connectTimeout)")`.
  Phases: `capabilities`, `select`, `search`, `fetch sizes`, `fetch body`,
  `mark seen`, `copy`, `move`, `delete`, `logout`.

### SMTP (lettre 0.11.23, Tokio1 — verified against the pinned source)

- `connectTimeout` → `AsyncSmtpTransportBuilder::timeout(Some(d))`.
  Verified semantics at this version: it wraps each per-address
  `TcpStream::connect` future (`client/async_net.rs:171`) — the bound is
  **per resolved address**, not for aggregate connection establishment; the
  worst case is one bound per resolved address. DNS resolution and the TLS
  handshake sit **outside** it. Default 60000 preserves the connect bound
  operators see today.
- `ioTimeout` → a `tokio::time::timeout` wrap around `send_raw` inside
  `LettreSmtpTransport::send`. This is a **whole-transaction** bound:
  DNS, TLS handshake, greeting, EHLO, AUTH, MAIL/DATA, up to the server's
  acceptance reply. It is not per-read and not per-phase — the pinned lettre
  exposes no such hook in the async path (the blocking path sets
  `SO_*` timeouts; the async path sets none). It bounds the caller-visible
  send, not the background connection cleanup: with pooling disabled, a
  recycled connection is aborted in a spawned task whose QUIT exchange can
  wait indefinitely on a stalled server (`client/async_connection.rs`
  `abort`). Each timed-out send may therefore leave one best-effort
  background task and socket; that leak is part of the residual bd issue,
  not of this change's promise. A 50 MiB message over a slow uplink must
  fit: 600000 ms covers ~50 MiB at 1 Mbit/s with protocol overhead, while
  still bounding an accept-then-stall relay to ten minutes. Default
  **600000** (10 minutes).
- Connect error: lettre's connect timeout surfaces as an error answering
  `is_timeout()`; the adapter maps it to
  `MailError::Transport("SMTP connect timed out after {n} ms
  (connectTimeout)")` instead of the raw `transport_err` string.
- Transaction error:
  `MailError::Transport("SMTP send timed out after {n} ms (ioTimeout)")`.
- Pool: `recycle` keeps any connection whose `has_broken()` flag is false;
  a send dropped mid-DATA sets no such flag, so reuse would hand the next
  send a desynced stream. `LettreSmtpTransport::new` therefore builds with
  `PoolConfig::new().max_size(0)`: every send opens a fresh connection and
  a timed-out send's connection is aborted on recycle. Documented in the
  proposal as a behaviour change (a TCP+TLS handshake per mail, mail
  workloads are not hot-loop). The background QUIT cleanup is unbounded —
  see the whole-transaction note above.
- Residual (bd, filed before implementation): lettre 0.11.23's async path
  offers no per-phase SMTP bounds (separate greeting/EHLO/AUTH/DATA
  deadlines), no socket read/write timeouts, and its background connection
  cleanup (QUIT) cannot be bounded; DNS lookup is covered by `ioTimeout`,
  not `connectTimeout`.

### Error mapping

No new `MailError` variants; today's classification is preserved. IMAP
timeouts keep their retryability: the connect timeout is `Connect` and an
operation timeout is `Store` (the session is dropped and reconnected per
CONTEXT.md). SMTP timeouts are `Transport`, which `is_retryable()` reports
as non-retryable — unchanged; the producer still never resends, and a route
error handler may retry the whole exchange. Messages carry the phase, the
bound in ms and the option name that set it; server-derived text stays
behind `error::server_text`.

## Affected crates

- `camel-component-mail`: `config.rs` (options, parse helper, consts),
  `metadata.rs` (descriptors + parity-test defaults), `adapter/imap.rs`
  (thread config, phase labels), `adapter/smtp.rs` (builder timeout,
  transaction wrap, pool, connect-timeout mapping), plus their unit tests;
  `tests/smtp_protocol.rs` and `tests/support/fake_smtp.rs`
  (`StallAfterData` behaviour).
- `docs`: `docs/src/components/mail.md` option tables for both schemes and
  a short "Timeouts" section.

## Architecture boundaries

Component-internal change only, matching ADR-0020/0060 (third-party types
stay inside `src/adapter/`) and ADR-0093 (mail component record). The
`MailTransport`/`MailStore` seams keep their signatures; no core, DSL,
config-schema or runtime change. URI options flow through the existing
`ComponentMetadata` descriptors so the CLI/LSP surface stays consistent.

## Phases

Omitted — single coherent slice (parse/config, adapters, docs, spec delta
land together; each task is independently reviewed).
