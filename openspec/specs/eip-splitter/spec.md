# eip-splitter Specification

## Purpose
TBD - created by archiving change splitter-fail-loud. Update Purpose after archive.
## Requirements
### Requirement: Split fails loud on wrong body type

The splitter SHALL return a typed error when the exchange body type does not match the split expression input contract. The error SHALL be `CamelError::TypeConversionFailed`. This SHALL apply to the eager splitter service, the split segment, and the declarative split compiler.

#### Scenario: BodyLines rejects a JSON body

- **GIVEN** an exchange with `Body::Json` and the `body_lines` split expression
- **WHEN** the splitter runs
- **THEN** it returns `TypeConversionFailed` and the message contains `body_lines`, the received type name `json`, the expected `text`, and the phrase `add an unmarshal step before split`

#### Scenario: BodyJsonArray rejects a text body

- **GIVEN** an exchange with `Body::Text` and the `body_json_array` split expression
- **WHEN** the splitter runs
- **THEN** it returns `TypeConversionFailed` and the message contains `body_json_array`, the received type name `text`, the expected `json (array)`, and the phrase `add an unmarshal step before split`

#### Scenario: BodyJsonArray rejects a non-array JSON body

- **GIVEN** an exchange with `Body::Json(Object)` and the `body_json_array` split expression
- **WHEN** the splitter runs
- **THEN** it returns `TypeConversionFailed` and the message contains `json (non-array)` and the unmarshal phrase

#### Scenario: Declarative split matches the eager splitter

- **GIVEN** a declarative split step whose expression evaluates to a `Value` that is neither `String` nor `Array`
- **WHEN** the compiled split segment runs
- **THEN** the outcome is `Failed` with `TypeConversionFailed`, the message contains `declarative split`, the received value type name, the expected `text or array`, and the unmarshal phrase, and the original exchange is not cloned as a single fragment

#### Scenario: Split segment error is a failure, not a stop

- **GIVEN** a split segment whose expression returns the typed error
- **WHEN** the segment runs
- **THEN** the outcome is `Failed` and carries the typed error

### Requirement: Empty content keeps pass-through

The splitter SHALL return the original exchange unchanged when the body is empty or the split yields zero fragments over correct-type content.

#### Scenario: Empty body passes through

- **GIVEN** an exchange with `Body::Empty`
- **WHEN** the splitter runs with any built-in expression
- **THEN** it returns the original exchange with success semantics

#### Scenario: Empty text yields zero fragments

- **GIVEN** an exchange with `Body::Text("")` and the `body_lines` expression
- **WHEN** the splitter runs
- **THEN** it returns the original exchange unchanged

#### Scenario: Empty array yields zero fragments

- **GIVEN** an exchange with `Body::Json([])` and the `body_json_array` expression
- **WHEN** the splitter runs
- **THEN** it returns the original exchange unchanged

### Requirement: Streaming splitter uses the typed error

The streaming splitter SHALL return `TypeConversionFailed` with the mandated message when the body is not `Body::Stream`.

#### Scenario: Streaming splitter rejects a non-stream body

- **GIVEN** an exchange with `Body::Text` and the streaming splitter
- **WHEN** the splitter pulls its first item
- **THEN** it returns `TypeConversionFailed` and the message contains `streaming split`, the received type name `text`, the expected `stream`, and the phrase `add an unmarshal step before split`

### Requirement: Split error diagnostics carry no payload

A split type error SHALL name the expression kind, the received body variant, the expected type, and an unmarshal remediation hint. The error message SHALL NOT contain body content.

#### Scenario: Error message omits payload bytes

- **GIVEN** an exchange whose body holds the marker string `SECRET-8f31a` and a mismatched split expression
- **WHEN** the split fails
- **THEN** the error message contains the four diagnostic fields and does not contain `SECRET-8f31a`

### Requirement: Fragment body typing is element-driven

A split that derives fragments from JSON array elements SHALL type each
fragment body from the array element it derives from: string elements produce
`Body::Text`; number, boolean, object, nested-array, and null elements produce
`Body::Json`. Fragment typing SHALL NOT depend on how many nodes the split
expression matched. The rule holds for the declarative split and for the
programmatic `split_body_json_array` expression alike.

#### Scenario: N-match string nodeset yields raw-text fragments

- **GIVEN** a declarative split whose xpath expression matches 3 string nodes in the input XML
- **WHEN** the route runs and a fragment body is observed
- **THEN** the fragment body is `Body::Text` and `${body}` renders the raw string with no JSON quote characters

#### Scenario: Match-count parity

- **GIVEN** the same xpath split expression applied once to XML where it matches 1 node and once to XML where it matches 3 nodes
- **WHEN** fragments are produced
- **THEN** the per-fragment body type and content are identical for both counts (`Body::Text`, raw string)

#### Scenario: collect_all aggregation is indifferent to the typing change

- **GIVEN** a declarative split with `collect_all` aggregation over string elements
- **WHEN** the split scope closes
- **THEN** the aggregated JSON array is byte-identical to the pre-change output, because `Body::Text` and `Body::Json(String)` aggregate to the same `Value::String`

#### Scenario: jsonpath string-array splits yield text fragments

- **GIVEN** a declarative split whose jsonpath expression evaluates to `["a","b"]`
- **WHEN** fragments are produced
- **THEN** each fragment body is `Body::Text` with the raw string

#### Scenario: Non-string elements keep JSON typing

- **GIVEN** a declarative split whose expression evaluates to an array containing a number and an object
- **WHEN** fragments are produced
- **THEN** those fragment bodies are `Body::Json` with the element value unchanged

#### Scenario: Programmatic body_json_array keeps JSON typing

- **GIVEN** the programmatic `split_body_json_array` expression over a JSON array of strings
- **WHEN** the splitter runs
- **THEN** each fragment body is `Body::Text` with the raw string (name kept for
  archive stability; behavior updated by delta 2026-09-25-splitjson)

### Requirement: Split trace restart above item threshold

When a split produces more fragments than the configured trace item
threshold, the splitter SHALL start a new trace per fragment instead of
nesting fragment work under the live route span. Each per-item root span
SHALL carry exactly one OTEL span Link to the originating split segment
span, SHALL have no parent span id, and its sampled flag SHALL equal the
originating span's sampled flag. At or below the threshold, fragment work
SHALL stay nested in the originating trace exactly as before (trace-model
tree P0-b semantics). The threshold SHALL be configurable per split step;
`0` SHALL disable restart (legacy always-nested behavior); the default
SHALL be `100`.

#### Scenario: At or below the threshold fragments stay nested

- **GIVEN** a traced route with a split step configured with
  `trace_item_threshold: 2` and a body of 2 fragments
- **WHEN** the route runs and spans are exported
- **THEN** every fragment body span nests under the split segment span,
  all spans share the route's single trace id, and no span carries a link

#### Scenario: One item past the threshold each fragment starts a new trace

- **GIVEN** a traced route with a split step configured with
  `trace_item_threshold: 2` and a body of 3 fragments
- **WHEN** the route runs and spans are exported
- **THEN** each fragment's body runs under its own root span with a fresh
  trace id distinct from the route trace and from the other fragments, and
  the route trace contains no fragment body spans

#### Scenario: Per-item roots link back to the split segment span

- **GIVEN** a traced route whose split fan-out exceeds the threshold
- **WHEN** the per-item root spans are exported
- **THEN** each carries exactly one link whose span context equals the
  split segment span's span context, and no per-item root has a parent
  span id

#### Scenario: Sampling flag follows the origin

- **GIVEN** a sampler chain whose root honors links and a split fan-out
  above the threshold
- **WHEN** the originating route trace is sampled and a second identical
  run is not sampled
- **THEN** the per-item roots of the first run are sampled and the
  per-item roots of the second run are not sampled

#### Scenario: Zero disables trace restart

- **GIVEN** a traced route with a split step configured with
  `trace_item_threshold: 0` and a body of 500 fragments
- **WHEN** the route runs and spans are exported
- **THEN** all fragment body spans nest under the split segment span in
  one trace, byte-identical to the pre-change nested mode

#### Scenario: Default threshold is one hundred

- **GIVEN** a traced route with a split step that omits
  `trace_item_threshold` and a body of 100 fragments, then a body of 101
  fragments
- **WHEN** the route runs and spans are exported
- **THEN** the 100-fragment run stays nested in one trace and the
  101-fragment run restarts traces per item, because 100 keeps a nested
  trace (items × ~3–5 step spans each) inside common collector and trace
  UI per-trace span budgets while covering typical small batch fan-outs

#### Scenario: Compiled split segments stamp fragment metadata

- **GIVEN** a traced route whose split compiles to the outcome-pipeline
  split segment and a body of 3 fragments
- **WHEN** fragments are produced
- **THEN** each fragment carries `CamelSplitIndex`, `CamelSplitSize`
  (3), and `CamelSplitComplete` properties, matching the eager splitter
  service metadata contract

