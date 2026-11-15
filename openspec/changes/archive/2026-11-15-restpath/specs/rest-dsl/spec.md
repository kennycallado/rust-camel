## ADDED Requirements

### Requirement: REST path template placeholder grammar

A REST path template segment SHALL recognise `{name}` placeholders anywhere
inside the segment, with literal prefix, infix, and suffix text around them,
and SHALL allow more than one placeholder in one segment. A placeholder name
SHALL be one or more characters, none of which is `{` or `}`. A bare segment
`{name}` SHALL keep its existing meaning as a single-placeholder segment.
Every placeholder in a segment SHALL contribute its name to the operation's
path-parameter set (unique per path) and SHALL set a `CamelHttpPath_<param>`
header whose value is the captured substring.

#### Scenario: mixed literal+placeholder segment loads and matches

- **GIVEN** a REST operation `GET /files/{id}.xml` in a REST block
- **WHEN** the block lowers and a request `GET /files/42.xml` arrives
- **THEN** the route loads, the request matches with `id = 42`, and the
  header `CamelHttpPath_id` has value `42`

#### Scenario: multiple placeholders in one segment

- **GIVEN** a REST operation `GET /{a}-{b}` in a REST block
- **WHEN** a request `GET /1-2-3` arrives
- **THEN** the request matches with `a = 1` and `b = 2-3`, header
  `CamelHttpPath_a` has value `1`, and header `CamelHttpPath_b` has value
  `2-3`

### Requirement: REST path parameter capture rule

Within one segment, placeholders SHALL capture left to right and each
placeholder SHALL capture the shortest substring that still lets the
remainder of the segment match. Trailing literal text SHALL NOT be part of
the captured value. Captured values MAY be empty. Brace characters in
request path values SHALL be treated as data, never as grammar.

#### Scenario: trailing literal is not captured

- **GIVEN** a REST operation `GET /report-{date}.json` in a REST block
- **WHEN** a request `GET /report-2026-11-15.json` arrives
- **THEN** the request matches with `date = 2026-11-15` and NOT
  `2026-11-15.json`

#### Scenario: adjacent placeholders capture leftmost-shortest

- **GIVEN** a REST operation `GET /{a}{b}` in a REST block
- **WHEN** a request `GET /123` arrives
- **THEN** the request matches with `a = ""` and `b = "123"`

#### Scenario: three placeholders resolve left to right

- **GIVEN** a REST operation `GET /{a}-{b}-{c}` in a REST block
- **WHEN** a request `GET /1-2-3-4` arrives
- **THEN** the request matches with `a = 1`, `b = 2`, and `c = 3-4`

#### Scenario: empty mixed-segment capture is permitted

- **GIVEN** a REST operation `GET /a{y}bc` in a REST block
- **WHEN** a request `GET /abc` arrives
- **THEN** the request matches with `y = ""`

#### Scenario: brace text in request values is data

- **GIVEN** a REST operation `GET /files/{id}.xml` in a REST block
- **WHEN** a request `GET /files/{oops}.xml` arrives
- **THEN** the request matches with `id = {oops}`

### Requirement: REST path template fail-closed malformed rejection

A path template whose segment contains a raw `{` or `}` not consumed by a
well-formed placeholder (stray brace, unclosed placeholder, empty name)
SHALL fail route load with an error naming the offending path. It SHALL NOT
degrade to a literal segment. For templates built only from brace-free
literal segments and whole-segment placeholders, matching semantics and
precedence are unchanged; mixed literal+placeholder templates change from
literal matching to parameter capture by design, and previously undetected
malformed or ambiguous templates become load errors.

#### Scenario: unclosed placeholder fails load

- **GIVEN** a REST operation with path `/files/{id.xml`
- **WHEN** the block lowers
- **THEN** lowering fails with a route-load error that names the path
  `/files/{id.xml`

#### Scenario: stray closing brace fails load

- **GIVEN** a REST operation with path `/files/id}.xml`
- **WHEN** the block lowers
- **THEN** lowering fails with a route-load error that names the path
  `/files/id}.xml`

#### Scenario: empty placeholder name fails load

- **GIVEN** a REST operation with path `/files/{}.xml`
- **WHEN** the block lowers
- **THEN** lowering fails with a route-load error that names the path and
  states that parameter names are required

### Requirement: REST path template specificity and ambiguity

Template specificity SHALL be the pair `(L, C)` where `L` is the number of
segments containing at least one literal part (a bare `{name}` segment
contributes 0; a mixed or literal-only segment contributes 1) and `C` is
the total number of literal characters across all segments. Two templates
SHALL be compared lexicographically on `(L, C)`; the higher pair is more
specific. On templates made only of bare-placeholder and literal-only
segments, `L` equals the legacy literal-segment count, so legacy precedence
is preserved. At match time the most specific matching candidate SHALL win.
Two operations on the same listener (host and port) and the same HTTP
method, across all REST blocks bound to that listener, SHALL fail route
load as ambiguous when their templates have equal specificity pairs and at
least one request path matches both templates (per-segment language overlap
at every segment position).

#### Scenario: literal segment beats mixed segment

- **GIVEN** operations `GET /fixed.xml` and `GET /{id}.xml` on the same
  listener and method
- **WHEN** a request `GET /fixed.xml` arrives
- **THEN** `/fixed.xml` is selected, `id` is not set, and no ambiguity is
  reported

#### Scenario: longer mixed literal beats shorter mixed literal

- **GIVEN** operations `GET /{p}y` and `GET /{q}xy` on the same listener
  and method
- **WHEN** a request `GET /1xy` arrives
- **THEN** `/{q}xy` is selected with `q = 1`

#### Scenario: legacy multi-segment precedence is preserved

- **GIVEN** operations `GET /a/{x}/{y}` and `GET /{u}/b/c` on the same
  listener and method
- **WHEN** a request `GET /a/b/c` arrives
- **THEN** `/{u}/b/c` is selected with `u = a`, matching the legacy
  literal-segment-count precedence

#### Scenario: equal-specificity overlapping templates are ambiguous

- **GIVEN** operations `GET /{id}.xml` and `GET /{name}.xml` on the same
  listener and method
- **WHEN** the block lowers
- **THEN** lowering fails with an ambiguity error naming both operations

#### Scenario: overlapping mixed segments with different part order are ambiguous

- **GIVEN** operations `GET /ab{x}` and `GET /a{y}b` on the same listener
  and method
- **WHEN** the block lowers
- **THEN** lowering fails with an ambiguity error naming both operations,
  because both templates match `GET /abb`

#### Scenario: prefix and suffix forms of the same literal are ambiguous

- **GIVEN** operations `GET /a{x}` and `GET /{y}a` on the same listener
  and method
- **WHEN** the block lowers
- **THEN** lowering fails with an ambiguity error naming both operations,
  because both templates match `GET /aa`

#### Scenario: mirrored literal positions across segments are ambiguous

- **GIVEN** operations `GET /x/{id}` and `GET /{a}/y` on the same listener
  and method
- **WHEN** the block lowers
- **THEN** lowering fails with an ambiguity error naming both operations,
  because both templates match `GET /x/y`

#### Scenario: ambiguity spans REST blocks on the same listener

- **GIVEN** two REST blocks bound to the same host and port, one with
  `GET /{id}.xml` and the other with `GET /{name}.xml`
- **WHEN** the document lowers
- **THEN** lowering fails with an ambiguity error naming both operations

#### Scenario: different listeners or different methods are independent

- **GIVEN** `GET /{id}.xml` and `GET /{name}.xml` on different listeners,
  and `GET /{id}.xml` with `POST /{name}.xml` on one listener
- **WHEN** the document lowers
- **THEN** lowering succeeds and no ambiguity is reported

### Requirement: REST path template parser agreement

The runtime matcher (`camel-component-http`) and the DSL validator
(`camel-dsl`) SHALL implement one grammar: for every template, both parsers
SHALL classify segments identically (same literal/placeholder structure and
names) or reject identically (same malformed classification).

#### Scenario: shared corpus classifies and rejects identically

- **GIVEN** a corpus covering bare placeholders, mixed segments,
  multi-placeholder segments, plain literals, previously accepted
  placeholder names such as `{user-id}`, and every malformed form
- **WHEN** the corpus is parsed by both implementations
- **THEN** both yield identical segment classifications for the valid
  templates and identical rejections for the malformed ones
