# Declarative camel test
`camel test` loads each `*.test.yaml` (or `*.test.yml`) document. The document selects route files, injects `direct:` inputs, and asserts `mock:` expectations. An optional `intercepts` block adds route interception without editing production routes. `camel run` wildcard discovery skips test documents; naming one literally in a route pattern fails with a reserved-suffix error that points at `camel test`. `camel run` never parses the `intercepts` block.

## Intercepts
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

## Bean stubs
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

## Endpoint expectations
`expects` maps a `mock:` endpoint name to an expectation object. The object may hold `count`, `minCount`, `maxCount`, a `bodies` list, and a `headers` map. `count` is mutually exclusive with `minCount` and with `maxCount`. `minCount` together with `maxCount` means the inclusive range `[minCount, maxCount]`; `minCount` above `maxCount` is a document error. An explicit `maxCount: 0` asserts absence: the endpoint must receive no exchanges while the document settles. Example: `expects: {mock: out: {minCount: 1, maxCount: 2}}` passes with 1 or 2 exchanges and fails with 3.

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

`sequence` asserts cross-endpoint arrival order. It is a top-level list of `mock:` endpoint refs in the order the arrivals must have happened. The list needs at least two entries; fewer is a document error and `camel test` exits with code 2. Duplicates are allowed: the same endpoint may appear for consecutive arrivals. Each entry must carry the `mock:` scheme; the prefix is stripped to the bare endpoint name exactly as for an `expects` key, so `mock:probe-a` addresses the endpoint `probe-a`.

The assertion projects the arrivals at the listed endpoints in global arrival order and requires the projection to equal the declared list exactly. Arrivals at unlisted endpoints are ignored, so the assertion narrows to the probes that matter while `expects` handles the rest. A mismatch is an assertion failure: `camel test` prints a `FAIL` line naming the first divergence — the position, the expected endpoint, and the actual endpoint — and exits with code 1. Parse errors (fewer than two entries, an entry that is not a `mock:` URI or names an empty endpoint path) exit with code 2.

## Probe pattern
Observe an intermediate route send by diverting a copy to a probe endpoint, then assert the order of those observations with `sequence:`. `divertCopyTo` copies the exchange to the probe before the real send continues, so the route is unchanged.

```yaml
intercepts:
  seda:audit: {divertCopyTo: mock:probe-a}
  seda:persist: {divertCopyTo: mock:probe-b}
expects:
  mock:probe-a: {count: 1}
  mock:probe-b: {count: 1}
sequence: [mock:probe-a, mock:probe-b]
```

Cross-endpoint order is deterministic only between causally-ordered sends — sequential route steps, reply chains. Two concurrent branches that race to different probes produce a happened-order that `sequence:` faithfully reports but that is nondeterministic between runs. Assert order only over sends the route orders; concurrent branches assert happened-order only.

## Reply assertions
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

## Repository stubs
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

## Env fixtures
An optional `env:` map declares string fixture values for the unit-tier interpolation seams. Route files, inline `routes:` sources, and the doc-side identifier fields (repository and bean stub keys, intercept sources and targets, `mock:` references, `inputs[].to`) consult the map before their inline `:-default`. The map is the only resolution source beyond the defaults: the ambient process environment is never read, so runs stay hermetic.

```yaml
env:
  CACHE_REPO_NAME: faststub
repositories:
  cache:
    "${env:CACHE_REPO_NAME:-persistent}": memory
```

The placeholder on the stub key resolves to `faststub`. A route file carrying the matching reference resolves through the same map, so the step asks for the repository the document stubbed:

```yaml
- cache:
    repository: "${env:CACHE_REPO_NAME:-persistent}"
    key: k
```

Both sides name `faststub`; the stub registers and the route loads. Without the `env:` map the same placeholder pair would resolve to the default `persistent` on both sides — the stub key and the step would still agree, but the fixture steers the name without editing either file.

Every `env:` value must be a string. An integer, boolean, or null value is a document error. Values are data, never re-interpolated: the value text is substituted verbatim, so a value that itself looks like a placeholder stays literal. Typing follows the route-side contract: a substituted leaf keeps string typing, so an integer- or boolean-typed field carrying a placeholder fails the document exactly as `camel run` rejects it, even when the `env:` map supplies the value. Numeric-typed knobs are tracked separately.

Identifier interpolation has a fixed grammar and scope. The doc-side identifier fields resolve `${env:NAME:-default}` through the same scanner as the route sources. A stub key and its route reference thus always name the same value. Interpolation runs after deserialization and before the other document checks. The scheme, blank-name, and built-in `memory` guards see the resolved value. A placeholder without a default and without an `env:` entry fails document validation at exit 2. The message names the variable and the field position. Assertion data and path fields stay literal. Input `body` and `headers` values, `expectReply` blocks, matcher contents, bean `methods` and `config` values, repository stub targets, `settle`, and the `routeFiles` and `routeFilesFromRoot` paths are never interpolated. The scope follows the mock-testkit spec requirement "Doc-side identifiers interpolate through the document env layer for name-match parity with route sources" (`openspec/specs/mock-testkit/spec.md` in the repository, outside the rendered book). The regression that motivated the requirement is tracked as bd rc-4hexo.

## Settling before assertions
`camel test` settles traffic before it evaluates expectations. The settle mode is structural: it is derived from the routes, not declared in the document. A document whose routes consume from no self-firing source runs in **completion mode**. A document with at least one such consumer runs in **stability mode**. In the lean registry the only self-firing source is `timer:`; `direct`, `log`, `mock`, and `seda` are demand-driven.

Completion mode settles on the in-flight quiescence notification: the context-global accepted-not-completed counter releases its last claim. No quiet window applies. `settle:` is the settle timeout, a humantime string within `0 < settle <= 5s`; the default is 5s. The timeout is anchored after input delivery, so delivery time never consumes the settle budget. An expired deadline fails before any idle acceptance; an idle counter with an unexpired deadline completes at once.

Stability mode cannot use counter quiescence: a self-firing source keeps producing traffic that no input claims. The quiet window (default 250ms, overridden by `settle:`) must elapse with no change in the expected endpoints' `received_count`. Every arrival that changes a sampled count restarts the window. The document-wide deadline starts when route execution begins and equals one full quiet window plus a 5s instability budget, so any valid `settle:` value can satisfy its own window.

In both modes a count above its expectation does not end settling — only the mode's completion condition does. A run that reaches its deadline without settling fails the document with a settle-timeout message and exit code 1. It never hangs.

Historical note: an interim mitigation set `settle: 50ms` to shorten lean runs (4.9x on lean batches; 258ms down to 52.7ms per document). Completion mode supersedes that mitigation: lean documents now settle in microseconds without tuning. Existing `settle: 50ms` configurations keep working — the value now acts as the completion-mode deadline.

The contract lives in the mock-testkit spec (`openspec/specs/mock-testkit/spec.md` in the repository — outside the rendered book), requirement "Settling before assertion".

## CI output and filters
`camel test` accepts five flags for CI use: `--junit`, `--filter-file`, `--filter-endpoint`, `--unit`, and `--integration`.

`--junit <FILE>` writes a JUnit XML report after the run. The report holds one `testsuite` per attempted document, named by the document path as displayed in stdout. Each suite carries a `<property name="tier">` row with the derived tier (`lean` or `full`) when the tier is known. Each assertion row becomes one `testcase` with the same label as its `PASS`/`FAIL` line (endpoint name, `reply[i] <to>` reply label, `<settle>`). A failing row carries a `<failure>` element. A document-level error (unreadable file, parse error, boot failure, route load failure, input delivery failure) becomes one `<error>` testcase named `<document>` in that document's suite. An expansion-level error (unreadable directory entry, zero-document directory) becomes one synthetic suite named by the path in the error, with a single `<error>` testcase named `<expansion>`. The report is written on exit-0, exit-1, and exit-2 runs alike. It is not written when a filter flag fails validation (see below). A report write failure prints to stderr and exits 2.

`--filter-file <GLOB>` narrows the expanded document set to documents whose entire displayed-path string matches the glob. The glob follows `glob`-crate semantics: `*` does not cross `/`, and `**` does. The match happens before reading, so filtered-out documents are never read or parsed. Directory arguments display the paths as collected: a `.` argument yields `./`-prefixed paths, and an absolute argument yields absolute paths. Patterns must account for the prefix. For example, `--filter-file './sub/**'` matches the `./sub/`-prefixed paths a `.` argument produces.

`--filter-endpoint <NAME>` narrows the set to file-admitted documents whose `expects` map contains the given name. The match is exact against the bare endpoint name (the URI suffix after `mock:`). Scenario documents declare no `expects`, so an endpoint filter excludes them. Select scenario documents with `--filter-file` or by naming them on the command line. A file-admitted document that fails to parse still reports its error and sets exit 2, regardless of the endpoint filter.

`--unit` and `--integration` are symmetric tier filters. `--unit` runs only documents that derive the lean tier. `--integration` runs only documents that derive the full tier. The tier is content-derived, so the filter applies after parsing and tier derivation. A nonmatching document found through directory expansion is excluded silently. A nonmatching document named explicitly on the command line fails with `tier-filter-collision` and exits 2. Supplying both flags together is misuse. `camel test` rejects it before any document is read and exits 2.

Every executed document prints one tier annotation line before its `PASS`/`FAIL` rows: `[lean]` for the unit tier, `[full]` for the integration tier. CI parsers that consume stdout must account for these lines.

Exit codes follow a fixed contract. Verdict failures exit 1: expectation mismatch, settle timeout, reply assertion failure, scenario `receive-timeout`, and `validation-mismatch`. Apparatus failures exit 2: runtime `scenario-var-unresolved` (an unset variable is an authoring bug, not a product failure), `action-transport-failure`, `partner-startup-failure`, `partner-bind-failure`, `log-capture-unavailable`, `shutdown-failure`, and `infra-unavailable`. Document validation failures exit 2: unreadable file, parse error, boot failure, and harness wiring errors. Precedence is 2 over 1 over 0.

Filters combine as AND across kinds and OR within repeats of one kind. The tier filter counts as a kind. When at least one filter is given and no document survives, `camel test` prints a misuse error naming the filters and exits 2. An invalid glob pattern prints to stderr and exits 2 before any document runs.

Split a large suite across CI jobs with `--filter-file`. Each job runs one shard and writes its own report. Example: a job that runs only the `shard-1` documents:

```text
camel test . --junit shard-1.xml --filter-file './src/**/shard-1*'
```

Annotating pull requests from the report requires the CI platform's JUnit publisher or report-ingest integration. On GitHub Actions, upload the report as an artifact and pass it to a JUnit-annotation action of your choice.

