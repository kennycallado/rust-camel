# Testing

This section describes how to test routes with the lean `camel test` boot and with route interception. Interception rewrites `to:` send points at compile time. It supports isolated unit tests without `mock:` lines in production routes.

## Route interception

Use route interception to replace or copy a send without changing the route. Rules run at compile time. Validation happens once, in `InterceptRules::new`; the compiler consults the frozen rules at each send point.

Two actions exist:

- `SkipTo` replaces the original send. The exchange goes only to the `mock:` target.
- `DivertCopyTo` copies the exchange to a `mock:` target and then runs the real producer. The copy uses WireTap semantics: detached when the bound (20) admits it, inline `CallerRuns` when saturated.

Targets must be `mock:` URIs. `InterceptRules::new` rejects other targets at build time. The match is exact URI, first-match-wins.

Rules freeze at first successful route registration or at context start. After freeze, `set_intercept_rules` returns `CamelError::Config`. Use `CamelContextBuilder::with_intercept_rules` before freeze.

### SkipTo example

```rust,ignore
use camel_core::intercept::{InterceptAction, InterceptRule, InterceptRules};
use camel_core::{CamelContext, RouteDefinition};
use camel_core::route::BuilderStep;

let rules = InterceptRules::new(vec![InterceptRule {
    uri: "seda:out".into(),
    action: InterceptAction::SkipTo { uri: "mock:tap".into() },
}])?;

let mut ctx = CamelContext::builder()
    .with_intercept_rules(rules)
    .build()
    .await?;

ctx.add_route_definition(
    RouteDefinition::new("direct:in", vec![BuilderStep::To("seda:out".into())])
        .with_route_id("send-route"),
)
.await?;
```

The send `to: seda:out` never reaches `seda:`. The exchange goes to `mock:tap` only. The `seda:` producer is not resolved.

### DivertCopyTo example

```rust,ignore
use camel_core::intercept::{InterceptAction, InterceptRule, InterceptRules};
use camel_core::{CamelContext, RouteDefinition};
use camel_core::route::BuilderStep;

let rules = InterceptRules::new(vec![InterceptRule {
    uri: "kafka:orders".into(),
    action: InterceptAction::DivertCopyTo { uri: "mock:orders-copy".into() },
}])?;

let mut ctx = CamelContext::builder()
    .with_intercept_rules(rules)
    .build()
    .await?;

// Consumer route still receives the real message.
ctx.add_route_definition(
    RouteDefinition::new("kafka:orders", vec![BuilderStep::To("mock:arrival".into())])
        .with_route_id("consumer"),
)
.await?;
ctx.add_route_definition(
    RouteDefinition::new("direct:in", vec![BuilderStep::To("kafka:orders".into())])
        .with_route_id("send"),
)
.await?;
```

The exchange goes to `mock:orders-copy` and to the real `kafka:orders` producer. A failure in the copy does not change the real outcome.

For processor composition, `camel_processor::compose_divert` builds the same divert from a `WireTapService` copy stage and a `BoxProcessor` real stage. The runtime owns the lifecycle: `WireTapLifecycle::start` reopens admission with a fresh token and tracker after restart.

Further detail lives in [`crates/camel-core/CONTEXT.md`](https://github.com/kennycallado/rust-camel/blob/main/crates/camel-core/CONTEXT.md) and [`crates/camel-processor/CONTEXT.md`](https://github.com/kennycallado/rust-camel/blob/main/crates/camel-processor/CONTEXT.md). The contract is defined in [ADR-0064](../adr/0064-two-tier-testing-contract.md).

## Declarative camel test

`camel test` loads each `*.test.yaml` document. The document selects route files, injects `direct:` inputs, and asserts `mock:` expectations. An optional `intercepts` block adds route interception without editing production routes. `camel run` ignores test documents and never parses the `intercepts` block.

### Intercepts

Declare intercepts as a map from source URI to an action object. The object holds exactly one key: `skipTo` or `divertCopyTo`. The value must be a `mock:` URI.

```yaml
intercepts:
  kafka:orders:
    skipTo: mock:orders
  seda:audit:
    divertCopyTo: mock:audit
```

`skipTo` replaces the original send before the compiler resolves the source component. The real component does not need to be in the lean set, and the exchange never reaches it. `divertCopyTo` copies the exchange to the `mock:` target and then runs the real producer. The real component must be in the lean set, because the compiler still resolves it. Divert uses WireTap semantics: detached when the bound admits it, inline `CallerRuns` when saturated. A failure in the copy does not change the real outcome.

Target and expectation share the endpoint name. `skipTo: mock:orders` and `expects: {mock:orders: {count: 1}}` both resolve to endpoint `orders` on the `mock:` component. Use the same name in both places to collect the intercepted exchange.

Matching uses the full URI verbatim. Query parameters are part of the key. `kafka:orders` does not match `kafka:orders?x=1`. List the exact URI that the route sends to.

Failure handling stays unchanged. Parse errors in the `intercepts` map and route-load errors from interception (for example, a `divertCopyTo` whose source has no registered component) are document errors. `camel test` reports them on stderr and exits with code 2. No endpoint result counts toward `passed` or `failed` in that case.

The contract lives in [ADR-0064](../adr/0064-two-tier-testing-contract.md) and the route-interception spec (`openspec/specs/route-interception/spec.md` in the repository — outside the rendered book).
