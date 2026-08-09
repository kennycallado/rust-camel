# Validator

The Validator is a Message Transformation pattern from Hohpe and Woolf. It checks an exchange body against a schema or predicate. It rejects the exchange when the check fails, so the rest of the route only sees valid input.

```rust,ignore
{{#include ../../../examples/validator/src/main.rs:validator-route}}
```

<details>
<summary>YAML equivalent</summary>

```yaml
- id: xsd-valid
  from: timer:xsd-valid?period=3000&repeatCount=2
  steps:
    - set_body:
        value: "<order><id>A1</id><amount>5</amount></order>"
    - validate: "${body.contains('<order>')}"
    - to: log:info?showBody=true
```

</details>

The included route fires a timer twice. The `.set_body(...)` step sets the body to an XML order. The `.validate(&xsd)` step loads the XSD schema from the configured path and checks the body against it. The log step after the validator runs only when the body is valid. A second route in the example sends an invalid order. It pairs the validator with an `error_handler`, so the validation error is logged instead of crashing the process.

On a mismatch the validator returns an error. The error flows back through the same `RouteErrorHandler` boundary as any other step error. A route-level `error_handler` or a `do-try` catch block can recover it. Wrap the validator in a `do-try` block to keep the exchange in the pipeline after a failure. Use the bare validator to fail fast on bad input. The YAML `validate` step also accepts a predicate expression like `${body.contains('<order>')}` for inline checks without a schema file.

The Validator differs from the Filter. The Validator stops the route on a failed check and surfaces the error. The Filter silently drops the exchange. Use the Validator for input validation and contract enforcement at a trust boundary. Use the Filter when an exchange is well-formed but not relevant.

Per [ADR-0001](../adr/0001-tower-data-plane-split-from-control-plane.md), the validator compiles into a `Service<Exchange>` step. The schema engine runs inside the step. The processor contract is documented in [camel-processor/CONTEXT.md](https://github.com/kennycallado/rust-camel/blob/main/crates/camel-processor/CONTEXT.md).

The example source is at [`examples/validator`](https://github.com/kennycallado/rust-camel/tree/main/examples/validator).
