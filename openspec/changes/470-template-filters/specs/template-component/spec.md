## ADDED Requirements

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
