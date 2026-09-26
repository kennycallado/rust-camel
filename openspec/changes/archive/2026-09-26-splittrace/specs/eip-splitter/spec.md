## ADDED Requirements

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
