## MODIFIED Requirements

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
