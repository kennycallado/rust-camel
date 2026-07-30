# template-component Specification

## Purpose
TBD - created by archiving change external-template-component. Update Purpose after archive.
## Requirements
### Requirement: External template URI resolution

The system SHALL parse templates only from route-declared
`template:file:///abs/path` URIs at endpoint construction and SHALL reject
bare-path (`template:/abs/path`) and non-`file:` schemes at construction with
`CamelError::Config`. File acquisition SHALL occur during awaited startup, not
at construction.

#### Scenario: valid file URI parses at construction

- **GIVEN** a route declaring `template:file:///srv/templates/page.html`
- **WHEN** the endpoint is constructed
- **THEN** the URI parses and construction succeeds without filesystem access

#### Scenario: bare-path URI rejected

- **GIVEN** a route declaring `template:/srv/templates/page.html`
- **WHEN** the endpoint is constructed
- **THEN** construction fails with `CamelError::Config`

#### Scenario: non-file scheme rejected

- **GIVEN** a route declaring `template:http://host/page.html`
- **WHEN** the endpoint is constructed
- **THEN** construction fails with `CamelError::Config`

### Requirement: Zero-override resource policy

The system SHALL select the template resource exclusively from operator config
and SHALL NOT permit any Exchange body, header, or property to select or override
it.

#### Scenario: header cannot select template

- **GIVEN** a started route with a compiled template set
- **WHEN** a request carries a header attempting to name a different template
  file
- **THEN** the compiled set is rendered and the header has no effect on resource
  selection

### Requirement: Compile-before-activation

The system SHALL compile the full template set at startup and SHALL move the
route to `Failed` on any compile, path, or bound error before serving any
request. Successfully started lifecycle handles SHALL roll back in reverse order
when a later handle fails to start.

#### Scenario: startup compile failure fails the route

- **GIVEN** a route whose declared template file does not exist
- **WHEN** the route starts
- **THEN** the route enters `Failed` and serves no requests

#### Scenario: rollback on startup failure

- **GIVEN** a route with two template producers whose second producer's set fails
  to compile
- **WHEN** the route starts
- **THEN** the first producer's started handle is shut down and the route enters
  `Failed`

### Requirement: Compile-once hot path

The system SHALL reuse compiled template state across requests and SHALL perform
zero filesystem I/O on the render hot path.

#### Scenario: repeated requests reuse compiled state

- **GIVEN** a started route with a compiled set
- **WHEN** N requests are served
- **THEN** no additional filesystem reads occur for the template source

### Requirement: Template render semantics

The system SHALL render the root template against the current Exchange (body,
headers, properties as context), SHALL replace the Exchange body only after a
successful render, SHALL preserve headers and properties, and SHALL retain the
Stage 1 renderer guarantees: strict-undefined behavior, explicit autoescape, and
context/output/fuel/recursion/timeout bounding.

#### Scenario: body replaced only on successful render

- **GIVEN** a started route with a compiled template
- **WHEN** a request is processed and the template renders successfully
- **THEN** the response body is the rendered output and the original headers and
  properties are preserved

#### Scenario: render error leaves body unchanged

- **GIVEN** a started route with a compiled template
- **WHEN** rendering fails (e.g. fuel exhaustion)
- **THEN** the Exchange body is not replaced and the error surfaces as a
  `CamelError`

### Requirement: Bounded acquisition fail-closed

The system SHALL bound source bytes, context bytes, output bytes, fuel,
recursion depth, include count, include depth, single-template size, and reload
wall-clock, and SHALL fail closed on exhaustion without truncating and reporting
success.

#### Scenario: oversize source rejected

- **GIVEN** a template file larger than the configured source-size bound
- **WHEN** the set is acquired
- **THEN** acquisition fails and the route does not activate

#### Scenario: zero-bound config rejected at startup

- **GIVEN** a route configured with a zero value for a bounded dimension
- **WHEN** the route starts
- **THEN** startup rejects the configuration and the route enters `Failed`

### Requirement: Dependency-closure contract

The system SHALL resolve `include`, `extends`, `import`, and `from` targets
openat-relative to the entry template's parent directory (the configured root)
through a statically discovered closure. The system SHALL reject dynamic
(render-time-computed) template names, symlinks, `..` segments, absolute paths,
cycles, and duplicate file identities.

#### Scenario: include within root accepted

- **GIVEN** a template that includes a file under the configured root
- **WHEN** the closure is acquired
- **THEN** the include resolves relative to the root and is compiled

#### Scenario: include escape rejected

- **GIVEN** a template that includes a path escaping the configured root
- **WHEN** the closure is acquired
- **THEN** acquisition rejects the include

#### Scenario: dynamic template name rejected

- **GIVEN** a template whose include target is a render-time variable
- **WHEN** the closure is acquired
- **THEN** acquisition rejects the closure as not statically discoverable

#### Scenario: cycle rejected

- **GIVEN** template A includes template B and B includes A
- **WHEN** the closure is acquired
- **THEN** acquisition rejects the cycle

### Requirement: Atomic valid reload

The system SHALL swap a changed, valid template set atomically so in-flight
renders complete on the prior set and subsequent renders use the new set.

#### Scenario: valid change swaps

- **GIVEN** a started route
- **WHEN** a `ReloadTemplates` command completes with a valid set
- **THEN** subsequent renders use the new compiled set

### Requirement: Route-scoped reload atomicity

For a route with multiple template producers, the system SHALL stage every
replacement and commit all only after every build succeeds, SHALL serialize
concurrent reloads per route, SHALL reject stale generations, and SHALL bound the
whole operation by `reload_timeout_ms` so a timed-out build does not swap later.

#### Scenario: all-or-nothing commit

- **GIVEN** a route with two template producers where the second build fails
- **WHEN** a `ReloadTemplates` command is issued
- **THEN** neither producer swaps and the prior sets are retained

#### Scenario: delayed stale build does not swap

- **GIVEN** a started route where a newer reload already committed at generation
  G+1
- **WHEN** an older build tagged at generation G reaches commit
- **THEN** the older build is rejected as stale and does not swap

#### Scenario: timeout prevents late swap

- **GIVEN** a reload that exceeds `reload_timeout_ms`
- **WHEN** its build eventually completes after the timeout
- **THEN** the late-completing build does not swap

### Requirement: Invalid reload retention

The system SHALL retain the prior compiled set when a reload fails to compile or
acquire, so rendering continues uninterrupted on the last known-good set.

#### Scenario: invalid reload keeps prior set

- **GIVEN** a started route with compiled set S0
- **WHEN** a `ReloadTemplates` command targets a set that fails to compile
- **THEN** renders continue to use S0

### Requirement: Reload control-plane command

The system SHALL provide `RuntimeCommand::ReloadTemplates` as a non-lifecycle
infrastructure command that swaps the template set without persisting lifecycle
intent or mutating `RouteStatus`, mirroring the `ReloadTlsCerts` intercept.

#### Scenario: reload bypasses lifecycle journal and dedup

- **GIVEN** a started route
- **WHEN** `ReloadTemplates` is issued
- **THEN** the template set is reloaded and `RouteStatus` is unchanged, and the
  command is not recorded in the lifecycle journal

### Requirement: Async startup lifecycle hook

The system SHALL provide a generic async `StepLifecycle::start` hook on the
existing `StepLifecycle` trait, awaited by the route controller before pipeline
spawn, with a blanket no-op default that leaves existing routes unaffected.

#### Scenario: existing route unaffected

- **GIVEN** a route whose producer does not override `start`
- **WHEN** the route starts
- **THEN** startup proceeds identically to the pre-change behavior

#### Scenario: template producer compiles at start

- **GIVEN** a template producer that overrides `start`
- **WHEN** the route starts
- **THEN** the template set compiles during `start` before any request is served

