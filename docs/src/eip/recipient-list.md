# Recipient List

The Recipient List is a Message Router from Hohpe and Woolf. It evaluates an expression once to resolve a list of endpoints, then sends a copy of the exchange to each one.

```rust,ignore
{{#include ../../../examples/recipientlist/src/main.rs:recipient-list-route}}
```

<details>
<summary>YAML equivalent</summary>

```yaml
- id: recipientlist-demo
  from: timer:tick?period=2000&repeatCount=3
  steps:
    - set_header:
        key: destinations
        value: "log:channel-a?showBody=true,log:channel-b?showBody=true,log:channel-c?showBody=true"
    - recipient_list:
        simple: "${header.destinations}"
        parallel: true
    - to: log:summary?showBody=true
```

</details>

The route stores three `log` endpoint URIs in the `destinations` header. The `.recipient_list_with_config(...)` call takes a closure of type `RecipientListExpression`. That closure reads the header and returns the comma-separated string. The processor splits the string, resolves each URI, and dispatches a clone of the exchange to each endpoint. The `.parallel(true)` flag runs the three dispatches concurrently instead of one after the other.

The exchange that reaches the next step depends on the aggregation strategy. The default `LastWins` strategy forwards one branch's result to the step that follows. Set `MulticastStrategy::Original` to pass the input exchange through unchanged and discard every branch output. Set `CollectAll` to gather each branch body into a JSON array. The example keeps the default, so `log:summary` receives the result of one resolved branch.

Use the Recipient List when the destinations are data. A header, a registry lookup, or a computation decides the targets at runtime. Use [Multicast](multicast.md) when the targets are fixed in the route. A [Content-Based Router](content-based-router.md) picks one branch from a set. The Recipient List fans out to all of them. A guard caps the resolved list at `max_recipients` (default 1000) before any endpoint resolves. An expression that returns millions of URIs cannot exhaust memory.

The recipient list compiles into a plain processor step, not an outcome-aware segment. A branch error surfaces through the same step-error boundary as any other step: the route error handler decides recovery. A partial dispatch failure leaves the remaining branches' results available to the aggregation strategy.

Per [ADR-0001](../adr/0001-tower-data-plane-split-from-control-plane.md), the recipient list compiles into a `Service<Exchange>` step in the Tower middleware pipeline. The processor contract is documented in [camel-processor/CONTEXT.md](https://github.com/kennycallado/rust-camel/blob/main/crates/camel-processor/CONTEXT.md).

The example source is at [`examples/recipientlist`](https://github.com/kennycallado/rust-camel/tree/main/examples/recipientlist).
