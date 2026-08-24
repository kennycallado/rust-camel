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

`camel lint` warns `R-MOCK-IN-PRODUCTION` on inline `to: mock:` and `endpoints: mock:` sends in route files. The warning is exempt for `tests/fixtures/` paths and `*.test.yaml` documents. Migrate the send to an `intercepts:` block in a `*.test.yaml` document, as described above.

### Bean stubs

A `beans:` block declares stub beans for the `bean:` steps in the routes. A stub bean is an in-process processor registered in the bean registry before the context boots. The `bean:` step resolves against it, so the test runs without a real bean implementation. The block maps a bean name to a declaration.

```yaml
beans:
  validator:
    kind: echo
  enricher:
    kind: setBody
    config:
      body: enriched
```

Each declaration has a `kind` and an optional `methods` list and `config` map. The `kind` selects the stub behavior.

| Kind | Config | Behavior |
|------|--------|----------|
| `echo` | none | Passes the exchange through untouched. |
| `setBody` | `body` (required) | Replaces the input body with the configured string. |
| `fail` | `message` (optional) | Fails with the configured message. Without `message`, it fails with exactly `fail bean <name>`. |

`echo` accepts no config keys. `setBody` requires `body` and rejects any other key. `fail` accepts only `message`. A config key that does not fit the kind is a document error.

The `methods` list is an allowlist. When omitted, the stub accepts every method the routes invoke on it. When present, the runner cross-validates it against the methods the routes call before boot. A route that calls a method outside the list is a document error and exits with code 2.

A `fail` stub surfaces as a document error. The runner reports it on stderr and exits with code 2. Settling and evaluation are skipped. The default message `fail bean <name>` uses the declared bean name.

The stub beans mirror the `bean:` step. The step looks up a bean by name and calls a method on it. The stub supplies that lookup in the test. See [Bean](../steps/bean.md) for the step contract. The example pair lives in [`examples/yaml-dsl/config/beans-demo.yaml`](https://github.com/kennycallado/rust-camel/blob/main/examples/yaml-dsl/config/beans-demo.yaml) and [`beans-demo.test.yaml`](https://github.com/kennycallado/rust-camel/blob/main/examples/yaml-dsl/config/beans-demo.test.yaml).

### Reply assertions

An input may declare `expectReply` to assert against the reply message the `direct:` producer returns. The block holds two optional keys: `body` and `headers`. At least one must be present. An empty `expectReply` is a document error.

```yaml
inputs:
  - to: "direct:enrich"
    body: "plain"
    expectReply:
      body: "enriched"
```

The `body` value is a string or a JSON value. A string matches the reply body exactly. A JSON value matches structurally. The `headers` value is a map from header name to JSON value. The reply must carry every expected header with an equal value. Extra headers on the reply do not fail the assertion.

The reply message is the route output when the route set one. Otherwise it is the final input message. Nothing in the lean `camel test` component set sets the output today. The reply pairs with the input by delivery order. Inputs deliver strictly sequentially, so `reply[i]` matches the `i`-th input.

Each asserted input produces one result row labeled `reply[i] <input.to>`. A mismatch is an assertion failure. It surfaces as a `FAIL` line and counts toward `failed`. The document exits with code 1. A delivery error is a document error. It exits with code 2 and skips reply evaluation.

A document may omit `expects` when at least one input declares `expectReply`. The reply assertions then drive the outcome. A document with neither endpoint expectations nor any `expectReply` still fails to parse.

The example pair lives in [`examples/yaml-dsl/config/reply-demo.yaml`](https://github.com/kennycallado/rust-camel/blob/main/examples/yaml-dsl/config/reply-demo.yaml) and [`reply-demo.test.yaml`](https://github.com/kennycallado/rust-camel/blob/main/examples/yaml-dsl/config/reply-demo.test.yaml).

### Repository stubs

A `repositories:` block declares in-memory stubs for the named repositories that `cache:`, `idempotent:`, and `claimCheck:` steps resolve against. The block maps a registry kind to a map of repository name to stub target. The only valid target in v1 is the literal `memory`.

```yaml
repositories:
  cache:
    persistent: memory
  idempotent:
    dedupe: memory
  claimCheck:
    store: memory
```

Three registry kinds exist: `cache`, `idempotent`, and `claimCheck`. Each maps repository names to the stub target. The runner registers a fresh memory backend under each declared name before the routes load. The steps then resolve at compile time. Only the `memory` target is supported. Any other target is a document error.

The built-in name `memory` is not stubbable. Registering it would collide with the built-in repository, so the runner rejects it. Blank repository names are rejected too. An undeclared name still fails route load. A stub resolves only its explicitly declared name. A typo hits the same compile-time `ComponentNotFound` gate as production. An unknown registry kind is a document error that lists the three supported kinds.

Stubs are lossy. The `R-REPOSITORY-STUB` warning on stderr names each stubbed registry and repository and lists the semantics the memory backend does not exercise: for `cache`, prefix purge, TTL/stale timing, disk offload, and stats; for `idempotent` and `claimCheck`, persistence; for all, backend failure. Cover these in the integration tier.

The example pair lives in [`examples/yaml-dsl/config/repositories-demo.yaml`](https://github.com/kennycallado/rust-camel/blob/main/examples/yaml-dsl/config/repositories-demo.yaml) and [`repositories-demo.test.yaml`](https://github.com/kennycallado/rust-camel/blob/main/examples/yaml-dsl/config/repositories-demo.test.yaml).
