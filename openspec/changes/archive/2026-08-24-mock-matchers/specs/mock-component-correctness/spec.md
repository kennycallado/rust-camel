## ADDED Requirements

### Requirement: Body and header matcher vocabulary

The mock component SHALL expose a public, synchronous matcher vocabulary
for assertions: `BodyMatcher` with variants `Equals`, `Regex`, `Contains`,
`StartsWith`, `EndsWith`, `Exists`, `JsonSubset`, and `HeaderMatcher` with
variants `Equals`, `Regex`, `Exists`. Each variant SHALL provide a
`matches` predicate over the received body or header value and a Display
form naming the matcher kind and its pattern or value. Matchers SHALL be
assertion-side only: they SHALL NOT change producer behavior (the producer
stays a sink per the component identity ruling).

String semantics. The string matchers `Equals`, `Regex`, `Contains`,
`StartsWith`, and `EndsWith` operate on `Body::Text` (for `Equals`, also
`Body::Json` by structural equality, matching the existing variant-tagged
`body_eq` behavior). Against any other body variant (`Json` for the
non-`Equals` string matchers, `Xml`, `Bytes`, `Stream`, `Empty`) they
SHALL fail naming that the body is not text. `Exists` SHALL pass for every
body variant except `Empty`. Matcher payloads: `Regex`, `Contains`,
`StartsWith`, and `EndsWith` take a string payload; `Equals` takes the
value class valid for its position (a string, or any JSON structure where
literal JSON equality is expressible); `Exists` takes no payload;
`JsonSubset` takes a JSON object.

Header semantics. `Exists` SHALL pass when the header key is present
(regardless of value, including JSON null) and fail naming the absent key
otherwise. `Equals` SHALL compare the received JSON value for equality
(null equals null). `Regex` SHALL require the received value to be a
string; a non-string or absent value fails the matcher naming the header
and the received values.

`JsonSubset` semantics. The pattern SHALL be a JSON object. The received
body SHALL be `Body::Json` or a `Body::Text` that parses as JSON (a text
that does not parse fails the matcher stating the body is not JSON). The
received top-level JSON SHALL be an object; any other top-level shape
fails the matcher stating the body is not a JSON object. Objects match
recursively: every pattern key SHALL exist in the received object with a
value that is either JSON-equal or, for objects, a recursive subset.
Pattern keys absent from the received object fail the matcher. A pattern
value of `null` requires the received key to hold JSON null. Arrays
compare exactly — same length, same order, elements JSON-equal (nested
objects compare by full equality, not subset).

Matcher evaluation SHALL slot into the existing body-list machinery
without changing its invariants: ordered lists keep position-indexed
matching, the component's any-order surface (where present) keeps its
matching rules, count enforcement is unchanged, and mismatch latching
(including `fail_fast_error`) behaves exactly as for exact bodies. An
invalid `Regex` pattern SHALL surface as an error (the existing
malformed-pattern class), never as a pass and never latched as a
mismatch.

#### Scenario: regex body matcher pass and fail

- **Given** a mock endpoint with `expect_body_matcher(BodyMatcher::Regex("^order-[0-9]+$"))` and a received body `order-42`
- **When** `try_assert_satisfied` evaluates
- **Then** the expectation passes; with body `refunded-42` it fails naming the regex matcher, the pattern, and the received body

#### Scenario: substring and anchor matchers

- **Given** matchers `Contains("total")`, `StartsWith("order-")`, and `EndsWith("-42")` against body `order-total-42`
- **When** each evaluates
- **Then** all three pass

#### Scenario: exists matcher

- **Given** `BodyMatcher::Exists` against `Body::Text("x")` and against `Body::Empty`
- **When** evaluation runs
- **Then** the first passes and the second fails naming the exists matcher

#### Scenario: string matchers fail on non-text bodies

- **Given** `Contains("a")` against `Body::Json` and `Body::Bytes`
- **When** evaluation runs
- **Then** the matcher fails in both cases stating the body is not text

#### Scenario: jsonSubset recursive subset ignores extra fields

- **Given** `JsonSubset({"status": "ok", "meta": {"seq": 3}})` against body `{"id": 7, "status": "ok", "meta": {"seq": 3, "ts": 9}}`
- **When** evaluation runs
- **Then** the expectation passes (top-level and nested extra fields ignored)

#### Scenario: jsonSubset arrays compare exactly

- **Given** `JsonSubset({"tags": ["a", "b"]})` against body `{"tags": ["b", "a"]}`
- **When** evaluation runs
- **Then** the expectation fails naming the jsonSubset matcher and the received array

#### Scenario: jsonSubset parses text bodies as JSON

- **Given** `JsonSubset({"status": "ok"})` against `Body::Text("{\"status\": \"ok\"}")`
- **When** evaluation runs
- **Then** the expectation passes; against `Body::Text("ok")` it fails stating the body is not JSON

#### Scenario: jsonSubset null pattern value requires null

- **Given** `JsonSubset({"err": null})` against body `{"err": null}` and against body `{"err": 0}`
- **When** evaluation runs
- **Then** the first passes and the second fails naming the key and the received value

#### Scenario: header matchers on null and missing values

- **Given** headers `X-A: null` and no `X-B`, with `HeaderMatcher::Exists` on `X-A`, `Exists` on `X-B`, `Equals(null)` on `X-A`, and `Regex("^a$")` on `X-A`
- **When** evaluation runs
- **Then** `Exists` on `X-A` passes, `Exists` on `X-B` fails naming the absent key, `Equals(null)` passes, and `Regex` fails stating the value is not a string

#### Scenario: header matcher setter

- **Given** a mock endpoint with `expect_header_matcher("X-Trace", HeaderMatcher::Regex("^[a-f0-9]{8}$"))` and a received header `ab12cd34`
- **When** evaluation runs
- **Then** the expectation passes; with header `xyz` it fails naming the header, the regex matcher, and the received values

#### Scenario: matcher list preserves ordered semantics

- **Given** `expect_body_matcher` called twice (first `Regex("^a-")`, then `Regex("^b-")`) and bodies received in order `a-1`, `b-2`
- **When** evaluation runs
- **Then** the expectation passes; with order `b-2`, `a-1` it fails per the existing ordered-body mismatch behavior

#### Scenario: invalid regex is an error, never a silent pass

- **Given** a matcher constructed with pattern `(unclosed`
- **When** evaluation or construction encounters the invalid pattern
- **Then** an error naming the pattern is surfaced (malformed-pattern class, not latched as `fail_fast_error`); the expectation never reports a pass

### Requirement: Matcher-aware assertion diagnostics

Assertion failures from matchers SHALL be reported through the existing
`MockAssertionError` surface with new variants or fields that name the
matcher kind, its pattern or value, and the received body (or header
values), so `camel test` FAIL lines identify which matcher failed without
guessing. The received value inside a matcher-failure message SHALL be
rendered whole (not truncated). This does not alter existing diagnostic
list caps elsewhere in the assertion surface (for example, the cap on the
number of header values listed in a mismatch).

#### Scenario: failure text identifies matcher and received body

- **Given** an ordered body expectation `Regex("^ok$")` at index 1 receiving `denied`
- **When** the assertion fails
- **Then** the error text contains the index, the matcher kind (`regex`), the pattern, and `denied`

#### Scenario: header matcher failure lists received values

- **Given** `expect_header_matcher("X-Trace", Regex("^[a-f0-9]{8}$"))` with last-received headers carrying `X-Trace: zzzz`
- **When** the assertion fails
- **Then** the error text names the header key, the matcher, and the received values
