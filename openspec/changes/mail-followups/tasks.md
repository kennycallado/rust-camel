# Tasks: mail-followups

## camel-component-mail: header names

### Task 1.1: add the two class-A header constants and their wire names

**Files:**
- `crates/components/camel-mail/src/headers.rs` (modified)

**Steps:**
1. Add `pub const CAMEL_MAIL_LIST_UNSUBSCRIBE: &str = "CamelMailListUnsubscribe";` with a rustdoc line
   naming it a class-A content header (ADR-0093).
2. Add `pub const CAMEL_MAIL_LIST_UNSUBSCRIBE_POST: &str = "CamelMailListUnsubscribePost";` with the
   same style of rustdoc.
3. Add `pub(crate) const WIRE_LIST_UNSUBSCRIBE: &str = "List-Unsubscribe";` and
   `pub(crate) const WIRE_LIST_UNSUBSCRIBE_POST: &str = "List-Unsubscribe-Post";`.
4. Add `pub(crate) const ONE_CLICK_TOKEN: &str = "List-Unsubscribe=One-Click";`.

**Tests:** (executable spec — name, arrange, act, assert)
- `class_a_list_unsubscribe_names` — arrange: nothing; act: read the five constants; assert:
  `CAMEL_MAIL_LIST_UNSUBSCRIBE == "CamelMailListUnsubscribe"`,
  `CAMEL_MAIL_LIST_UNSUBSCRIBE_POST == "CamelMailListUnsubscribePost"`,
  `WIRE_LIST_UNSUBSCRIBE == "List-Unsubscribe"`, `WIRE_LIST_UNSUBSCRIBE_POST == "List-Unsubscribe-Post"`,
  `ONE_CLICK_TOKEN == "List-Unsubscribe=One-Click"`.
  command: `cargo test -p camel-component-mail --lib headers::tests::class_a_list_unsubscribe_names`;
  expected: RED before step 1 (the constants do not resolve), GREEN after.

**Acceptance:**
- `cargo test -p camel-component-mail --lib headers::tests` passes.
- `cargo check -p camel-component-mail` exits 0. (`clippy -D warnings` is deferred to task 1.4: the
  private wire constants have no reader until `render_mime` uses them, so an earlier clippy gate
  would fail on dead code.)
- `cargo xtask lint-missing-docs-ratchet` exits 0 (both `pub const` carry doc comments).

- [x] 1.1

## camel-component-mail: producer message construction

### Task 1.2: validate `CamelMailListUnsubscribe` and carry it on `OutboundMessage`

**Files:**
- `crates/components/camel-mail/src/message.rs` (modified)
- `crates/components/camel-mail/src/transport.rs` (modified)

**Steps:**
1. Add two fields to `OutboundMessage`: `pub(crate) list_unsubscribe: Option<String>` and
   `pub(crate) list_unsubscribe_post: Option<String>`.
2. In `crates/components/camel-mail/src/transport.rs`, the `FakeTransport` fixture builds an
   `OutboundMessage { ... }` literal: add `list_unsubscribe: None, list_unsubscribe_post: None` so the
   crate still compiles.
3. Add `fn unsubscribe_list_value(exchange: &Exchange) -> Result<Option<String>, MailError>` that reads
   `CAMEL_MAIL_LIST_UNSUBSCRIBE` through the existing `string_header` helper (reason
   `"must be a string"`), returns `Ok(None)` when absent, applies `headers::validate_header_value`, and
   rejects non-ASCII with reason `"must be ASCII"` (the only place the ASCII check lives). It then calls
   `unsubscribe_uris` and returns the original value.
4. Add `fn unsubscribe_uris(value: &str) -> Result<Vec<String>, MailError>` as the single parser for the
   list: split on `,`; trim ASCII whitespace; every element must be `<...>` with a non-empty inner part
   that contains no whitespace, `<` or `>`; every inner part must start ASCII-case-insensitively with
   `mailto:` plus at least one more character, or `https://` plus a non-empty host (bytes after
   `https://` up to the first `/`, `?` or `#`). It returns the inner parts in order. Every failure
   returns `MailError::HeaderValidation { header: CAMEL_MAIL_LIST_UNSUBSCRIBE.to_string(), reason }`
   with reason `"must contain only mailto: or https: angle-bracket URIs"` or a `validate_header_value`
   reason. Task 1.3 must reuse this parser, never a second copy.
5. Call `unsubscribe_list_value` from `build_outbound` before the `extra_headers` loop, place the result
   in `list_unsubscribe`, and set `list_unsubscribe_post: None` (task 1.3 fills it).

**Tests:** (executable spec — name, arrange, act, assert)
- `list_unsubscribe_accepts_multiple_uris` — arrange: `ex(text("x"), &[("CamelMailListUnsubscribe",
  s("<mailto:u@example.com>, <https://example.com/u/1>, <https://example.com/u/2>"))])`; act:
  `build_outbound(&cfg(BASE), &e)`; assert: `Ok`, `m.list_unsubscribe` equals that exact string.
- `list_unsubscribe_accepts_https_only` — arrange: `<https://example.com/u/1>`; act: `build_outbound`;
  assert: `Ok`, `m.list_unsubscribe.as_deref() == Some("<https://example.com/u/1>")`.
- `list_unsubscribe_rejects_non_string` — arrange: `("CamelMailListUnsubscribe", Value::from(7))`; act:
  `build_outbound`; assert: `hv_header(...) == ("CamelMailListUnsubscribe", "must be a string")`.
- `list_unsubscribe_rejects_cr` — arrange: `<mailto:a@x.test\r>`; assert:
  `("CamelMailListUnsubscribe", "contains CR")`.
- `list_unsubscribe_rejects_lf` — arrange: `<mailto:a@x.test\n>`; assert:
  `("CamelMailListUnsubscribe", "contains LF")`.
- `list_unsubscribe_rejects_nul` — arrange: `<mailto:a@x.test\0>`; assert:
  `("CamelMailListUnsubscribe", "contains NUL")`.
- `list_unsubscribe_rejects_over_998` — arrange: `<mailto:` + `"a".repeat(999)` + `>`; assert:
  `("CamelMailListUnsubscribe", "longer than 998 bytes")`.
- `list_unsubscribe_rejects_non_ascii` — arrange: `<mailto:a@b.test?subject=café>`; assert:
  `("CamelMailListUnsubscribe", "must be ASCII")`.
- `list_unsubscribe_rejects_unbracketed` — arrange: `https://example.com/u/1`; assert:
  `("CamelMailListUnsubscribe", "must contain only mailto: or https: angle-bracket URIs")`.
- `list_unsubscribe_rejects_empty_brackets` — arrange: `<>`; assert: the same reason string.
- `list_unsubscribe_rejects_unsupported_scheme` — arrange: `<ftp://example.com/u>`; assert: the same
  reason string.
- `list_unsubscribe_rejects_unsupported_element_in_list` — arrange:
  `<mailto:a@x.test>, <http://x.test>`; assert: the same reason string.
- `list_unsubscribe_rejects_https_without_host` — arrange: `<https:///p>`; assert: the same reason
  string.
- `bare_list_unsubscribe_ignored` — arrange: headers `list-unsubscribe`, `LIST-UNSUBSCRIBE`,
  `camelmaillistunsubscribe`, `CAMELMAILLISTUNSUBSCRIBEPOST`, each with a valid value; act:
  `build_outbound`; assert: `Ok`, `m.list_unsubscribe.is_none()`, `m.list_unsubscribe_post.is_none()`,
  `m.extra_headers` is empty.
- `http_copied_list_unsubscribe_ignored` — arrange: Title-Case `List-Unsubscribe` =
  `<https://x.test/u>` and `List-Unsubscribe-Post` = `List-Unsubscribe=One-Click`; assert: `Ok`, both
  new fields `None`, `m.extra_headers` is empty.
  command for the block: `cargo test -p camel-component-mail --lib message::tests`;
  expected: RED before steps 1–3 (fields and helpers do not exist), GREEN after.

**Acceptance:**
- Every test above passes; existing `message::tests` pass unchanged.
- `cargo test -p camel-component-mail --lib` passes and `cargo check -p camel-component-mail` exits 0.
- No `unwrap()` outside `#[cfg(test)]` (`cargo xtask lint-unwrap`).

- [x] 1.2

### Task 1.3: enforce the one-click Post pairing and token

**Files:**
- `crates/components/camel-mail/src/message.rs` (modified)

**Steps:**
1. Add `fn unsubscribe_post_value(exchange: &Exchange, list: Option<&str>) -> Result<Option<String>, MailError>`
   that reads `CAMEL_MAIL_LIST_UNSUBSCRIBE_POST` via `string_header` (reason `"must be a string"`),
   returns `Ok(None)` when absent, and applies `headers::validate_header_value`.
2. When the Post value is present, require `list` to be `Some`, else
   `MailError::HeaderValidation { header: CAMEL_MAIL_LIST_UNSUBSCRIBE_POST.to_string(), reason: "requires CamelMailListUnsubscribe" }`.
3. Require the value to equal `headers::ONE_CLICK_TOKEN` byte for byte, else reason
   `"must be exactly List-Unsubscribe=One-Click"`.
4. Call `unsubscribe_uris(list)` (task 1.2) and require at least one returned element to start
   ASCII-case-insensitively with `https://` followed by a non-empty host, else
   `CAMEL_MAIL_LIST_UNSUBSCRIBE_POST` with reason
   `"requires an https URI in CamelMailListUnsubscribe (RFC 8058)"`. Do not duplicate parser rules.
5. Call it from `build_outbound` right after the list value is read, and set `list_unsubscribe_post`.

**Tests:** (executable spec — name, arrange, act, assert)
- `list_unsubscribe_post_accepted_with_https_list` — arrange: list `<https://example.com/u/1>` + Post
  `List-Unsubscribe=One-Click`; act: `build_outbound`; assert:
  `m.list_unsubscribe_post.as_deref() == Some("List-Unsubscribe=One-Click")`.
- `list_unsubscribe_post_requires_list` — arrange: Post only; assert:
  `("CamelMailListUnsubscribePost", "requires CamelMailListUnsubscribe")`.
- `list_unsubscribe_post_rejects_lowercase_token` — arrange: valid https list + `list-unsubscribe=one-click`;
  assert: `("CamelMailListUnsubscribePost", "must be exactly List-Unsubscribe=One-Click")`.
- `list_unsubscribe_post_rejects_trailing_space` — arrange: same list + `List-Unsubscribe=One-Click `;
  assert: the same pair as the lowercase case.
- `list_unsubscribe_post_rejects_mailto_only_list` — arrange: list `<mailto:u@example.com>` + valid token;
  assert: `("CamelMailListUnsubscribePost", "requires an https URI in CamelMailListUnsubscribe (RFC 8058)")`.
- `list_unsubscribe_post_rejects_non_string` — arrange: `Value::Bool(true)`; assert:
  `("CamelMailListUnsubscribePost", "must be a string")`.
  command: `cargo test -p camel-component-mail --lib message::tests::list_unsubscribe_post`;
  expected: RED before step 1, GREEN after.

**Acceptance:**
- All tests above pass, and the task 1.2 tests still pass.
- `cargo test -p camel-component-mail --lib` passes and `cargo check -p camel-component-mail` exits 0.

- [x] 1.3

## camel-component-mail: MIME rendering

### Task 1.4: render the two wire headers in `render_mime`

**Files:**
- `crates/components/camel-mail/src/adapter/smtp.rs` (modified)

**Steps:**
1. In `render_mime`, after the `msg.extra_headers` loop, add for `msg.list_unsubscribe`:
   `builder = builder.raw_header(HeaderValue::new(HeaderName::new_from_ascii(
   headers::WIRE_LIST_UNSUBSCRIBE.to_string()).map_err(transport_err)?, value.clone()));`.
2. Do the same for `msg.list_unsubscribe_post` with `headers::WIRE_LIST_UNSUBSCRIBE_POST`.
3. Import the two wire constants; the literals must not be repeated in this file.

**Tests:** (executable spec — name, arrange, act, assert)
- `render_emits_list_unsubscribe` — arrange: `outbound("&to=a@corp.test", "x",
  &[("CamelMailListUnsubscribe", "<https://example.com/u/1>")])`; act: `render(&m)`; assert: the output
  contains `List-Unsubscribe: <https://example.com/u/1>` exactly once and contains no
  `X-CamelMailListUnsubscribe`.
- `render_emits_list_unsubscribe_post` — arrange: the same list plus
  `("CamelMailListUnsubscribePost", "List-Unsubscribe=One-Click")`; assert: the output contains
  `List-Unsubscribe-Post: List-Unsubscribe=One-Click`.
- `render_omits_both_when_absent` — arrange: no such headers; assert: the output contains neither
  `List-Unsubscribe:` nor `List-Unsubscribe-Post:`.
  command: `cargo test -p camel-component-mail --lib adapter::smtp::tests::render_`;
  expected: RED before step 1 (the header is not rendered), GREEN after.

**Acceptance:**
- All three tests pass; existing `adapter::smtp::tests` pass unchanged.
- `cargo clippy -p camel-component-mail --all-targets -- -D warnings` exits 0 (first task where every
  new constant and field has a reader, so the dead-code gate belongs here).
- `cargo fmt --check` exits 0 for the crate.

- [x] 1.4

## camel-component-mail: IMAP consumer failure logging

### Task 2.1: log every failed route attempt with a redacted cause

**Files:**
- `crates/components/camel-mail/src/poll.rs` (modified)
- `crates/components/camel-mail/src/consumer.rs` (modified)
- `crates/components/camel-mail/src/consumer/tests.rs` (modified)

**Steps:**
1. In `poll.rs`, change `pub(crate) fn failed(&mut self, uid: u32)` to return `u32`; after
   `*attempts += 1`, bind `let attempt = *attempts;`, keep the `maxAttempts` remove-and-skip branch, and
   return `attempt`.
2. Add `pub(crate) fn warn_failed_attempt(&mut self, uid: u32, error_kind: &'static str)`:
   `let attempt = self.failed(uid);` then `warn!(uid, uid_validity = self.uid_validity, attempt,
   max_attempts = self.mailbox.config.max_attempts, error_kind, "mail route failed; message left unseen");`.
3. In `consumer.rs` `poll_once`, replace the catch-all arm with:
   `Ok(ex) if !ex.has_error() => cycle.delivered(uid, message_id.as_deref()).await?`,
   `Ok(ex) => cycle.warn_failed_attempt(uid, ex.error.as_ref().map_or("unknown", |e| e.variant_name()))`,
   `Err(CamelError::ChannelClosed) => return Ok(Polled::RouteClosed)`,
   `Err(e) => cycle.warn_failed_attempt(uid, e.variant_name())`.
4. In `consumer/tests.rs`, change the `Route::Fail` error text to
   `CamelError::ProcessorError("route failed at https://secret.example/token?password=hunter2 body SECRETBODY".into())`,
   and add `Route::ErrorReply`, which sets that same error on the exchange with `ex.set_error(...)` and
   returns `Ok(ex)`.

**Tests:** (executable spec — name, arrange, act, assert)
- `failed_route_attempt_is_logged` — arrange: `one(7)`, default `maxAttempts=3`, `Route::Fail`, capture
  with `capture_warnings()`; act: one `h.poll()`; assert: the log contains `uid=7`, `uid_validity=1`,
  `attempt=1`, `max_attempts=3`, `error_kind="ProcessorError"` (the fmt formatter Debug-quotes string
  fields), and `mail route failed`.
- `final_attempt_logs_true_attempt_number` — arrange: `"&maxAttempts=3"`, `Route::Fail`; act: three
  polls; assert: the third per-attempt warning carries `attempt=3` and `max_attempts=3`, and
  `skipped for the process lifetime` also appears.
- `error_bearing_success_is_a_failed_attempt` — arrange: `Route::ErrorReply`; act: one poll; assert:
  `h.state.attempts.get(&(1, 7)) == Some(&1)`, the message is unseen, and the log carries
  `error_kind="ProcessorError"`.
- `logged_cause_is_redacted` — arrange: `Route::Fail`; act: one poll; assert: the captured log contains
  none of `secret.example`, `hunter2`, `SECRETBODY`.
- extend `route_channel_closed_stops_poll` — arrange: add a `capture_warnings()` guard; act: unchanged;
  assert: additional assertion that the log contains no `mail route failed` line.
  command: `cargo test -p camel-component-mail --lib consumer::tests`;
  expected: RED before steps 1–3 (no `mail route failed` line), GREEN after.

**Acceptance:**
- All `consumer::tests` and `polling` tests pass; no existing test needs a changed assertion.
- `cargo xtask lint-log-redaction` and `cargo xtask lint-log-levels` exit 0.
- These retained scenario tests pass UNMODIFIED: `failed_route_leaves_message_untouched`,
  `poison_message_bounded`, `refused_disposition_redelivery_is_bounded`, `success_sets_seen` (the
  "success sets Seen" scenario) and `disposition_failure_counted` (the "disposition failure counted"
  scenario).
- `cargo clippy -p camel-component-mail --all-targets -- -D warnings` exits 0.

- [x] 2.1

## Docs

### Task 3.1: correct `docs/src/components/mail.md`

**Files:**
- `docs/src/components/mail.md` (modified)

**Steps:** Load the `ste-writing` skill before editing. Then:
1. Replace the sentence "Other JSON values, XML and an empty body fail the Exchange." with text that
   separates the two cases: an absent body (`Body::Empty`) fails the Exchange, while `set_body` with
   `value: ""` sets a 0-byte `text/plain` body that is sent.
2. In "Metrics and health", state that `camel_component_operations_total` (both `send` and `poll`) is
   emitted only when `[observability.metrics] components = true`, and that the `b-prime:mail:*` error
   labels come from the error family and are always emitted.
3. In "pollEnrich", state that the default `UseEnrichedBody` strategy copies only the body into the
   original exchange, keeps the original headers and properties, and discards every polled
   `CamelMailIn*` header; a route that needs them must use a custom enrichment strategy.
4. In "TLS", state that a relative `caFile` resolves against the process working directory, not against
   the directory that holds `Camel.toml`.
5. In "Accounts", state that when the file has profile sections the accounts live under
   `[default.components.mail.accounts.<name>]`, and that the flat `[components.mail.accounts.<name>]`
   form is for a config without `[default]` or `[<profile>]` sections.
6. In "Headers", add `CamelMailListUnsubscribe` and `CamelMailListUnsubscribePost` to the class-A list
   with their wire mapping, the exact one-click token, the rule that a bare standard header name is
   ignored, and the note that RFC 8058 one-click needs a DKIM signature over both headers, which this
   component does not produce.

**Tests:** (executable spec — name, arrange, act, assert)
- `mdbook_build` — act: `nix shell nixpkgs#mdbook -c mdbook build docs`; assert: exit 0.
- `mdbook_test` — act: `nix shell nixpkgs#mdbook -c mdbook test docs`; assert: exit 0.
- `grep components = true docs/src/components/mail.md` returns ≥ 1 line.
- `grep UseEnrichedBody docs/src/components/mail.md` returns ≥ 1 line.
- `grep "working directory" docs/src/components/mail.md` returns ≥ 1 line.
- `grep "\[default.components.mail.accounts." docs/src/components/mail.md` returns ≥ 1 line.
- `grep "List-Unsubscribe=One-Click" docs/src/components/mail.md` returns ≥ 1 line.
- `grep "Body::Empty" docs/src/components/mail.md` returns ≥ 1 line.
- `grep CamelMailListUnsubscribePost docs/src/components/mail.md` returns ≥ 1 line.
  expected: the five drift greps are RED before the edit, GREEN after.

**Acceptance:**
- Both mdbook commands exit 0; all eight greps return ≥ 1 line.
- Every statement matches the code (verified against `crates/components/camel-mail/CONTEXT.md`,
  `crates/camel-config/src/config.rs` root-key policy and
  `crates/camel-processor/src/enrichment_strategy.rs`).
- Prose is English and STE-compliant.

- [x] 3.1

### Task 3.2: correct `crates/components/camel-mail/README.md`

**Files:**
- `crates/components/camel-mail/README.md` (modified)

**Steps:** Load the `ste-writing` skill before editing. Then add a section titled "Behaviour notes" with
these statements, and a "## Exchange headers" section:
1. Component operation metrics require `[observability.metrics] components = true`; the
   `b-prime:mail:*` error labels are always emitted.
2. `Body::Empty` fails the send; `set_body` with `value: ""` sends a 0-byte `text/plain` body.
3. `pollEnrich` with the default `UseEnrichedBody` copies only the body; polled `CamelMailIn*` headers
   are dropped.
4. A relative `caFile` resolves against the process working directory.
5. With profile sections, accounts live under `[default.components.mail.accounts.<name>]`; the flat
   `[components.mail.accounts.<name>]` form is for profile-less files.
6. Exchange headers: class A = `CamelMailSubject`, `CamelMailContentType`, `CamelMailHtmlBody`,
   `CamelMailListUnsubscribe`, `CamelMailListUnsubscribePost`, `X-*`; class B = `CamelMailTo`,
   `CamelMailCc`, `CamelMailBcc` (fenced); class C is endpoint-only. State that
   `CamelMailListUnsubscribePost` must equal `List-Unsubscribe=One-Click`, needs an `https` URI in
   `CamelMailListUnsubscribe`, and that a bare `List-Unsubscribe` header is ignored.

**Tests:** (executable spec — name, arrange, act, assert)
- `cargo xtask lint-context-citations` exits 0.
- `grep "observability.metrics" crates/components/camel-mail/README.md` returns ≥ 1 line.
- `grep "Body::Empty" crates/components/camel-mail/README.md` returns ≥ 1 line.
- `grep UseEnrichedBody crates/components/camel-mail/README.md` returns ≥ 1 line.
- `grep "working directory" crates/components/camel-mail/README.md` returns ≥ 1 line.
- `grep "\[default.components.mail.accounts." crates/components/camel-mail/README.md` returns ≥ 1 line.
- `grep CamelMailListUnsubscribePost crates/components/camel-mail/README.md` returns ≥ 1 line.
  expected: the six greps are RED before the edit, GREEN after.

**Acceptance:**
- `cargo xtask lint-context-citations` exits 0; all six greps return ≥ 1 line.
- No code changes; `cargo test -p camel-component-mail` unaffected.

- [x] 3.2

### Task 3.3: amend ADR-0093 and the crate CONTEXT.md class-A lists

**Files:**
- `docs/adr/0093-mail-component-posture-and-header-classes.md` (modified)
- `crates/components/camel-mail/CONTEXT.md` (modified)

**Steps:**
1. In the ADR `Status` line, append
   `; amended 2026-11-01: List-Unsubscribe class-A headers (bd rc-dhd1k)`.
2. In the ADR "Class A, content" paragraph, add `CamelMailListUnsubscribe` and
   `CamelMailListUnsubscribePost` to the name list, and add one bullet: both render as
   `List-Unsubscribe` / `List-Unsubscribe-Post`, are string-only, and follow the RFC 8058 pairing rule
   (an `https` URI is required when the Post header is present).
3. In `crates/components/camel-mail/CONTEXT.md`, update the "Header classes" bullet under
   "Key design decisions" and the "Class A/B/C header" glossary entry so class A also names
   `CamelMailListUnsubscribe` and `CamelMailListUnsubscribePost`.

**Tests:** (executable spec — name, arrange, act, assert)
- `grep -c "rc-dhd1k" docs/adr/0093-mail-component-posture-and-header-classes.md` returns ≥ 1.
- `grep -c "CamelMailListUnsubscribe" docs/adr/0093-mail-component-posture-and-header-classes.md` returns ≥ 1.
- `grep -c "CamelMailListUnsubscribe" crates/components/camel-mail/CONTEXT.md` returns ≥ 1.
  expected: RED before the edits (all counts 0), GREEN after.

**Acceptance:**
- All three greps return ≥ 1.
- `cargo xtask lint-context-citations` exits 0 (no broken ADR references).

- [x] 3.3
