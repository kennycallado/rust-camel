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

### Endpoint expectations

`expects` maps a `mock:` endpoint name to an expectation object. The object may hold `count` or `minCount`, a `bodies` list, and a `headers` map. `count` and `minCount` are mutually exclusive.

`bodies` uses strict grammar. Each entry is a bare string or a single-key matcher map. A bare string is exact equality (`equals`). A map with one recognized body-matcher key selects that matcher. Any other form is a document error. `camel test` exits with code 2 and names the field and the key.

Body matchers in v1: `equals`, `regex`, `contains`, `startsWith`, `endsWith`, `exists`, `jsonSubset`. `exists` takes `null` and takes no argument. `jsonSubset` takes a JSON object. A `regex` value must be a valid pattern. The runner rejects an invalid pattern at parse time and exits with code 2.

`headers` values use dual grammar. Any literal JSON value stays exact structural equality (`equals`). A map whose sole key is `equals`, `regex`, or `exists` selects that matcher. Any other value stays a literal. `jsonSubset` on a header is a document error. `camel test` exits with code 2.

```yaml
expects:
  mock:result:
    count: 2
    bodies:
      - regex: "^order-[0-9]+$"
      - jsonSubset: {status: "ok"}
    headers:
      X-Trace: { regex: "^[a-f0-9]{8}$" }
      mode: {batch: 1, predicate: "raw"}
```

`jsonSubset` requires a JSON object pattern. The received body may be `Body::Json` or `Body::Text` that parses as JSON. A text body that does not parse fails the matcher. The received top-level JSON must be an object. Objects match recursively. Every pattern key must exist with a matching value. Nested objects match by subset. Arrays compare exactly by length, order, and element equality. Extra fields in the received object do not fail the assertion.

A sole `predicate` key is reserved. `camel test` rejects it with `predicate matchers are not supported` and exits with code 2. This applies in every matcher position. A multi-key object that contains `predicate` stays a literal in dual positions. It does not select a matcher.

Matcher mismatches are assertion failures. `camel test` prints a `FAIL` line that names the matcher, its pattern, and the received value. The received value is rendered whole. The document exits with code 1. Parse-time errors (invalid regex, non-object `jsonSubset`, wrong key count) exit with code 2.

Migration note: only literals whose single key is a matcher key change meaning in dual positions (`expectReply.body`, `expects.headers` values, `expectReply.headers` values). For example `expectReply: {body: {equals: "x"}}` previously meant literal equality of `{"equals": "x"}`. With matchers it selects `equals "x"`. Wrap the literal to keep the old meaning: `body: {equals: {equals: "x"}}`. `expects.bodies` entries were strings before, so matcher maps there add no migration. A sole `predicate` key, or a sole `jsonSubset` key on a header, parsed as a literal before; it now fails at parse with exit 2.

### Reply assertions

An input may declare `expectReply` to assert against the reply message the `direct:` producer returns. The block holds two optional keys: `body` and `headers`. At least one must be present. An empty `expectReply` is a document error.

```yaml
inputs:
  - to: "direct:enrich"
    body: "plain"
    expectReply:
      body: "enriched"
```

`expectReply.body` uses dual grammar. Every bare scalar (string, number, boolean, `null`) and every array is literal `equals`. A string becomes `Body::Text`. Other scalars and arrays become `Body::Json`. An object with one recognized body-matcher key selects that matcher. Any other object is literal `equals` with structural equality. `expectReply.headers` values use the same dual header grammar as `expects.headers`. The reply must satisfy every expected header. Extra headers on the reply do not fail the assertion.

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

### CI output and filters

`camel test` accepts five flags for CI use: `--junit`, `--filter-file`, `--filter-endpoint`, `--unit`, and `--integration`.

`--junit <FILE>` writes a JUnit XML report after the run. The report holds one `testsuite` per attempted document, named by the document path as displayed in stdout. Each suite carries a `<property name="tier">` row with the derived tier (`lean` or `full`) when the tier is known. Each assertion row becomes one `testcase` with the same label as its `PASS`/`FAIL` line (endpoint name, `reply[i] <to>` reply label, `<settle>`). A failing row carries a `<failure>` element. A document-level error (unreadable file, parse error, boot failure, route load failure, input delivery failure) becomes one `<error>` testcase named `<document>` in that document's suite. An expansion-level error (unreadable directory entry, zero-document directory) becomes one synthetic suite named by the path in the error, with a single `<error>` testcase named `<expansion>`. The report is written on exit-0, exit-1, and exit-2 runs alike. It is not written when a filter flag fails validation (see below). A report write failure prints to stderr and exits 2.

`--filter-file <GLOB>` narrows the expanded document set to documents whose entire displayed-path string matches the glob. The glob follows `glob`-crate semantics: `*` does not cross `/`, and `**` does. The match happens before reading, so filtered-out documents are never read or parsed. Directory arguments display the paths as collected: a `.` argument yields `./`-prefixed paths, and an absolute argument yields absolute paths. Patterns must account for the prefix. For example, `--filter-file './sub/**'` matches the `./sub/`-prefixed paths a `.` argument produces.

`--filter-endpoint <NAME>` narrows the set to file-admitted documents whose `expects` map contains the given name. The match is exact against the bare endpoint name (the URI suffix after `mock:`). Scenario documents declare no `expects`, so an endpoint filter excludes them. Select scenario documents with `--filter-file` or by naming them on the command line. A file-admitted document that fails to parse still reports its error and sets exit 2, regardless of the endpoint filter.

`--unit` and `--integration` are symmetric tier filters. `--unit` runs only documents that derive the lean tier. `--integration` runs only documents that derive the full tier. The tier is content-derived, so the filter applies after parsing and tier derivation. A nonmatching document found through directory expansion is excluded silently. A nonmatching document named explicitly on the command line fails with `tier-filter-collision` and exits 2. Supplying both flags together is misuse. `camel test` rejects it before any document is read and exits 2.

Every executed document prints one tier annotation line before its `PASS`/`FAIL` rows: `[lean]` for the unit tier, `[full]` for the integration tier. CI parsers that consume stdout must account for these lines.

Exit codes follow a fixed contract. Verdict failures exit 1: expectation mismatch, settle timeout, reply assertion failure, scenario `receive-timeout`, `validation-mismatch`, and runtime `scenario-var-unresolved`. Apparatus failures exit 2: `action-transport-failure`, `partner-startup-failure`, `shutdown-failure`, and `infra-unavailable`. Document validation failures exit 2: unreadable file, parse error, boot failure, and harness wiring errors. Precedence is 2 over 1 over 0.

Scenario documents run through one of two execution paths. The build selects the path.

| Build | Endpoint schemes | Execution path |
|-------|------------------|----------------|
| default | `fake:` only | No-boot smoke path. Any other scheme reports `infra-unavailable`, names the adapter, and exits 2. |
| `integration-http` | `fake:`, `direct:`, `http:` | Embedded full boot. Real composition root, real wire, harness partner listeners. Any other scheme reports `infra-unavailable`, names the adapter, and exits 2. |

The default build provides only the in-memory `fake:` partner adapter. A scenario whose endpoints are all `fake:` runs the no-boot smoke path. A `fake:`-only scenario keeps that path in any build.

The `integration-http` build boots the real composition root. A scenario whose endpoints are all `fake:`, `direct:`, or `http:` qualifies. Each `http:` endpoint binds a harness partner listener on `127.0.0.1:0`. A `direct:` send stimulates the booted context through its own producer path. The document runs over the real wire.

Filters combine as AND across kinds and OR within repeats of one kind. The tier filter counts as a kind. When at least one filter is given and no document survives, `camel test` prints a misuse error naming the filters and exits 2. An invalid glob pattern prints to stderr and exits 2 before any document runs.

Split a large suite across CI jobs with `--filter-file`. Each job runs one shard and writes its own report. Example: a job that runs only the `shard-1` documents:

```text
camel test . --junit shard-1.xml --filter-file './src/**/shard-1*'
```

Annotating pull requests from the report requires the CI platform's JUnit publisher or report-ingest integration. On GitHub Actions, upload the report as an artifact and pass it to a JUnit-annotation action of your choice.

### Scenario documents

A `scenario:` document is the integration-tier contract of [ADR-0069](../adr/0069-integration-tier-testing-contract.md). The document declares an action list. The runner executes four actions in order: `send`, `receive`, `sleep`, and `validate`. A `send` takes an optional `method` field, for example `method: PUT`. The field is uppercased at load. Without the field, a body implies `POST` and no body implies `GET`. The [README](https://github.com/kennycallado/rust-camel/blob/main/crates/camel-integration-test/README.md) of the `camel-integration-test` crate is the grammar reference.

A scenario document may declare a `partners:` section to script the responses a harness partner serves. The section is a map from the declared endpoint string to a sequence of script entries. The same document interpolates `${name}` in endpoint strings, body string leaves, and header values. `bindVar` fills a scenario variable with the partner's bound authority. The crate [README](https://github.com/kennycallado/rust-camel/blob/main/crates/camel-integration-test/README.md) documents the `partners:` shape, the interpolation surface, and the two-layer `bindVar` rule.
