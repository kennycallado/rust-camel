# cli-dev-watch Specification

## Purpose
TBD - created by archiving change 476-template-proto-dev-reload. Update Purpose after archive.
## Requirements
### Requirement: Dev-watch target derivation

In `camel run --watch` mode only, the watcher SHALL derive file watch targets
from committed per-route reload-target metadata: every exposed template
closure file, every validator protobuf descriptor source (schema file plus
resolved local imports), and every resolved script source file registered by
route discovery. Publication follows the runtime outcome of the reload pass,
not discovery success: successfully added, restarted, or swapped routes
publish their staged script dependencies, failed or skipped routes retain
their previous script dependencies, and routes removed by a committed reload
have their script dependencies deleted. The watcher SHALL NOT watch arbitrary files or
directories, SHALL NOT create targets for gRPC proto references (a documented
non-goal), and SHALL re-derive targets after every route reload pass and
after every committed template/proto reload. A partially failed route reload
SHALL retain the surviving routes' targets.

#### Scenario: targets limited to declared sources

- **GIVEN** a watched route set with one registered template target whose
  closure has three files and one validator target with a schema and one
  local import
- **WHEN** watch targets are derived
- **THEN** exactly the three closure files, the validator schema, and its
  local import form the tracked dispatch set — changes to any other path
  never dispatch a reload

#### Scenario: gRPC proto reference creates no target

- **GIVEN** a gRPC endpoint whose `protoFile` names a local `.proto` file
- **WHEN** watch targets are derived
- **THEN** no watch target is created for that reference

#### Scenario: re-derivation after route reload

- **GIVEN** a watcher with derived targets
- **WHEN** a route hot-reload commits a route set whose registered closure
  now includes a new file
- **THEN** the new file is watched after the reload pass

#### Scenario: failed route reload keeps surviving targets

- **GIVEN** two watched routes, one whose new definition fails to compile
- **WHEN** a route reload pass rebuilds only the valid route and the invalid
  one keeps running its previous generation
- **THEN** the failed route's previously committed targets stay watched

#### Scenario: watched file replaced by atomic save stays watched

- **GIVEN** a watched template file
- **WHEN** an editor replaces the file via delete-and-recreate or
  write-temp-rename, and the file is then edited again
- **THEN** the replacement does not disable observation and the second edit
  still triggers a reload dispatch

#### Scenario: watched file with a route-file extension stays a dev-target event

- **GIVEN** a tracked closure file named with a `.yaml` or `.json` extension
  inside a watched route directory
- **WHEN** that file changes
- **THEN** the event is handled as a dev-target event, not a route-file
  reload

#### Scenario: resolved script files join the tracked set

- **GIVEN** a watched route whose document references
  `source_file: "scripts/enrich.rhai"`
- **WHEN** watch targets are derived after route discovery
- **THEN** the resolved script path is in the tracked dispatch set and an
  edit to it dispatches a reload, while changes to unrelated files dispatch
  nothing

#### Scenario: failed route compile keeps the previous script target

- **GIVEN** a watched route referencing `scripts/enrich.rhai`, and a second
  watch pass whose discovery succeeds but whose route rebuild fails
- **WHEN** the pass commits no swap for the failed route
- **THEN** the failed route keeps its previously published script
  dependency in the tracked set, while successfully swapped routes publish
  their new script dependencies

#### Scenario: changed script path publishes the new target

- **GIVEN** a watched route whose committed generation references
  `source_file: "scripts/old.rhai"`
- **WHEN** the route document is edited to reference
  `source_file: "scripts/new.rhai"` and the reload pass successfully
  restarts the route
- **THEN** `scripts/new.rhai` is in the route's tracked script set and
  `scripts/old.rhai` is no longer tracked for that route

### Requirement: Template and proto change handling

On a template or proto file change, after the configured debounce, the
watcher SHALL dispatch `RuntimeCommand::ReloadTemplates { route_id }` through
the runtime execution handle for each affected route that has at least one
registered reload target. The dispatch SHALL NOT restart the route, change
`RouteStatus`, append journal events, or run a full route reload. When route
files and template/proto files change in the same debounce batch, the
existing route-reload behavior SHALL run first, and template/proto reloads
SHALL still dispatch for affected routes whose generation survived the pass
without a rebuild.

#### Scenario: template edit changes output without restart

- **GIVEN** a watched route rendering a committed template
- **WHEN** the template file is edited and the debounce window closes
- **THEN** `ReloadTemplates` is dispatched for that route and the next render
  uses the new content
- **THEN** the route keeps running without a restart and `RouteStatus` is
  unchanged

#### Scenario: nested include edit triggers reload

- **GIVEN** a watched route whose template entry includes a child template
- **WHEN** only the child include file is edited
- **THEN** `ReloadTemplates` is dispatched for that route and the next render
  reflects the edited include

#### Scenario: validator proto edit is observed and recompiled

- **GIVEN** a watched route whose validator endpoint compiled a `.proto`
  descriptor
- **WHEN** the `.proto` file is edited and the debounce window closes
- **THEN** the affected route's reload target recompiles the descriptor and
  the next validation uses the recompiled schema

#### Scenario: local import edit triggers reload

- **GIVEN** a watched validator endpoint whose schema imports another local
  `.proto` file
- **WHEN** only the imported file is edited
- **THEN** `ReloadTemplates` is dispatched for the affected route and the
  recompiled descriptor reflects the import change

#### Scenario: debounce coalesces rapid edits

- **GIVEN** a watched template file
- **WHEN** the file is written several times within one debounce window
- **THEN** a single reload dispatch runs for the affected route

#### Scenario: mixed batch still dispatches for surviving unrebuilt routes

- **GIVEN** a debounce batch containing a route-file event for route B and a
  template-file event for route A, where the route pass rebuilds route B but
  not route A
- **WHEN** the debounce window closes and the route reload pass completes
  (including when discovery fails or the pass computes no actions)
- **THEN** route A still receives its `ReloadTemplates` dispatch, and route B
  does not receive a separate one

#### Scenario: route without reload targets is skipped

- **GIVEN** a changed file whose referencing route has no registered reload
  target
- **WHEN** the debounce window closes
- **THEN** no `ReloadTemplates` command is dispatched for that route and the
  watcher continues

### Requirement: Non-watch isolation

File watching for templates and proto descriptors SHALL exist only under
`camel run --watch`. Non-watch runs and compiled artifacts SHALL never watch
any file.

#### Scenario: non-watch mode never watches

- **GIVEN** a route set with template and proto references run without
  `--watch`
- **WHEN** a template or `.proto` file changes
- **THEN** no watcher exists and no reload is dispatched

#### Scenario: compiled artifacts never watch

- **GIVEN** a self-contained compiled artifact executed from `camel run
  <artifact>`
- **WHEN** it runs
- **THEN** no file watcher is created

### Requirement: Watcher failure containment

A failed or timed-out template/proto reload SHALL keep the previously
committed compiled set, log a redacted error, and leave the route running.
Reload dispatches SHALL be bounded by the configured reload timeout, SHALL be
cancellable by shutdown without a later commit, and SHALL NOT block the
watcher loop or its shutdown responsiveness.

#### Scenario: failed template reload keeps prior output

- **GIVEN** a watched route with a committed template set
- **WHEN** the template is edited to invalid syntax and the reload runs
- **THEN** the reload fails, the redacted error is logged, the prior
  template set keeps rendering, the route stays running, and the watcher
  continues processing later events

#### Scenario: timed-out dispatch commits nothing

- **GIVEN** a reload dispatch in flight that exceeds the reload timeout
- **WHEN** the timeout fires
- **THEN** the watcher abandons the dispatch, logs, and continues its loop,
  and the abandoned reload never commits a staged set afterwards

#### Scenario: shutdown stays responsive during a dispatch

- **GIVEN** a reload dispatch in flight
- **WHEN** the shutdown signal arrives before the dispatch completes
- **THEN** the watcher observes the shutdown promptly and no commit happens
  for the abandoned dispatch

#### Scenario: broken proto blocks same-route template reload atomically

- **GIVEN** a route with both a validator target and a template target, and
  a batch editing the validator's `.proto` to invalid syntax together with
  a valid template edit
- **WHEN** the reload dispatch runs
- **THEN** the reload fails all-or-nothing, nothing commits, and BOTH the
  prior descriptor and the prior template set keep serving

### Requirement: Script source change handling

On a script source file change, after the configured debounce, the watcher
SHALL run the full route reload pass (re-discovery plus source-hash diff) for
the watched route set. It SHALL NOT dispatch `ReloadTemplates` for a script
change. When script files and route files change in the same debounce batch,
exactly one reload pass SHALL run. A script edit that reproduces identical
content SHALL keep the route running without a swap.

#### Scenario: script edit swaps the route

- **GIVEN** a watched route whose document references
  `source_file: "scripts/enrich.rhai"`
- **WHEN** the script file is edited and the debounce window closes
- **THEN** the route reload pass runs, the route's new source hash differs
  from the committed hash, and the route swaps to the generation built from
  the new script contents

#### Scenario: script and route file edits in one batch run a single pass

- **GIVEN** a watched route document and its referenced script file
- **WHEN** both files are edited within one debounce window
- **THEN** exactly one reload pass runs for the batch

#### Scenario: script restored to identical content does not swap

- **GIVEN** a watched route referencing a script file
- **WHEN** the script file is edited and restored to its previous content
  before one debounce window closes, without an intervening reload commit
- **THEN** the reload pass skips the route because the folded source hash,
  compared against the currently committed generation, is unchanged

