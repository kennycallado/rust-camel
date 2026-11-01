# Design: mail-followups

## Approach

### 1. List-Unsubscribe (class A)

Two new `pub const` header names live in `crates/components/camel-mail/src/headers.rs`, next to
the other class-A names:

- `CAMEL_MAIL_LIST_UNSUBSCRIBE = "CamelMailListUnsubscribe"`
- `CAMEL_MAIL_LIST_UNSUBSCRIBE_POST = "CamelMailListUnsubscribePost"`

The wire names `List-Unsubscribe` and `List-Unsubscribe-Post` are private constants in the same
module, so each literal has one definition site (`lint-single-source`).

`OutboundMessage` (`message.rs`) gains two `Option<String>` fields, `list_unsubscribe` and
`list_unsubscribe_post`. `build_outbound` fills them and validates them before any network I/O;
`render_mime` (`adapter/smtp.rs`) maps them to the standard wire names with `raw_header`.

The fields do NOT ride `extra_headers`. That vector means "`X-*` pass-through" (ADR-0093 class
A), and reusing it for standard names would blur the rule. With the existing `X-` filter
untouched, a bare `List-Unsubscribe`, `list-unsubscribe` or `Camelmaillistunsubscribe` exchange
header stays ignored with no extra code: `Message::headers` is a case-sensitive `HashMap`, so
only the exact `CamelMailListUnsubscribe` name is read.

Validation, in `build_outbound`:

1. Value must be a string, else `MailError::HeaderValidation { header, reason }`.
2. `validate_header_value` rejects CR, LF, NUL and values over 998 bytes.
3. The value must be ASCII. Non-ASCII would be encoded by lettre as RFC 2047 and corrupt the
   URI.
4. The value is split on `,`. Each element is trimmed of ASCII whitespace and must be `<...>`
   with a non-empty inner part that contains no whitespace, `<` or `>`. Text outside brackets is
   rejected (no RFC 2369 comments).
5. EVERY element must start with `mailto:` plus at least one character, or `https://` plus a
   non-empty host, compared ASCII case-insensitively. The host is the run of characters after
   `https://` up to the first `/`, `?` or `#`. An element with any other scheme rejects the
   whole list, so a stray `<http://...>` or `<javascript:...>` cannot ride along next to a valid
   element.

`CamelMailListUnsubscribePost` must equal `List-Unsubscribe=One-Click` byte for byte. Surrounding
whitespace and case changes are rejected. It requires `CamelMailListUnsubscribe` on the same
message. RFC 8058 requires an `https` URI in `List-Unsubscribe` when the Post header is present,
so a `mailto:`-only value paired with Post is rejected and the error names
`CamelMailListUnsubscribePost`.

Every rejection is `MailError::HeaderValidation`. The `header` field carries the Camel header
name, matching the existing class-A errors.

### 2. Consumer failure-cause logging

`Cycle::failed` (`poll.rs`) returns the attempt count. The count is read after the increment but
before the `maxAttempts` branch removes the entry, so the last attempt reports `maxAttempts` and
not 0. The `maxAttempts` skip and the state removal keep their current order, so retry,
disposition and skip semantics do not change. On the final attempt the skip warning is emitted
inside `failed` and the per-attempt warning after it; the spec states no ordering between them.
The two other call sites (`load` body-download failure, `delivered` unseen disposition failure)
ignore the return value.

`poll_once` (`consumer.rs`) replaces the catch-all `_` arm with two explicit paths: `Ok(ex)` with
`ex.error == Some(e)`, and `Err(e)` other than `ChannelClosed`. `ChannelClosed` keeps its current
arm: it returns `Polled::RouteClosed` without logging and without counting an attempt. The log
line is

```text
warn!(uid, uid_validity, attempt, max_attempts, error_kind, "mail route failed; message left unseen")
```

`error_kind` is `CamelError::variant_name()`, a `&'static str`. `classify()` is too coarse
("processor") and `Display`/`Debug` can carry the target URL or body text, so neither is used.
The fields carry no host, user, subject or body, which satisfies ADR-0051 and
`lint-log-redaction`.

### 3. Documentation drift

`docs/src/components/mail.md` and `crates/components/camel-mail/README.md` are corrected on five
points, each verified against the code before writing:

1. Component metrics are opt-in: `[observability.metrics] components = true`
   (`ComponentMetrics` gates `camel_component_operations_total`; see ADR-0066).
2. An empty body splits two cases: `Body::Empty` fails the exchange, while
   `set_body value: ""` is a 0-byte text body and sends.
3. `pollEnrich` with `UseEnrichedBody` copies only the body; `CamelMailIn*` headers do not reach
   the original exchange.
4. A relative `caFile` resolves against the process working directory, not against the
   `Camel.toml` location.
5. A profile needs `[default.components.mail...]`; the flat `[components.mail...]` form is the
   non-profile layout used by the examples.

The same two pages SHALL document the two new class-A headers: `CamelMailListUnsubscribe` and
`CamelMailListUnsubscribePost`, their wire mapping, the rule that a bare standard name is
ignored, the pairing and token constraints, and the RFC 8058 requirement that a one-click
`List-Unsubscribe-Post` needs a DKIM signature over both headers (this component does not sign).

## Affected crates

- `camel-component-mail`: `headers.rs`, `message.rs`, `adapter/smtp.rs`, `consumer.rs`,
  `poll.rs`, plus unit tests in those modules.
- `docs/`: `src/components/mail.md`, the crate README, and an amendment note in
  `docs/adr/0093-mail-component-posture-and-header-classes.md` that adds the two names to the
  class-A list. No code.

## Architecture boundaries

The change stays inside the Components tier. It adds no Runtime, DSL, Services or Languages
surface. `OutboundMessage` stays crate-private, so the public surface grows only by two header
constants. ADR-0093 is amended for the two class-A names rather than bypassed. ADR-0032 (exchange
data is untrusted, hence validation before I/O), ADR-0051 (redaction), ADR-0012 (the consumer
owns the failure signal) and ADR-0066 (metric gating) are respected.

## Alternatives considered

- Pushing the wire headers through `extra_headers`: fewer fields, but it makes the `X-*`
  pass-through vector a general header channel and weakens the ADR-0093 class-A rule.
- Validating in `render_mime`: it runs inside the transport adapter and cannot fail the exchange
  as early; `build_outbound` is pure and already owns class-A validation.
- Logging `Display` of the route error: rejected because the text can contain an SSRF target
  URL, which the ticket forbids.
- Logging `classify()`: rejected because "processor" does not identify the failure.
