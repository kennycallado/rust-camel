# Do Try

The Do Try is an Error Handling pattern from Hohpe and Woolf. It wraps a group of steps in a local scope with one or more catch clauses and an optional finally clause. A route can repair or clean up after a failing step without triggering the route-level error handler.

```rust,ignore
{{#include ../../../examples/do-try/src/main.rs:do-try-route}}
```

<details>
<summary>YAML equivalent</summary>

```yaml
- id: catch-by-variant
  from: direct:catch-by-variant
  steps:
    - do_try:
        steps:
          - to: direct:failing-op
        catch:
          - exception:
              - ProcessorError
            steps:
              - to: log:caught-by-variant
```

</details>

A `do_try()` block opens the scope. Steps inside the try body run in sequence. When a step returns `Err`, the block walks its catch clauses in order and runs the body of the first match. `do_catch_exception(&["ProcessorError"])` matches by `CamelError` variant name. `do_catch_when(predicate)` matches by a `FilterPredicate` over the exchange when the variant alone is not specific enough. The example shows both shapes. List specific clauses before broad ones. The first match wins.

`do_finally()` adds a body that runs exactly once whether the try body succeeded, threw, or was caught. Use it for cleanup that must always run. The example's third route pairs a `.propagate()` catch with a `do_finally` counter. The catch logs the failure and lets the error escape. The finally block still runs and bumps the counter.

Each catch clause ends with a disposition. The disposition decides what happens to the exchange after the catch body runs. `Handled` clears the error and stops the block. `propagate()` keeps the error live so it escapes after the catch body finishes. The full disposition model, with all values and their effects on the pipeline, lives in [error handling](../concepts/error-handling.md). The block is a local error-handling island. A `Handled` catch never reaches the route's `error_handler`. Only unhandled errors and errors from a `propagate()` catch escape to the route level.

Use `do_try` when the repair is scoped to one step or a small group of steps. Use the [route-level `error_handler`](../concepts/error-handling.md) when every step in the route shares one retry, dead-letter, or disposition policy. The two compose. A `Handled` catch repairs a step locally. Unhandled failures still fall through to the route-level safety net.

Per [ADR-0001](../adr/0001-tower-data-plane-split-from-control-plane.md), the do-try compiles into a `Service<Exchange>` step in the Tower middleware pipeline. The try body and each catch branch compile as child steps on the same route channel. The processor contract is documented in [camel-processor/CONTEXT.md](https://github.com/kennycallado/rust-camel/blob/main/crates/camel-processor/CONTEXT.md).

The example source is at [`examples/do-try`](https://github.com/kennycallado/rust-camel/tree/main/examples/do-try).
