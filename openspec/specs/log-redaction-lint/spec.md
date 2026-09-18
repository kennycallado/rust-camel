# log-redaction-lint Specification

## Purpose
TBD - created by archiving change redactstrict. Update Purpose after archive.
## Requirements
### Requirement: Call-shape redact redemption for identifier references

The `lint-log-redaction` xtask SHALL redeem a sensitive-identifier argument
segment only when the segment contains a redact identifier in call shape: an
identifier whose lowercase form contains `redact` immediately followed by its
parenthesized argument group. Redemption SHALL be span-local — an identifier
merely named like a redact helper (bare `redacted_url`), a function-pointer
reference (`map(redact_url)`), or a redact call in a different argument
segment SHALL NOT redeem.

#### Scenario: bare redact-named ident no longer redeems

- **GIVEN** a log macro argument `url = %redacted_url` where `redacted_url`
  is a bare identifier (no call) holding an unredacted value
- **WHEN** `lint-log-redaction` scans the segment
- **THEN** it reports a violation ("sensitive url/uri/config value not
  wrapped in a redact_* helper — see ADR-0076")

#### Scenario: function-pointer reference no longer redeems

- **GIVEN** a log macro segment containing `redact_url` passed as a value
  (e.g. inside `map(redact_url)`) with no parenthesized call group adjacent
- **WHEN** the segment references a sensitive identifier
- **THEN** the lint reports a violation

#### Scenario: free-function call shape redeems

- **GIVEN** a segment `url = redact_url(&u)`
- **WHEN** the lint scans the segment
- **THEN** no violation is reported for that segment

#### Scenario: path-qualified call shape redeems

- **GIVEN** a segment `url = camel_api::redact::redact_url(&u)`
- **WHEN** the lint scans the segment
- **THEN** no violation is reported for that segment

#### Scenario: method call shape redeems

- **GIVEN** a segment `url = u.to_redacted_string()`
- **WHEN** the lint scans the segment
- **THEN** no violation is reported for that segment

#### Scenario: pre-redacted local outside the macro is not recognized

- **GIVEN** `let url = redact_url(&u);` followed by `debug!(url = %url)`
- **WHEN** the lint scans the macro
- **THEN** it reports a violation, because redemption is span-local; the call
  site must wrap (`%redact_url(&u)`) or use an escape hatch

### Requirement: Binding-local redemption for message captures

For a sensitive identifier capture (`{url}` and peers) inside a log macro
message literal, the lint SHALL redeem the capture only when the argument
segment that binds the captured identifier (`url = <expr>`) itself satisfies
the call-shape redemption predicate. A redact identifier or call anywhere else
in the macro SHALL NOT redeem the capture. A sensitive capture with no binding
segment in the macro (implicit capture of a local) SHALL be a violation.

#### Scenario: capture redeemed by its own binding segment

- **GIVEN** `debug!("at {url}", url = redact_url(&u))`
- **WHEN** the lint scans the macro
- **THEN** no violation is reported

#### Scenario: capture not redeemed by unrelated redact call

- **GIVEN** `debug!("at {url}", url = raw, other = redact_seed(x))` — a
  redact call in a segment that does not bind `url`
- **WHEN** the lint scans the macro
- **THEN** it reports a message-capture violation

#### Scenario: implicit capture is unredeemable

- **GIVEN** `debug!("at {url}")` with no `url = ...` binding segment in the
  macro (implicit capture of the raw local) and a redact call in an unrelated
  argument
- **WHEN** the lint scans the macro
- **THEN** it reports a message-capture violation

#### Scenario: prose words in message literals do not fire

- **GIVEN** a message literal `"direct endpoint created"` (English prose, no
  `{ident}` capture)
- **WHEN** the lint scans the macro
- **THEN** no violation is reported — message-literal detection is
  `{ident}`-capture-regex only, never substring word matching

### Requirement: Sensitive identifier set

The lint's sensitive value-identifier set SHALL contain exactly the
workspace-sensitive names: `url`, `uri`, `base_url`, `db_url`, `jdbc_url`,
`broker_url`, `connection_string`, `dsn`, `config`, `endpoint`, `address`,
`host`, `remote`. Matching SHALL be exact-identifier and leaf-or-standalone:
object position (`config.topic`) is not a hit; standalone (`%host`), field
name (`address = ...`), and leaf (`self.remote`) are. The message-capture
regex alternation SHALL contain the same names.

#### Scenario: endpoint field name is caught

- **GIVEN** a log macro argument `endpoint = %value` with no redact call
- **WHEN** the lint scans the segment
- **THEN** it reports a violation

#### Scenario: host leaf access is caught

- **GIVEN** a log macro argument `host = %self.host` with no redact call
- **WHEN** the lint scans the segment
- **THEN** it reports a violation

#### Scenario: suffixed variants are not caught

- **GIVEN** log macro arguments referencing `remote_addr`, `host_name`, or
  `endpoint_id`
- **WHEN** the lint scans the segments
- **THEN** no violation is reported (exact-identifier matching)

#### Scenario: qualified sensitive leaf is caught

- **GIVEN** a log macro argument `host = %config.host` — `config` merely
  qualifies the leaf, and `host` is the sensitive leaf identifier
- **WHEN** the lint scans the segment
- **THEN** it reports a violation

#### Scenario: object position is not caught

- **GIVEN** a log macro argument referencing `config.topic` (sensitive
  identifier in object position qualifying a benign leaf)
- **WHEN** the lint scans the segment
- **THEN** no violation is reported

#### Scenario: captures resolve independently

- **GIVEN** `debug!("{url} {uri}", url = redact_url(&u), uri = raw)` — the
  `url` binding is redeemed but the `uri` binding is not
- **WHEN** the lint scans the macro
- **THEN** it reports a violation for the `uri` capture only; each matched
  sensitive capture is resolved against its own binding segment
  independently

#### Scenario: capture of an extended name is caught

- **GIVEN** a message literal `"bound to {address}"` with no redeeming
  binding segment
- **WHEN** the lint scans the macro
- **THEN** it reports a message-capture violation

### Requirement: Escape hatches and corpus state preserved

The escape hatches SHALL keep their semantics: `// allow-log-redaction` on
the macro's start line, and `scripts/xtask/allowlist-log-redaction.txt`
entries (`<relative path>:<line>`). After this change lands, the lint SHALL
report zero violations on the workspace corpus; every allowlist entry added
by this change SHALL carry a justification comment stating why the logged
value is credential-free.

#### Scenario: inline escape hatch still suppresses

- **GIVEN** a macro with a sensitive argument and `// allow-log-redaction`
  on its start line
- **WHEN** the lint scans the file
- **THEN** no violation is reported

#### Scenario: full corpus is clean after remediation

- **GIVEN** the workspace source corpus (non-test `src/**/*.rs`, excluding
  `scripts/xtask`, `target`, `.worktrees`)
- **WHEN** `cargo xtask lint-log-redaction` runs after this change and its
  corpus remediation
- **THEN** it reports OK (0 violations)

### Requirement: Documented detection blind spots

The lint's span-local token-walk contract SHALL document (not detect) the
following consumption shapes, which can carry a sensitive value without a
violation: sensitive identifier in object position of a value-transforming
method chain (`self.url.clone()`, `url.to_string()`, `url.as_str()`); a
sensitive value nested inside a parenthesized group (`(url)`,
`helper(url)`); a `{ident}` capture nested inside a `format!` argument
passed to the log macro; dotted captures (`{conn.url}`); and redemption
breadth — any identifier containing `redact` followed by a parenthesized
group redeems its segment, including boolean predicates such as
`should_redact(x)`. The pre-existing structural blind spots (`span!` /
`*_span!` field sets, aliased macro imports, non-tracing sinks) remain
documented in the lint's module documentation. Tracked as bd rc-cgen3.

#### Scenario: method-chain object position is a documented blind spot

- **GIVEN** a log macro argument `location = %self.url.clone()` (the
  binding key is not sensitive; the sensitive identifier is in object
  position of a value-transforming method chain)
- **WHEN** the lint scans the segment
- **THEN** no violation is reported (documented blind spot — a sensitive
  binding key such as `url = ...` WOULD fire via rule 1 regardless of the
  value shape)

#### Scenario: redact-named boolean predicate redeems

- **GIVEN** a log macro argument
  `location = if should_redact(&u) { u.clone() } else { u.clone() }` (a
  non-sensitive binding key whose segment contains a redact-named call
  that is only a boolean predicate)
- **WHEN** the lint scans the segment
- **THEN** no violation is reported (any redact-named identifier in call
  shape redeems the segment — documented redemption breadth)

