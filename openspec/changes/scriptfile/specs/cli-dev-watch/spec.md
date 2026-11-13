## MODIFIED Requirements

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

## ADDED Requirements

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
