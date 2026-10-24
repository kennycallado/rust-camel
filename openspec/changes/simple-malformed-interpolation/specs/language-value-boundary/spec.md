## MODIFIED Requirements

### Requirement: Simple template expressions preserve literal text

`SimpleLanguage::create_expression` SHALL parse its source in TEMPLATE mode.
In template mode the source is literal text plus `${...}` interpolation slots
only. Characters and words outside a slot — including `=`, `<`, `>`, `!`,
`&&`, `||`, `contains`, `null`, `true`, `false`, single and double quotes, and
whitespace — SHALL be preserved verbatim and SHALL NOT be re-parsed as
predicate grammar. The whole source SHALL be outer-trimmed first so a YAML
block scalar's trailing newline is absorbed; whitespace inside the source SHALL
be preserved. An empty (after trim) source SHALL be a parse error. The
unclosed-`${` rule takes precedence over the raw-literal rule: a source
containing an unclosed `${` is governed by the unclosed rule, not classified
as a raw literal. An unclosed `${` SHALL always be a parse error: creation
SHALL fail with a message naming the construct (`unclosed interpolation`) and
the byte offset of the `${` in the trimmed source. An empty (or
whitespace-only) slot (`${}`) SHALL be a parse error naming the empty
interpolation. A `${` inside slot content (nested interpolation) SHALL be a
parse error. Literal `$`, `{`, and `}` outside a `${` remain verbatim text.
"Simple
template mode" is Simple's own literal-plus-`${}` grammar; it is NOT the
MiniJinja template engine (ADR-0047) and NOT RouteTemplate JSON-tree
substitution (ADR-0008).

#### Scenario: Operator-looking text stays literal

- **GIVEN** an expression created from `Got -> ${header.h}` with header `h`
  equal to `B`
- **WHEN** the expression is evaluated
- **THEN** the result SHALL be the string `Got -> B`; no numeric comparison
  SHALL be attempted

#### Scenario: Keyword-looking text stays literal

- **GIVEN** an expression created from `value is null: ${header.h}` with
  header `h` equal to `B`
- **WHEN** the expression is evaluated
- **THEN** the result SHALL be the string `value is null: B`, preserving the
  word `null`

#### Scenario: Whitespace and quoting in literal text are preserved

- **GIVEN** an expression created from `${header.h} 3 items` with header `h`
  equal to `B`, and an expression created from `literal 'quoted' text`
- **WHEN** each expression is evaluated
- **THEN** the results SHALL be `B 3 items` and `literal 'quoted' text`
  respectively

#### Scenario: Angle-bracket literals do not fail

- **GIVEN** an expression created from `<order>${header.h}</order>` with header
  `h` equal to `B`
- **WHEN** the expression is evaluated
- **THEN** the result SHALL be `<order>B</order>` and no parse error SHALL be
  raised

#### Scenario: Outer trim absorbs a block-final newline

- **GIVEN** an expression source `"Got ${header.h}\n"` with header `h` equal to
  `B`
- **WHEN** the expression is evaluated
- **THEN** the result SHALL be `Got B`, with no trailing newline, and the
  internal space of `Got ` SHALL be preserved

#### Scenario: Empty template is a parse error

- **GIVEN** an expression source `""` or `"   "`
- **WHEN** the expression is created
- **THEN** creation SHALL fail with a parse error

#### Scenario: Unclosed slot is always a parse error

- **GIVEN** expression sources `Got ${body`, `${header.h} ${`, and `${`
- **WHEN** each expression is created
- **THEN** creation SHALL fail with a parse error whose message names
  `unclosed interpolation` and the byte offset of the offending `${` in the
  trimmed source (4, 12, and 0 respectively); an unclosed `${` SHALL NOT be
  treated as literal text in any position

#### Scenario: Empty slot is a parse error

- **GIVEN** expression sources `${}` and `${   }`
- **WHEN** each expression is created
- **THEN** creation SHALL fail with a parse error whose message names the
  empty interpolation

#### Scenario: Nested interpolation is a parse error

- **GIVEN** expression sources `${${header.a}}` and `${header.a${b}`
- **WHEN** each expression is created
- **THEN** creation SHALL fail with a parse error

## ADDED Requirements

### Requirement: Simple predicates reject malformed interpolation

`SimpleLanguage::create_predicate` SHALL apply the same unclosed-`${` rule as
template mode: a `${` with no closing `}` in predicate source SHALL be a
compile-time parse error naming `unclosed interpolation` and the byte offset
of the `${` in the trimmed source — never literal text. Single- and
double-quoted strings in predicate source SHALL continue to protect `${` from
slot parsing. Literal `$`, `{`, and `}` outside a `${` SHALL remain literal.

#### Scenario: Unclosed slot after literal text is a parse error

- **GIVEN** a predicate source `Got ${name`
- **WHEN** the predicate is created
- **THEN** creation SHALL fail with a parse error whose message names
  `unclosed interpolation` and `byte offset 4`

#### Scenario: Quoted strings still protect a dollar-brace

- **GIVEN** a predicate source `'Got ${name'`
- **WHEN** the predicate is created
- **THEN** creation SHALL succeed and the parsed string SHALL include
  `${name` verbatim

#### Scenario: Literal dollar and braces stay literal

- **GIVEN** a predicate source `Got $100 from ACME`
- **WHEN** the predicate is created
- **THEN** creation SHALL succeed
