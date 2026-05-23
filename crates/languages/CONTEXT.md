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

## Example dialogue

> "I want to filter exchanges where the `type` header equals `urgent`."
> "Use a Predicate: `js` Language with `camel.headers.get('type') === 'urgent'`. The filter step evaluates the Predicate — only Exchanges where it returns `true` continue."
>
> "What if I also need to modify the body during evaluation?"
> "Use a MutatingExpression instead. It produces a value and may mutate the Exchange. Both `rhai` and `js` Languages support this."
