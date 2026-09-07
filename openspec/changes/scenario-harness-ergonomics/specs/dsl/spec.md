## MODIFIED Requirements

### Requirement: Env-lookup-injected route discovery

Route discovery SHALL resolve `${env:}` placeholders through the injected environment lookup, applying interpolation to parsed YAML string keys and string leaves only, and only where the scalar text contains a placeholder or escape token. Interpolated leaves SHALL resolve as string scalars (numeric-looking results keep string typing — the camel-config leaf-interpolation precedent). For documents the tree walk processes, comment content SHALL never be interpolated and SHALL never fail resolution. The escape grammar (`$${env:X}`, `$$`) SHALL keep raw-splice semantics inside each string. If the raw text cannot be parsed as YAML by the same parser discovery hands off to, or the parsed tree contains tagged nodes, discovery SHALL fall back to whole-text raw interpolation with legacy semantics (including legacy comment sensitivity for those documents).

#### Scenario: numeric-looking interpolation result stays a string

- **GIVEN** a route value `port: ${env:PORT}` with `PORT` resolving to `8080`
- **WHEN** discovery interpolates the document
- **THEN** the leaf parses back as the string `"8080"`, not a number

#### Scenario: placeholders in comments do not fail resolution

- **GIVEN** a route file containing `${env:MISSING}` inside a YAML comment and no default
- **WHEN** discovery interpolates with a lookup that does not define `MISSING`
- **THEN** the document loads and the comment content is ignored

#### Scenario: quoted hash survives interpolation

- **GIVEN** a route value containing a quoted `#` character
- **WHEN** discovery interpolates the document
- **THEN** the value keeps its `#` (tree-walk interpolation, not comment stripping)

#### Scenario: block scalar content interpolates as a value

- **GIVEN** a literal block scalar whose content contains `${env:X}`
- **WHEN** discovery interpolates with `X` defined
- **THEN** the placeholder resolves inside the block content, matching raw-splice behavior
