## MODIFIED Requirements

### Requirement: Expectation evaluation via change #1 API

For each entry in `expects`, the runner SHALL obtain the endpoint via
`MockComponent::get_endpoint` (name = URI suffix after `mock:`), map fields to
change #1 setters (`count` → `expect_count`, `minCount` →
`expect_minimum_count`, `maxCount` → `expect_maximum_count`, `bodies` →
`expect_body_matcher` (bare strings as `equals`, matcher maps as their
matcher), `headers` → `expect_header_matcher` (literal values as `equals`,
matcher maps as their matcher)), and evaluate with `try_assert_satisfied()`.
`count` SHALL be mutually exclusive with `minCount` and with `maxCount` (a
document error, exit 2); `minCount` together with `maxCount` SHALL mean the
inclusive range `[minCount, maxCount]`, and `minCount` greater than
`maxCount` SHALL be a document error (exit 2). Assertion failures SHALL be reported
without aborting remaining endpoints. Header regex matchers SHALL reach the
existing header-regex engine; matcher mismatches SHALL be assertion failures
(exit 1) whose error text names the matcher kind, its pattern or value, and
the received body or header values.

#### Scenario: body and count expectations pass

- **Given** a running document with `expects: {mock:result: {count: 2, bodies: ["a", "b"]}}` and inputs producing exactly those bodies in order
- **When** evaluation runs
- **Then** the endpoint reports PASS and the summary counts it as passed

#### Scenario: mismatch reports change #1 error detail

- **Given** `expects: {mock:result: {count: 3}}` with only 2 exchanges received
- **When** evaluation runs
- **Then** the endpoint reports FAIL with the `MockAssertionError` text containing "expected 3 exchanges, got 2", remaining endpoints still evaluate, and the process exits 1

#### Scenario: count and minCount together rejected

- **Given** a document entry containing both `count` and `minCount`
- **When** `camel test` parses the document
- **Then** parsing fails with exit code 2 stating they are mutually exclusive

#### Scenario: count and maxCount together rejected

- **Given** a document entry containing both `count` and `maxCount`
- **When** `camel test` parses the document
- **Then** parsing fails with exit code 2 stating they are mutually exclusive

#### Scenario: minCount greater than maxCount rejected

- **Given** a document entry containing `{minCount: 3, maxCount: 2}`
- **When** `camel test` parses the document
- **Then** parsing fails with exit code 2 stating the range is empty

#### Scenario: minCount and maxCount form an inclusive range

- **Given** `expects: {mock:result: {minCount: 1, maxCount: 2}}` and inputs producing 2 exchanges
- **When** evaluation runs after settling
- **Then** the endpoint reports PASS; with 3 exchanges it reports FAIL with text in the house style ("expected at most 2 exchanges, got 3" form)

#### Scenario: maxCount zero asserts absence after settling

- **Given** `expects: {mock:silent: {maxCount: 0}}` on an endpoint that received no exchanges, and a route wired so one exchange WOULD arrive late without settling
- **When** evaluation runs after the settle window
- **Then** the endpoint reports PASS when nothing arrived during the window; a same-window arrival makes it FAIL with the at-most error text

#### Scenario: unknown mock endpoint fails the document

- **Given** `expects: {mock:ghost: {count: 1}}` where no route creates `mock:ghost`
- **When** evaluation runs
- **Then** the endpoint reports FAIL with a message naming `ghost` as absent, and the process exits 1

#### Scenario: regex body matcher passes on nondeterministic content

- **Given** a document whose route emits a body matching `^order-[0-9]+$` and `expects: {mock:result: {count: 1, bodies: [{regex: "^order-[0-9]+$"}]}}`
- **When** evaluation runs
- **Then** the endpoint reports PASS

#### Scenario: body matcher mismatch names the matcher

- **Given** a route emitting `total: 12` and `expects: {mock:result: {count: 1, bodies: [{contains: "total: 13"}]}}`
- **When** evaluation runs
- **Then** the endpoint reports FAIL with text naming the `contains` matcher, its value, and the received body, and the process exits 1

#### Scenario: header regex matcher evaluated

- **Given** a route emitting header `X-Trace: ab12cd34` and `expects: {mock:result: {count: 1, headers: {X-Trace: {regex: "^[a-f0-9]{8}$"}}}}`
- **When** evaluation runs
- **Then** the endpoint reports PASS

#### Scenario: jsonSubset partial body match passes

- **Given** a route emitting the JSON body `{"id": 7, "status": "ok", "meta": {"ts": "...", "seq": 3}}` and `expects: {mock:result: {count: 1, bodies: [{jsonSubset: {status: "ok", meta: {seq: 3}}}]}}`
- **When** evaluation runs
- **Then** the endpoint reports PASS (unmatched fields are ignored; nested subset matches)

## ADDED Requirements

### Requirement: shared matcher algebra consumption

The mock component's matcher evaluation SHALL consume the shared
`camel-matchers` algebra — string verbs (`regex`, `contains`, `startsWith`,
`endsWith`) and `jsonSubset` SHALL delegate to
`camel_matchers::expectation_matches` through per-tier projections of the
received `Body` (`Text(s)` projected as the JSON string `s` for string verbs;
non-text bodies project to nothing and fail closed; JSON bodies project their
value for `jsonSubset`, with a non-object pattern still failing before
delegation). Variant-tagged equality (`equals` via `body_eq`) and `exists`
SHALL remain observation-typed evaluations inside the mock. The mock's public
assertion API and diagnostics (matcher names, mismatch notes, error text)
SHALL be byte-identical to the pre-change behavior.

#### Scenario: string verbs delegate through the text projection

- **Given** a received body `Body::Text("order-42")` and the matcher `Regex("^order-[0-9]+$")`
- **When** the matcher evaluates
- **Then** it passes, and the identical verdict comes from `camel_matchers::expectation_matches` over the projected JSON string

#### Scenario: non-text bodies fail closed for string verbs

- **Given** a received `Body::Json` body and the matcher `Contains("total")`
- **When** the matcher evaluates
- **Then** it fails with the "body is not text" mismatch note, exactly as before the change

#### Scenario: no duplicate json-subset logic remains

- **Given** the mock component's source
- **When** searched for a local recursive JSON-subset implementation
- **Then** none exists; the evaluation path routes through `camel_matchers`

#### Scenario: camel-test exposes the shared types

- **Given** a programmatic user of the `camel-test` kit
- **When** asserting on mock endpoints
- **Then** the matcher and bound types visible at the kit's surface are the `camel_matchers` types (re-exported), not mock-private duplicates
