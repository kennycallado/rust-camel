# Languages

Expression and predicate evaluation against Exchanges. Each Language compiles a script or pattern into an executable that the Runtime invokes within Pipeline steps.

## Language

**Language**:
Factory that compiles scripts or patterns into Expressions, Predicates, or MutatingExpressions. Registered into CamelContext by name (e.g., `js`, `jsonpath`, `xpath`, `simple`, `rhai`).
_Avoid_: scripting engine, evaluator, interpreter

**Expression**:
Evaluates against an Exchange to produce a value. Does not modify the Exchange.
_Avoid_: script, function, query

**Predicate**:
Evaluates against an Exchange to produce a boolean. Used in filter, choice, and validation steps.
_Avoid_: condition, test, rule

**MutatingExpression**:
An Expression variant that may also modify the Exchange's body, headers, or properties as a side effect of evaluation. Used where a pipeline step needs both a value and state mutation.
_Avoid_: transformer expression (MutatingExpression is the precise term)

**MutatingPredicate**:
A Predicate variant that may also modify the Exchange as a side effect of evaluation.
_Avoid_: mutating condition, side-effecting predicate

## Implementations

- **[JavaScript](./camel-language-js/CONTEXT.md)** — synchronous Boa-backed expression, predicate, and mutating-expression language. Its crate context defines the in-process sandbox, exchange-data boundary, and resource limits. Authority: ADR-0006 and ADR-0032.
- **[MiniJinja](./camel-language-minijinja/CONTEXT.md)** — template rendering language for inline template execution. Produces structured output (HTML, JSON, prompts) from Exchange data via MiniJinja (Python Jinja2-inspired). Phase 1 covers inline templates only; Phase 2 (bd rc-64if, `crates/components/camel-template`) adds external file loading, includes, and hot-reload. Authority: ADR-0047.
- **[Rhai](./camel-language-rhai/CONTEXT.md)** — sandboxed expression, predicate, and mutating-expression language. Its crate context defines the unconditional host-access closure, resource limits, and mutation model. Authority: ADR-0032.
- **[XPath](./camel-language-xpath/CONTEXT.md)** — XPath 1.0 expression and predicate language over `sxd-xpath`. It evaluates a trusted query against an untrusted XML body with a configurable input-size bound. Authority: ADR-0032.
- **[JSONPath](./camel-language-jsonpath/CONTEXT.md)** — RFC 9535 JSONPath expression and predicate language over `jsonpath-rust`. It evaluates a trusted query against an untrusted JSON body. Default limits are 16 MiB for text input and 64 levels of nesting. Authority: ADR-0032.

## Example dialogue

> "I want to filter exchanges where the `type` header equals `urgent`."
> "Use a Predicate: `js` Language with `camel.headers.get('type') === 'urgent'`. The filter step evaluates the Predicate — only Exchanges where it returns `true` continue."
>
> "What if I also need to modify the body during evaluation?"
> "Use a MutatingExpression instead. It produces a value and may mutate the Exchange. Both `rhai` and `js` Languages support this."
