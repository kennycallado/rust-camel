## ADDED Requirements

### Requirement: Canonical URL redaction helper

The workspace SHALL provide one canonical string-based URL redactor,
`camel_api::redact`, for diagnostic surfaces that render URL text
(ADR-0051). The module SHALL expose a strict variant, a fail-closed
variant, and a query-allowlist variant, all built on one authority-window
scan. An authority window is the substring that begins immediately after
a maximal run of `/` and `\` characters and ends at the next `/`, `?`,
or `#`. A run made only of slashes opens a window when it is two or
more characters long. A run that contains a backslash opens a window
only when an RFC 3986 scheme prefix (alpha first character, then
alphanumeric, `+`, `.`, or `-`, then `:`) sits immediately before the
run: a backslash run of two or more characters needs any such prefix; a
single-backslash run needs a prefix of at least two characters, or a
one-character prefix when the candidate window content carries `:`
before its last `@` (credential-shaped content — `x:\user:pass@evil`
opens a window, `C:\Users\x@corp\file` does not).

The strict variant SHALL mask userinfo in every authority window (the
bytes from window start through the last `@` render as `***`), SHALL
truncate at the earliest `?` or `#`, SHALL append one `?[redacted]`
and/or `#[redacted]` sentinel per distinct introducer present, in
first-occurrence order, and SHALL cap the result at 256 bytes on a UTF-8
character boundary, reserving the sentinel byte budget before the cap so
a sentinel never renders split. The fail-closed variant SHALL return the
constant sentinel `[redacted]` and no other content when any authority
window contains `@`; otherwise it SHALL behave as the strict variant.
The query-allowlist variant is specified in the `jms` capability
(broker URL redaction).

Every diagnostic surface that renders URL text in camel-config,
camel-jms, and camel-http SHALL render byte-identical output for the
same input through the same variant, because all three consume this one
implementation.

#### Scenario: strict variant masks every window and composes sentinels

- **GIVEN** `https://user:pass@h/p?a=1#t=x` and
  `https://h//u2:p2@evil/` (later-window credentials parked in the path)
- **WHEN** each string passes through the strict variant
- **THEN** the first renders `https://***@h/p?[redacted]#[redacted]` and
  the second masks the later window's userinfo bytes (`u2:p2` never
  renders; `***@evil` does)

#### Scenario: fail-closed variant suppresses any window that carries credentials

- **GIVEN** a string whose any authority window contains `@`, such as
  `http://u:secretpw@host:99999/x`
- **WHEN** it passes through the fail-closed variant
- **THEN** the output is exactly `[redacted]`

#### Scenario: backslash authority runs window only when scheme-prefixed

- **GIVEN** `foo:\\user:pass@evil/` (non-special scheme, backslash
  authority), `http:\user:pass@evil\path` (single backslash after a
  two-character scheme), `x:\user:pass@evil` (single backslash after a
  one-character scheme, credential-shaped content), `C:\Users\x@corp\file`
  (drive path), and `\\server\x@y` (UNC path)
- **WHEN** each passes through the strict variant
- **THEN** the first three mask their userinfo (`***@`); the drive and UNC
  strings render unchanged — no qualifying scheme-prefixed backslash run
  windows them

#### Scenario: sentinel never splits at the byte cap

- **GIVEN** a URL whose pre-sentinel bytes exceed 245 bytes and whose
  query and fragment both exist
- **WHEN** it passes through the strict variant
- **THEN** the output is at most 256 bytes, ends with the complete
  `?[redacted]#[redacted]` sequence, and cuts on a UTF-8 character
  boundary

#### Scenario: cross-surface redaction identity

- **GIVEN** `http://h:99999/p?token=secret` (fails `url::Url::parse` on
  the port; no authority window carries `@`) rendered through the
  camel-config cache Debug path, the camel-jms broker log path, and the
  camel-http failure arm
- **WHEN** all three render diagnostics for it
- **THEN** every surface renders exactly `http://h:99999/p?[redacted]` —
  byte-identical, no query byte, no credential byte

#### Scenario: masking is idempotent on the composition path

- **GIVEN** `https://***@host/p?a=1#f` — a render that already carries
  an accessor-masked window
- **WHEN** it passes through the strict variant
- **THEN** the output is exactly `https://***@host/p?[redacted]#[redacted]`
  — the masked window rewrites to itself and each sentinel appends once
