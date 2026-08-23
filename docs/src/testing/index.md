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
