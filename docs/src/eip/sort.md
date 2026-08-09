# Sort

The Sort is a Message Translator from Hohpe and Woolf. It orders the elements of a body array by a sort key expression. The route downstream sees the array in the chosen order.

```yaml
{{#include ../../../examples/sort/routes.yaml:sort-route}}
```

The included route fires a timer every second for three ticks. The `set_body` step sets the body to a JSON array of eight numbers. The `sort` step orders the array by the `${body}` expression, which uses each element itself as the sort key. The `to` step logs the sorted array. The sort defaults to ascending order. A `reverse: true` flag produces descending order.

The Sort step reads the body as a JSON array. A non-array body fails with a processor error. The upstream `set_body` step guarantees that contract for this example. The sort key expression extracts a comparable value from each element. The expression `${body}` uses the element itself. The expression `${body.field}` extracts a nested field for sorting objects by a property.

The Sort differs from the [Splitter](splitter.md). The Sort keeps the body as a single array and reorders it in place. The Splitter decomposes the body into many exchanges. A route that needs both patterns sorts first and then splits per element. The Sort also pairs with the Aggregator. A split-sort-aggregate flow splits a collection, sorts the fragments, then aggregates the results back into one message.

Per [ADR-0001](../adr/0001-tower-data-plane-split-from-control-plane.md), the sort step compiles into a `Service<Exchange>` step in the Tower middleware pipeline. The sort runs inside the step and the ordered body flows out before the next step runs. The processor contract is documented in [camel-processor/CONTEXT.md](https://github.com/kennycallado/rust-camel/blob/main/crates/camel-processor/CONTEXT.md).

The example source is at [`examples/sort`](https://github.com/kennycallado/rust-camel/tree/main/examples/sort).
