# Design: route-template-minijinja-skip

## Context

`substitute_strings_in_json` (materializer) walks the template JSON and
substitutes `{{name}}` in every string node via `substitute_placeholders`
(placeholder.rs). The placeholder grammar
`\{\{\s*([A-Za-z_][A-Za-z0-9_.\-]*)\s*\}\}` accepts `{{ headers.name }}`
and `{{ body }}` — exactly minijinja syntax. Two failure modes:

1. Minijinja-only variables (`headers.name`) hit
   `TemplateError::UnknownParameter` because no route-template parameter
   declares that name.
2. A declared parameter colliding with a minijinja variable (`body`) is
   silently substituted into the minijinja source.

The DSL has one minijinja shape: `LanguageExpressionDef { language, source }`
(re-exported from `camel-api::declarative`). The YAML lowering accepts
`language`+`source` or the shorthands `simple`/`rhai`/`jsonpath`/`xpath` —
there is no `minijinja` shorthand key, so the two-field object is the only
shape to detect. The model types live in `crates/camel-api/src/declarative.rs`;
consumers span script steps, predicates, split expressions, and value
sources, all of which embed the same object shape in template route JSON.

## Decision

**Skip at the walk level, keyed on the strict shape.** In
`substitute_json_value`, the Object branch checks for `language` == string
`"minijinja"` AND a string `source`. When present, the `source` value is
returned untouched (other keys of the same object still walk, so any
future sibling string keys keep substitution). Detection is exact-string
on `language`; `rhai`/`simple`/etc. keep full substitution and validation.

Byte-identity is trivially preserved: the skipped value is never passed to
`substitute_placeholders`.

**Escape hatch `\{{` in `substitute_placeholders`.** A segment scan around
the 3-byte sequence `\{{`: text before the escape substitutes normally,
the escape itself emits a literal `{{`, and scanning resumes after it.
Inputs without `\{{` take exactly one segment — byte-identical behavior,
including inputs or parameter values containing arbitrary control
characters (no sentinel, nothing restored). The rule is lexical: `\{{`
ALWAYS renders literal `{{` and blocks substitution for that brace pair;
`whole_node_placeholder` cannot match it (the leading backslash already
fails the grammar). A preceding extra backslash stays as-is
(`\\{{name}}` renders `\{{name}}`, name not substituted — there is no
backslash-escape layer). Unknown `{{name}}` outside minijinja still
errors — the escape is the only way to author a literal brace pair in a
normal string, so typos stay loud.

Unknown `{{name}}` outside minijinja still errors — the escape is the only
way to author a literal brace pair in a normal string, so typos stay loud.

## Alternatives considered

- Backtick/triple-brace delimiters for route templates: breaks every
  existing template; rejected.
- Escaping inside minijinja sources only: wrong layer; the collision bug
  is the placeholder engine consuming foreign syntax.
- Skipping all `language: *` expression objects: silently disables
  placeholders in rhai/simple sources, which today are legitimate
  substitution targets; rejected as a semantic change beyond the mission.

## Testing

Unit tests in `placeholder.rs` (escape grammar, collision, unknown still
errors) and `materializer.rs` (byte-identical skip, sibling keys still
substitute, non-minijinja languages unchanged, whole-node typed form
unaffected). Integration tests in
`crates/camel-dsl/tests/route_template_parity.rs` cover the full
discovery path: minijinja templated route compiles with the literal
source, `body` collision not substituted while the same parameter still
substitutes outside, and the escape hatch renders literally.
