# Value boundaries across languages

Exchange data is untrusted under ADR-0032. Each expression language converts Exchange values into its own runtime values, evaluates the script, and converts the result back. These conversions form the *value boundary*. A missing value can become one of three things at that boundary: a null, an empty string, or a hard error. The choice is per language and is not interchangeable.

This page is the authoritative cross-language reference for missing-value and null-versus-empty semantics. It covers [Simple](simple.md), [Rhai](rhai.md), and [MiniJinja](minijinja.md). Each row is pinned by tests in the three language crates.

## Missing-value matrix

The same absent header (`missing`, never set on the Exchange) behaves as follows.

| Concern | Simple | Rhai | MiniJinja |
|---|---|---|---|
| Interpolation or read | Whole `${header.missing}` slot returns `Value::Null`. In a mixed template it renders as an empty string: `prefix-${header.missing}-suffix` becomes `prefix--suffix`. | `header("missing")` and `headers["missing"]` return Rhai unit `()`, which lands on the camel boundary as `Value::Null`. | `{{ headers.missing }}` fails the render. Strict undefined has no empty-string fallback. |
| Conditional use | A bare `${header.missing}` predicate is `TypeMismatch` expecting `bool`. There is no truthiness at the predicate boundary. Inside `&&` and `||`, null and empty string are false. | A bare `header("missing")` predicate is `TypeMismatch` expecting `bool`, actual `null`. An `if` on a missing value is `EvalFailure` with class `TypeMismatch`. It does not take the else branch. | `{% if headers.missing %}` also fails the render. Guard with `is defined` instead. |
| Null versus empty | `Value::Null` is distinct from `""`. `${header.missing} == null` matches true. | An absent key and a stored JSON `null` both read as unit. Use `contains(...)` or `"k" in headers` to test presence. | `headers.missing is defined` is false when the key is absent. The `| default('')` filter supplies an explicit fallback. |
| Error behavior | `LanguageError::TypeMismatch` at the predicate boundary. Interpolation never errors for a missing slot. | `LanguageError::TypeMismatch` or `LanguageError::EvalFailure` with class `TypeMismatch`. Failure detail is redacted. | Render failure with a redacted message that names the undefined path or kind. |

## Simple

Simple has two parsing modes, and the missing-value result depends on the mode. In template mode, a source that is exactly one `${...}` slot returns the slot value unchanged. A missing slot in that position is `Value::Null`. When a slot sits in literal text, the missing slot renders as an empty string. Thus `${header.missing}` evaluates to `Value::Null`, while `prefix-${header.missing}-suffix` evaluates to `prefix--suffix`.

Predicate mode is strict. A predicate must evaluate to a real boolean. A bare missing header is not coerced from null or from an empty string. It fails with `type mismatch: expected bool, actual null`. Truthiness applies only inside `&&` and `||`, where null and an empty string are false. It never changes the final predicate result.

Null and empty string remain distinct. The documented escape hatch for "is this absent?" is an explicit null comparison:

```text
${header.missing} == null
```

## Rhai

Rhai reads a missing header through `header("missing")` or `headers["missing"]`. Both return Rhai unit `()`, which crosses the boundary as `Value::Null`. Predicates are strict-bool. A bare missing read fails with `type mismatch: expected bool, actual null` instead of coercing unit to false.

An `if` statement is stricter than a boolean expression. `if header("missing") { ... }` is not a false condition. It is an evaluation failure with class `TypeMismatch`, and the detail is redacted. Guard with a presence test when absence is expected.

Rhai can distinguish an absent key from a stored `null` through the map surface. Both read as unit, but presence tests tell them apart:

```rhai
if "missing" in headers {
    // the key is present
}
```

`headers.contains("missing")` is the equivalent call form.

## MiniJinja

MiniJinja runs with `UndefinedBehavior::Strict` (MiniJinja 2.24.0, pinned in `Cargo.lock`). An undefined path is a render error, not an empty string. Both interpolation and conditionals fail:

```jinja
{{ headers.missing }}
```

```jinja
{% if headers.missing %}...{% endif %}
```

The error is redacted and names the undefined path or kind. This is deliberate and fail-closed. Silent data loss is worse than a loud failure: a template that quietly renders a missing field as empty can ship an incomplete HTML page or a malformed JSON payload. A render failure stops the step, so the route author sees the problem at the first exchange that hits the path. A permissive default would hide it in production traffic.

Two opt-outs are verified against MiniJinja 2.24.0 under strict undefined. The `default` filter supplies a fallback for one interpolation:

```jinja
[{{ headers.missing | default('') }}]
```

This renders `[]` when the header is absent.

The `is defined` test guards a read without touching the value, so the else branch runs:

```jinja
{% if headers.missing is defined %}yes{% else %}no{% endif %}
```

This renders `no` when the header is absent. Use one of these opt-outs when a missing value is expected. Do not relax the engine globally.

## Choosing a language

Use Simple when a missing value should be null in a whole slot and empty in prose. Use Rhai when the script needs presence tests or branching around optional data. Use MiniJinja when a template must fail loudly on an incomplete context. Each contract is stable and test-pinned; pick the one that matches the failure mode the route needs.

**Reference**: [Language SPI](https://github.com/kennycallado/rust-camel/blob/main/crates/languages/camel-language-api/CONTEXT.md) · ADR-0032
