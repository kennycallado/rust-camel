# Scenario documents
A `scenario:` document is the integration-tier contract of [ADR-0069](../adr/0069-integration-tier-testing-contract.md). The document declares an action list. The runner executes five actions in order: `send`, `receive`, `sleep`, `validate`, and `sql`. A `send` takes an optional `method` field, for example `method: PUT`. The field is uppercased at load. Without the field, a body implies `POST` and no body implies `GET`. The [README](https://github.com/kennycallado/rust-camel/blob/main/crates/camel-integration-test/README.md) of the `camel-integration-test` crate is the grammar reference.

A scenario document may declare a `partners:` section to script the responses a harness partner serves. The section is a map from the declared endpoint string to a sequence of script entries. The same document interpolates `${name}` in endpoint strings, body string leaves, and header values. `bindVar` fills a scenario variable with the partner's bound authority. The crate [README](https://github.com/kennycallado/rust-camel/blob/main/crates/camel-integration-test/README.md) documents the `partners:` shape, the interpolation surface, and the two-layer `bindVar` rule.

A scenario can script failure paths. A `partners:` entry may declare `delay`, `fault: close`, and `times`. The harness holds for the delay before it serves the response or commits the fault. The `fault: close` drops the connection without an HTTP response. The `times` repeats an entry for a fixed number of matching requests. A route-level `error_handler.retry` redials after a fault, so a fault-to-healthy sequence tests retry behavior.

A `validate` action with a `partner` target asserts recorded-request traffic: exactly one count bound (`count` exact, `atLeast`, `atMost`, or an `atLeast`/`atMost` range), or a positional `requests` list that implies the count. Optional `method`, path, and `query` filters narrow it, and a `deadline` polls until the expectation settles. The runnable example pair lives in [`examples/integration-testing/partner-retry-route.test.yaml`](https://github.com/kennycallado/rust-camel/blob/main/examples/integration-testing/partner-retry-route.test.yaml) and [`partner-retry.routes.yaml`](https://github.com/kennycallado/rust-camel/blob/main/examples/integration-testing/partner-retry.routes.yaml).

A partner target's URI must equal a harness endpoint reference the scenario's own `send`/`receive` actions declare, or self-declare one. The object target form self-declares: `provisioning: harness` on an `http` endpoint plus a `partners:` entry naming the URI. The validate's own reference then wires the partner exactly as a `send`/`receive` reference does: the driver binds the partner and fills the reference's `bindVar` with the bound authority.

Proxy routes whose query varies per request need this form. The varying query makes each request dial a different wire path, so no literal arrival lane exists for a `receive` to name; the recorded traffic is the only assertion surface, and the scenario runs with no sacrificial receive. The route step reads the bound authority and appends the query — for example `to: ${env:UPSTREAM}/tiles?bbox=1.2`:

```yaml
scenario:
- send: {to: direct:start}
- validate:
    target: {partner: {endpoint: http://upstream/tiles, provisioning: harness, bindVar: UPSTREAM}}
    expectation: {count: 1}
    deadline: 5s
partners:
  http://upstream/tiles:
  - path: /tiles?bbox=1.2
    response: {status: 200, body: tile}
```

One declared harness endpoint and one `bindVar` can serve every path a
route dials. The route's `to:` URIs share the one authority and differ
only in path, for example `http://${MOCK}/orders` and
`http://${MOCK}/billing`. The `partners:` section scripts each path
under the same declared key, one entry per path with its own response
body. The two-key rule governs resolution: the declared endpoint string
is the provisioning key that binds the listener, and an interpolated
reference such as `http://${MOCK}/billing` is a dynamic reference the
router resolves to the already-bound partner by authority. Arrivals queue
per request path on that single listener.

Do not declare one endpoint per path. The N-bindVar fan-out provisions
one listener per path for what is one logical partner, and the extra
bindings reassign ports spuriously; this caused the 2026-09-06 pilot
incident. Each dynamic-reference receive names its own path, and each
drains its own arrival lane on the single listener: `from:
http://${MOCK}/orders` drains the orders lane, `from:
http://${MOCK}/billing` drains the billing lane. A dynamic receive must
name a path — a bare authority is an apparatus error. The registered
key remains the adapter-lookup key; the path on the wire picks the lane.
In the client role, standalone roundtrip receives drain their own
path's parked roundtrip oldest-first: two receives naming different
paths of one partner never cross-match (bd rc-cr5yf). The runnable
example pair lives in
[`examples/integration-testing/partner-multi-path.test.yaml`](https://github.com/kennycallado/rust-camel/blob/main/examples/integration-testing/partner-multi-path.test.yaml) and [`partner-multi-path.routes.yaml`](https://github.com/kennycallado/rust-camel/blob/main/examples/integration-testing/partner-multi-path.routes.yaml).

A migration note for suites that grew one script per assertion: express
each independent assertion chain as its own scenario document.
Documents run independently, and the runner continues past a failing
document, so one `camel test` run reports every chain. ADR-0069
section 11 keeps the ordered action list as the pin: the actions inside
one document stay ordered.

Back-to-back `send:` actions with no intervening `receive:` dispatch genuinely concurrent requests. The crate [README](https://github.com/kennycallado/rust-camel/blob/main/crates/camel-integration-test/README.md) documents the burst-send recipe for asserting the concurrent arrivals. A `direct:` send may declare `expectReply` to assert the route's synchronous reply; the README's usage/grammar area details the verb. The scenario-tier field takes matcher verbs directly, unlike the unit tier's `expectReply` block.

A scenario document may declare one document-level `logs:` block. The harness captures every tracing event emitted while the document runs, and it evaluates the block after the action list completes. Three clauses exist, and every declared clause must hold. `contains` lists substrings of captured event messages. `regex` lists unanchored patterns; each pattern matches against the composite camel-log message. `noLevelAbove` sets a level cap over the whole document window; the cap spans every target, route processors and harness tasks alike. An unknown level, an invalid pattern, or an unknown key fails the load.

Log capture is process-global. Documents that run concurrently in one process attribute events conservatively: the harness files an event in every open window, so a sibling document's WARN can fail this document's `noLevelAbove` cap. Serialize log-asserting documents, or keep them on the current-thread itest path, when a document needs strict isolation.

## Execution paths and feature builds

Scenario documents run through one of two execution paths. The build selects the path.

| Build | Endpoint schemes | Execution path |
|-------|------------------|----------------|
| `--no-default-features` | `fake:` only | No-boot smoke path. Any other scheme reports `infra-unavailable`, names the adapter, and exits 2. |
| default (`integration-http`, `integration-sql`) | `fake:`, `direct:`, `http:`; `sql:` actions | Embedded full boot. Real composition root, real wire, harness partner listeners. Any other scheme reports `infra-unavailable`, names the adapter, and exits 2. |
| `integration-http` | `fake:`, `direct:`, `http:` | Embedded full boot. Real composition root, real wire, harness partner listeners. Any other scheme reports `infra-unavailable`, names the adapter, and exits 2. |
| `integration-sql` | no scenario endpoint references required; `sql:` actions | Embedded full boot. An `sql:`-only document runs without `integration-http`. |

The `--no-default-features` build provides only the in-memory `fake:` partner adapter. A scenario whose endpoints are all `fake:` runs the no-boot smoke path. A `fake:`-only scenario keeps that path in any build.

The `integration-http` feature is enabled by default in `camel-cli`
since 2026-09-05. The build boots the real composition root. A scenario whose endpoints are all `fake:`, `direct:`, or `http:` qualifies. Each `http:` endpoint binds a harness partner listener on `127.0.0.1:0`. A `direct:` send stimulates the booted context through its own producer path. The document runs over the real wire.

The `integration-sql` feature is independent of `integration-http`. It is on by default in `camel-cli`. An `sql:`-only document boots the same composition root without `integration-http`. The `integration-sql` CI job proves this independence: it builds `camel-cli` with `--no-default-features --features integration-sql,itest-e2e` and runs the scenario e2e suite.

## Datasource steering
A scenario reads SQL state through the booted context's datasource catalog. The datasource itself lives in `Camel.toml`, not in the document: the `datasource:` field of a `sql:` action and of a `validate` sql target names a key from the `[datasources]` table. That table is a strict interpolation surface. Leaf values resolve `${env:NAME}` and `${env:NAME:-default}`, and a residual marker fails the load instead of passing through (see [`crates/camel-config/CONTEXT.md`](https://github.com/kennycallado/rust-camel/blob/main/crates/camel-config/CONTEXT.md)).

```toml
[datasources.appdb]
provider = "sqlx"
db_url = "${env:APPDB_URL:-sqlite:file:memdb_demo?mode=memory&cache=shared}"
```

At boot the placeholder resolves through the same layered source as the route files. The source checks harness-provisioned `bindVar` values first, then document `env:` values, then variables listed in `envPassthrough:`, then the inline default ([ADR-0069](../adr/0069-integration-tier-testing-contract.md) section 4). A hermetic document pins the value itself:

```yaml
env:
  APPDB_URL: "sqlite:file:memdb_demo?mode=memory&cache=shared"
```

The shared cache is a requirement, not a preference, for every `:memory:` datasource. A bare sqlite `:memory:` database is per-connection. An INSERT on one pooled connection and a SELECT on another can therefore hit different databases, and a validation can pass against state the document never seeded. The boot rejects `sqlite::memory:` without `cache=shared` with the `sql-memory-not-shared` error ([`crates/camel-integration-test/CONTEXT.md`](https://github.com/kennycallado/rust-camel/blob/main/crates/camel-integration-test/CONTEXT.md)).

The recipe above shows the recommended shape, the named shared-memory URI. A name such as `memdb_demo` holds one database, and every pool connection shares it, so the author selects `max_connections` for the workload. `sqlite:file:` matches no automatic datasource factory prefix, so the datasource pins `provider = "sqlx"`. The bare `sqlite::memory:?cache=shared` form is still accepted. Its shared name comes from sqlx-internal naming, and with the Any driver each pooled connection can hold a private database. Pin `max_connections = 1` with that form.

A document that needs a real database lists the variable in `envPassthrough:` and keeps the inline default:

```yaml
envPassthrough:
- APPDB_URL
```

The CI job or Compose file then supplies `APPDB_URL`, for example a service-container Postgres URL. The harness never provisions the database. The address arrives through the variable, so the surrounding infrastructure stays the author's concern (ADR-0069 section 9).

Two laws keep the surface orthogonal. The datasource name (`appdb`) is an identifier path, and interpolation never touches it. The `db_url` value is an env leaf path, and the layered source always resolves it. The same laws govern every strict-prefix table ([`crates/camel-config/CONTEXT.md`](https://github.com/kennycallado/rust-camel/blob/main/crates/camel-config/CONTEXT.md)), so this section describes one instance of a general steering pattern, not a datasource-specific rule (bd rc-l7m7t, bd rc-4hexo).

## Isolation and teardown
Each scenario boot owns its datasource catalog and its pools. The boot teardown closes those pools after the context stops. An in-memory sqlite database therefore dies with its boot: a later document booting the same `[datasources]` alias in the same process starts from an empty database. `camel test` runs documents sequentially in one process, so this per-boot freshness keeps one document's seeded rows out of the next document's validations. The guarantee is a contract of the teardown seam, not an accident of driver internals ([`crates/camel-integration-test/CONTEXT.md`](https://github.com/kennycallado/rust-camel/blob/main/crates/camel-integration-test/CONTEXT.md)).

The guarantee is load-bearing for named shared-memory URIs. A datasource pinned to `sqlite:file:<name>?mode=memory&cache=shared` shares one named database across every connection that uses the name, in any boot. A lingering connection from an earlier boot would carry that database's rows into the later boot; the teardown close is what kills it. The adversarial tests pin this shape directly.

The scenario-tier convention for memory fixtures is the named shared-memory URI, for example `sqlite:file:memdb_demo?mode=memory&cache=shared`. A named memory URI shares one database across every pool connection. The author therefore selects `max_connections` for the workload, and the `named_shared_memory_uri_probe` in the sqlx pool factory pins multi-connection sharing. The bare `sqlite::memory:?cache=shared` form is still accepted, but with the Any driver each pooled connection can hold a private database there, so that form pins `max_connections = 1`.

Durable datasources are outside the per-boot guarantee. A file-backed sqlite database, or a service-container Postgres behind `envPassthrough:`, keeps its rows across boots. No harness mechanism cleans it between documents. Isolation for durable datasources is the document author's responsibility — the same law that governs user-provided infrastructure generally (ADR-0069 section 9): the harness provisions hermetic defaults, never cleanup for resources it does not own.

The authoring convention for durable datasources is the prepare-action clean-first idiom: the first `sql:` prepare statement deletes the state a previous run may have left, before any INSERT re-seeds it.

```yaml
scenario:
- sql:
    datasource: appdb
    prepare:
    - DELETE FROM orders          # clean first: a prior document's rows
    - INSERT INTO orders VALUES ('seed-a')
```

`DELETE FROM` (whole-table) or a table-recreating statement are the two clean-first shapes; `TRUNCATE` applies where the engine supports it. A document that skips the clean-first statement works only as long as it runs alone. The adversarial boot-freshness tests in `crates/camel-integration-test` pin both directions: a second memory-sqlite boot reads zero, a second file-backed boot reads everything.

Known limitation, parallel mode (bd rc-gcf9n): `camel test` executes documents sequentially today. When parallel document execution lands, two concurrently booted documents that share one memory name could collide on the same shared memory database. The planned remedy is a per-boot unique suffix in the memory name (`memdb_{scenario}_{boot}`) minted by the harness for hermetic memory datasources. This note records a plan, not a rule for the current runner. Until parallel lands, documents that share a durable datasource must not run concurrently.

The STATE-side grammar has its own page. The `sql:` prepare action writes datasource state, and the `validate` sql target reads it: [SQL state assertions](scenario-sql.md).
