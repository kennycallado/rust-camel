# template-component Specification

## Purpose
The template component. Templates resolve only from route-declared `file:`
URIs, compile before activation, render from compiled state without filesystem
I/O, bound their acquisition fail-closed, and reload atomically under a
control-plane command.
## Requirements
### Requirement: External template URI resolution

The system SHALL parse templates from route-declared `template:file:///abs/path`
URIs and, when a component `root` is configured, from route-declared relative
`template:file:<rel-path>` URIs, at endpoint construction. The system SHALL
reject bare-path (`template:/abs/path`) and non-`file:` schemes at
construction with `CamelError::Config`. A relative URI without a configured
component `root` SHALL fail construction with `CamelError::Config`. The
relative form SHALL contain no empty path component, no `.` component, and
no `..` component. File acquisition SHALL occur during awaited startup, not
at construction.

#### Scenario: valid file URI parses at construction

- **GIVEN** a route declaring `template:file:///srv/templates/page.html`
- **WHEN** the endpoint is constructed
- **THEN** the URI parses and construction succeeds without filesystem access

#### Scenario: valid relative URI parses at construction when root configured

- **GIVEN** `[components.template] root` configured and a route declaring
  `template:file:templates/page.html`
- **WHEN** the endpoint is constructed
- **THEN** the URI parses and construction succeeds without filesystem access

#### Scenario: relative URI without configured root rejected

- **GIVEN** no `[components.template]` `root` configured and a route
  declaring `template:file:templates/page.html`
- **WHEN** the endpoint is constructed
- **THEN** construction fails with `CamelError::Config`

#### Scenario: bare-path URI rejected

- **GIVEN** a route declaring `template:/srv/templates/page.html`
- **WHEN** the endpoint is constructed
- **THEN** construction fails with `CamelError::Config`

#### Scenario: non-file scheme rejected

- **GIVEN** a route declaring `template:http://host/page.html`
- **WHEN** the endpoint is constructed
- **THEN** construction fails with `CamelError::Config`

#### Scenario: dot component in relative URI rejected

- **GIVEN** `[components.template] root` configured and a route declaring
  `template:file:./templates/page.html`
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
Stage 1 renderer guarantees: strict-undefined behavior, bounded
context/output/fuel/recursion/timeout, and an explicit escape-mode contract.
For the external `template:` component, the entry template's escape mode SHALL
be selected in precedence order: first, a valid explicit top-level
`{% autoescape %}` block in the entry source; second, the operator-configured
`[components.template]` `default-autoescape` value (`html`, `json`, or
`none`), which SHALL apply only when the entry contains no autoescape
statement at all; otherwise the compile SHALL reject. An entry containing an
autoescape statement that fails top-level validation SHALL reject even when a
default is configured: the configured default SHALL NOT repair an invalid
mode, a partial block, a duplicate block, or malformed syntax. The configured
default SHALL apply to the entry template only; include targets SHALL keep
their own semantics (the environment-wide auto-escape callback is never
changed by the config). The inline `language: minijinja` engine SHALL keep
the mandatory explicit block: no configured default applies to it. An invalid
`default-autoescape` value (unknown or wrongly-typed) SHALL fail config load
with an error naming the key and the allowed values, and SHALL never fall
back to an implicit HTML default.

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

#### Scenario: configured html default escapes the payload

- **GIVEN** `[components.template]` `default-autoescape = "html"` and an entry
  template with no autoescape statement
- **WHEN** the payload `<b>` renders through a `{{ … }}` expression in the entry
- **THEN** the output contains `&lt;b&gt;` and not the raw payload

#### Scenario: configured none default keeps the payload verbatim

- **GIVEN** `[components.template]` `default-autoescape = "none"` and an entry
  template with no autoescape statement
- **WHEN** the payload `<b>` renders through a `{{ … }}` expression in the entry
- **THEN** the output is the raw payload `<b>`

#### Scenario: explicit block overrides the configured default

- **GIVEN** `[components.template]` `default-autoescape = "html"` and an entry
  whose entire source is a top-level `{% autoescape "json" %}{{ payload }}{%
  endautoescape %}` block
- **WHEN** the payload `<b>` renders
- **THEN** the output is the JSON string literal `"<b>"` (quoted) and no HTML
  entity escaping is applied (`&lt;` is absent)

#### Scenario: missing block without configured default rejects

- **GIVEN** no `default-autoescape` key and an entry template with no
  autoescape statement
- **WHEN** the route starts or reloads
- **THEN** the compile fails closed with a compile error

#### Scenario: invalid explicit statement still rejects with a default configured

- **GIVEN** `[components.template]` `default-autoescape = "html"` and an entry
  containing `{% autoescape "htm" %}{{ x }}{% endautoescape %}`
- **WHEN** the route starts
- **THEN** the compile fails: the configured default does not repair the
  invalid explicit mode

#### Scenario: include target keeps its own semantics

- **GIVEN** `[components.template]` `default-autoescape = "none"`, an entry
  with no autoescape statement that includes `partial.txt`, and `partial.txt`
  wrapping its own output in `{% autoescape "html" %}`
- **WHEN** the payload `<b>` renders through the include target's expression
- **THEN** the output is HTML-escaped by the include target's own block, not by
  the configured default

#### Scenario: invalid configured value fails config load

- **GIVEN** `[components.template]` `default-autoescape = "yaml"`
- **WHEN** the component config loads
- **THEN** the load fails with an error naming `default-autoescape` and the
  allowed values `html`, `json`, `none`

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
openat-relative to the configured canonical root when a component `root` is
set, and openat-relative to the entry template's parent directory otherwise,
through a statically discovered closure. The system SHALL reject dynamic
(render-time-computed) template names, symlinks, `..` segments, absolute paths,
cycles, and duplicate file identities.

#### Scenario: include within root accepted

- **GIVEN** a configured root and a template that includes a file under the
  same canonical root
- **WHEN** the closure is acquired
- **THEN** the include resolves relative to the canonical root and is
  compiled

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

#### Scenario: existing anchoring preserved without a configured root

- **GIVEN** no component `root` configured and an absolute entry URI
- **WHEN** the closure is acquired
- **THEN** the closure resolves relative to the entry's parent directory
  with all rejection behaviors (dynamic names, symlinks, `..`, absolute
  paths, cycles, duplicate identities) unchanged

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

### Requirement: minijinja engine optionality

camel-template SHALL make `camel-language-minijinja` and `minijinja` optional
dependencies enabled by `dep:` entries in the `default` feature list, with no
other feature keys, so consumers using default features keep today's exact
behavior and closure, while `default-features = false` consumers resolve the
crate without either engine crate in their dependency closure.

#### Scenario: default closure unchanged

- **GIVEN** any consumer depending on camel-template with default features
- **WHEN** the dependency closure is resolved before and after the change
- **THEN** camel-language-minijinja and minijinja are present in both, and
  the camel-cli golden deptree fixture passes without regeneration

#### Scenario: engine-absent build compiles

- **GIVEN** camel-template built with `--no-default-features`
- **WHEN** the crate is compiled
- **THEN** neither camel-language-minijinja nor minijinja appears in the
  resolved dependencies, compilation succeeds, and the exported surface is
  exactly the engine-free set (config and error modules)

#### Scenario: engine-on behavior identical

- **GIVEN** camel-template compiled with default features
- **WHEN** its existing unit and integration tests run
- **THEN** results are identical to the pre-change baseline (no behavioral
  change behind the default set)

### Requirement: engine module boundary

camel-template SHALL cfg-gate on `feature = "default"` exactly the
engine-coupled modules — bundle, closure, component, endpoint, lifecycle,
producer, reload, template_set — plus their lib.rs re-exports
(`TemplateBundle`, `TemplateBundleConfig`, `TemplateComponent`). The public
config and error modules and the crate-private path_util and uri modules
SHALL remain engine-free and ungated.

#### Scenario: engine-free modules stay ungated

- **GIVEN** camel-template compiled with `--no-default-features`
- **WHEN** the crate's public engine-free items are used (limits-config
  types, `TemplateReloadError`)
- **THEN** they resolve without the default feature enabled, and the
  engine-free modules contain no reference to minijinja or
  camel-language-minijinja

#### Scenario: no named re-enable surface this change

- **GIVEN** camel-template's `[features]` table contains only `default`
- **WHEN** a consumer activates any non-default feature combination
- **THEN** no partial engine surface is exposed (the crate is all-or-nothing
  this change; the named re-enable feature and its cfg-key rename are
  deferred to the camel-cli mission that owns the consumer manifests and the
  golden-fixture regeneration)

### Requirement: Template root configuration and confinement

The system SHALL accept an optional `[components.template]` `root` path
(absolute or relative to the process CWD). When `root` is configured, the
system SHALL canonicalize both the configured root and the entry candidate
at startup, SHALL fail with a clear error on a missing root directory, a
missing entry file, or a canonical entry outside the canonical root, and
SHALL anchor the dependency closure at the canonical root. Entry opening
SHALL use the entry's original root-relative path components under the
same no-symlink confinement as closure targets; canonicalization SHALL NOT
replace confined opening. When `root` is not configured, the system SHALL
keep the existing entry-parent anchoring for absolute URIs unchanged.

#### Scenario: relative entry under root loads and renders

- **GIVEN** `[components.template] root = "<tempdir>"` and a route declaring
  `template:file:templates/page.html` with `page.html` present under
  `<tempdir>/templates/`
- **WHEN** the route starts and a request is processed
- **THEN** the nested `<tempdir>/templates/page.html` renders — not a
  different `<tempdir>/page.html` namesake

#### Scenario: dot-dot segment in relative URI rejected

- **GIVEN** `[components.template] root` configured and a route declaring
  `template:file:../escape.html`
- **WHEN** the endpoint is constructed
- **THEN** construction fails with `CamelError::Config`

#### Scenario: leaf symlink entry inside the root rejected

- **GIVEN** `[components.template] root` configured and a symlink
  `link.html → page.html` inside the root with a route declaring the
  `link.html` entry
- **WHEN** the route starts
- **THEN** startup fails with a path-escape error and the route does not
  serve requests

#### Scenario: intermediate symlink inside the root rejected

- **GIVEN** `[components.template] root` configured, a directory symlink
  `partials → <dir-inside-root>` inside the root, and a route declaring the
  `partials/page.html` entry
- **WHEN** the route starts
- **THEN** startup fails with a path-escape error and the route does not
  serve requests

#### Scenario: symlink escaping the root rejected

- **GIVEN** `[components.template] root` configured, an entry inside the
  root, and a closure target that is a symlink pointing outside the
  canonical root to an existing file
- **WHEN** the route starts
- **THEN** startup fails with a path-escape error and the route does not
  serve requests

#### Scenario: absolute entry outside configured root rejected

- **GIVEN** `[components.template] root = "<tempdir-a>"` and a route
  declaring `template:file:///<tempdir-b>/page.html` where `<tempdir-b>` is
  outside the canonical root and `page.html` exists there
- **WHEN** the route starts
- **THEN** startup fails with a clear escape error naming the entry and the
  canonical root

#### Scenario: missing root directory fails with clear error

- **GIVEN** `[components.template] root = "<path-that-does-not-exist>"`
- **WHEN** the route starts
- **THEN** startup fails with an error naming the configured root

#### Scenario: include outside root rejected

- **GIVEN** `[components.template] root` configured, the entry inside the
  root including `../outside.html`, and `outside.html` existing outside the
  root
- **WHEN** the route starts
- **THEN** the closure acquisition fails and the route does not serve
  requests

### Requirement: Allowlisted render filters

The external template component's MiniJinja environment SHALL register exactly
two new component-owned filters — `truncate` and `dump` — on top of the filters
the MiniJinja engine itself provides, and SHALL register no component-owned
functions and no component-owned globals. `truncate(length, end="…")` SHALL
truncate the input string to `length` Unicode scalar values without splitting
a scalar value, SHALL accept only a non-negative integer `length` (negative,
non-integral, boolean, and non-numeric values SHALL fail rendering), SHALL
append `end` only when truncation occurs, and SHALL have no HTML awareness.
`dump` SHALL produce a deterministic JSON serialization of the value (equal
inputs serialize byte-identically) and SHALL fail rendering for values that
cannot be serialized to JSON. Dump output SHALL be ordinary template text and
SHALL remain subject to the active escape mode; it SHALL NOT carry safe-string
marking. No allowlisted filter SHALL execute code or access the host. Filter
names other than `truncate`, `dump`, and the MiniJinja engine's own built-in
filters SHALL stay unregistered and SHALL fail at render time.

#### Scenario: truncate shorter than limit is a no-op

- **GIVEN** an entry template rendering `{{ name | truncate(10) }}` under
  `{% autoescape "none" %}`
- **WHEN** the context value `name` is `"eleventy"` (8 scalar values)
- **THEN** the output is `"eleventy"` with no end marker appended

#### Scenario: truncate multi-byte content stays scalar-aligned

- **GIVEN** an entry template rendering `{{ name | truncate(5) }}` under
  `{% autoescape "none" %}`
- **WHEN** the context value `name` is `"cafés niños"` (contains multi-byte
  `é` and `í`)
- **THEN** the output is the first 5 Unicode scalar values `"cafés"` followed
  by the end marker `"…"`, with no partial byte sequences

#### Scenario: custom end appended only on truncation

- **GIVEN** an entry template rendering `{{ name | truncate(4, "[more]") }}`
  and `{{ short | truncate(4, "[more]") }}` under `{% autoescape "none" %}`
- **WHEN** `name` is `"abcdefgh"` and `short` is `"abc"`
- **THEN** the first output is `"abcd[more]"` and the second is `"abc"`

#### Scenario: negative length fails as a render error

- **GIVEN** an entry template rendering `{{ name | truncate(-2) }}` where
  `name` is the defined context value `"abcdefgh"`
- **WHEN** the render runs
- **THEN** rendering fails with `TemplateReloadError::Render` (the existing
  data-plane render error path)

#### Scenario: non-integer and boolean lengths fail as render errors

- **GIVEN** entry templates rendering `{{ name | truncate(2.5) }}` and
  `{{ name | truncate(true) }}` where `name` is the defined context value
  `"abcdefgh"`
- **WHEN** each render runs
- **THEN** each rendering fails with `TemplateReloadError::Render`

#### Scenario: dump serializes object, array, and scalar deterministically

- **GIVEN** an entry template under `{% autoescape "none" %}` rendering
  `{{ obj | dump }}`, `{{ items | dump }}`, and `{{ title | dump }}`, where
  the context values are the object `{"name": "Ada", "year": 1843}`, the
  array `[1, 2, 3]`, and the string scalar `"Ada"`
- **WHEN** each render runs twice against equal inputs
- **THEN** the object output is exactly `{"name":"Ada","year":1843}` (keys
  sorted), the array output is exactly `[1,2,3]`, the scalar output is the
  JSON-quoted string `"Ada"`, and every output is byte-identical across
  repeated renders

#### Scenario: dump fails for an unserializable value

- **GIVEN** an entry template rendering `{{ broken | dump }}` where `broken`
  resolves to a map value containing a sequence-valued key (a value JSON
  cannot represent; the test constructs this value)
- **WHEN** the render runs
- **THEN** rendering fails with `TemplateReloadError::Render` because the
  value cannot be serialized to JSON

#### Scenario: only the component allowlist resolves

- **GIVEN** an entry template rendering `{{ name | truncate(10) }}` and a
  sibling template rendering `{{ name | numerify }}`
- **WHEN** both renders run
- **THEN** `truncate` resolves and renders, while `numerify` (a name the
  component never registers) fails at render time with an unknown-filter
  error

### Requirement: HTML escaping policy for the external component

The external template component SHALL keep MiniJinja's built-in HTML escaper —
including its escaping of `/` as `&#x2f;` — and SHALL NOT register a custom
escaper that weakens or disables it. The component docs SHALL document the
`&#x2f;` behavior and the safe migration options: `|safe` only for trusted
content, or pre-rendered values.

#### Scenario: slash escapes under the html escape mode

- **GIVEN** an entry template rendering `{{ path }}` under an html escape mode
  (an explicit `{% autoescape "html" %}` block or the configured
  `default-autoescape = "html"`)
- **WHEN** the context value `path` is `"a/b"`
- **THEN** the output contains `&#x2f;` wherever the expression emitted `/`,
  and the raw `/` never reaches the output from an escaped expression

### Requirement: Dev-watch source exposure

A template reload target registered in the `TemplateReloadRegistry` SHALL
expose, as unique absolute file paths, every file in its last committed
dependency closure — the entry and all includes, nested includes included.
The confinement root directory SHALL NOT be reported as a watch source.

#### Scenario: closure with nested include reports all files

- **GIVEN** a route with a `template:file:` endpoint whose entry includes a
  child template, which itself includes a grandchild
- **WHEN** the reload target's watch sources are enumerated
- **THEN** exactly three absolute file paths are reported — the entry, the
  child, and the grandchild — with no duplicates and no directory path

#### Scenario: sources refresh after a reload that grows the closure

- **GIVEN** a committed template set whose closure contains two files
- **WHEN** a reload commits a set whose closure adds a new include
- **THEN** subsequent enumeration reports the new file without re-registering
  the target

#### Scenario: failed reload keeps reporting the committed closure

- **GIVEN** a committed template set
- **WHEN** a reload build fails and no commit happens
- **THEN** enumeration still reports the previously committed closure only

