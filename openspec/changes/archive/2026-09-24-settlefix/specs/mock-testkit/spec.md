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
- **When** evaluation runs after settling completes
- **Then** the endpoint reports PASS when nothing arrived before settling completes; an arrival before settling completes makes it FAIL with the at-most error text

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

### Requirement: Settling before assertion

The runner SHALL settle traffic before evaluating, driven by completion
notifications — not periodic sampling. The document's mode is structural:
completion mode when no route consumes from a self-firing source, stability
mode when some route does. In the lean registry the only self-firing source
is `timer:` (direct, log, mock, seda are demand-driven).

Completion mode: settle SHALL complete when the harness receives the
in-flight quiescence notification — the context-global accepted-not-completed
counter releasing its last claim. No quiet window SHALL be imposed. The
settle timeout starts when settling begins (after input delivery, so
delivery time never consumes the settle budget) and equals the declared
`settle:` value (default 5 seconds). Deadline precedence: at entry and on
every wake, an expired deadline yields the timeout failure before any idle
acceptance; an idle counter with an unexpired deadline completes
immediately.

Stability mode: future emissions from a self-firing source are unclaimed,
so quiescence of the counter is not completion. The quiet window (default
250ms, `settle:` override) must elapse with no change in the expected
endpoints' `received_count` — every arrival notification that changes a
sampled count resets the window. The document-wide settle deadline starts
when route execution begins and equals one full quiet window plus a
5-second instability budget (so any valid `settle` value can always satisfy
its own window).

Both modes: count values above expectations do NOT end settling — only the
mode's completion condition does. Hitting the deadline without settling
SHALL fail the document with a settle-timeout message (exit 1), never hang.
Documents SHALL continue to execute sequentially in CLI argument order with
settle confined to one document at a time.

#### Scenario: lean document settles on the completion notification

- **Given** a document whose routes consume from no self-firing source, whose input-triggered traffic completes
- **When** the in-flight counter releases its last claim
- **Then** evaluation proceeds promptly — no quiet window and no sampling floor delays it

#### Scenario: timer route settles before assertion

- **Given** a timer-driven route emitting 3 exchanges and `expects: {mock:result: {count: 3}}`
- **When** the counts are stable for the quiet window within the deadline
- **Then** evaluation proceeds and passes

#### Scenario: count change resets the quiet window

- **Given** a stability-mode endpoint whose `received_count` changes 100ms into a 250ms quiet window
- **When** the arrival notification is received and the sample differs
- **Then** the quiet window restarts from that change and evaluation waits for a full stable window or the deadline

#### Scenario: unstable traffic hits the deadline

- **Given** a route still emitting when the settle deadline (quiet window + 5-second budget) is reached
- **When** the deadline hits
- **Then** the document fails with a settle-timeout message and exit code 1

#### Scenario: settle timeout fires when no completion arrives

- **Given** a completion-mode document whose in-flight work never completes (a stuck claim) and `settle: 50ms`
- **When** the settle timeout elapses
- **Then** the document fails with a settle-timeout message and exit code 1 — the runner never hangs
