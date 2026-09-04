# Design: itest-send-method

## Approach

Three layers, one field.

Adapter seam: `OutgoingMessage` (adapters.rs) gains
`pub method: String`, threaded from the resolved action by the
runner's send dispatch and populated at every construction site. The
HTTP adapter converts it with `Method::from_str(&msg.method)`.

Grammar layer (`document.rs`): `RawSend` gains
`method: Option<String>` under the existing `deny_unknown_fields` and
`camelCase` serde attributes. Validation, which already names the
action index for the deadline field, learns one rule: a present
method is trimmed, normalized with `to_ascii_uppercase`, and must be
a non-empty RFC 7230 token. The check is a small pure `is_http_token`
predicate: each character is an ASCII alphanumeric or one of the
token characters `!` `#` `$` `%` `&` `'` `*` `+` `-` `.` `^` `_`
`` ` `` `|` `~`. The grammar layer keeps no dependency on the `http`
crate. An invalid token reports `doc-validation` with the action
index, identical in shape to the missing-deadline error. The typed
`Send` action carries the resolved method as a `String`: explicit when
the field is present. Otherwise the legacy inference (`POST` with a
body, `GET` without) is computed once here. The adapter then has a
single source of truth.

Adapter layer (`adapters/http.rs`, behind the `http` feature): the
inline `body?POST:GET` match is deleted. The builder already runs
inside the feature gate, so the `http` dependency stays demand-gated.
A grammar-validated token always parses as a `Method`. The
conversion cannot fail at runtime for a validated document.

Docs layer: the crate README's scenario vocabulary and example gain
the `method` field, the inference default, and the uppercase
normalization. The book testing chapter does not document the action
grammar.

## Affected crates

- `camel-integration-test`: `document.rs` grammar and validation,
  `adapters.rs` message seam, `runner.rs` threading, `adapters/http.rs`
  request builder, unit and e2e tests.
- Docs: `crates/camel-integration-test/README.md`.

## Architecture boundaries

The change stays inside the ADR-0069 grammar and adapter planes. The
grammar layer gains a pure predicate and a defaulted field. No
transport, runtime, or core surface moves. Validation runs without
the `http` feature, preserving demand-gated activation: a
doc-validation failure is diagnosed the same way whether or not HTTP
adapters are compiled. The exit taxonomy, tier derivation, and
hermeticity layers are untouched.


## Resolved during implementation

- Task 1.5 (adoption surfaces) was added after task 1.4 at the human's
  request, before the holistic review. The first shape, a scenario
  send straight to the harness partner, was not expressible: the
  client role dials the declared URI literally and harness endpoints
  carry the `:0` router-key port. Blocked with evidence and tracked in
  bd rc-gz2r (p2, out of scope here). The shipped example demonstrates
  the explicit method against the system under test instead: an
  inbound route with `?httpMethod=PUT` REST dispatch serves the
  discriminator, a scratch GET run fails on status 404. The example
  README section and a short scenario pointer subsection in the book
  testing chapter shipped alongside. The crate README remains the
  normative grammar reference.
