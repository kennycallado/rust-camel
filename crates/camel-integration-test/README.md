# camel-integration-test

> Scenario-tier test support for rust-camel: `.test.yaml` documents that declare a `scenario:` section

Owns the scenario document model and parser behind the integration tier
of `camel test` (ADR-0069). A scenario document runs one action
vocabulary: `send`, `receive`, `sleep`, and `validate`. A `send` takes
an optional `method` HTTP token, uppercased at load. An invalid token
fails doc validation with exit 2. A `send` with a body defaults to
`POST`. A bodyless `send` defaults to `GET`. A document may declare a
top-level `sendDeadline` field. It is a humantime duration that bounds
every `send` action. It defaults to 30 s. An invalid value fails load
with a `doc-validation` error naming `sendDeadline`. The bound uses
real time only; there is no virtual time. A `send` to a `direct:` route
may declare `expectReply`. The matcher verbs are the same as `validate`:
`equals`, `regex`, `contains`, `startsWith`, `endsWith`, `exists`,
`jsonSubset`. It asserts the synchronous reply body. An `expectReply`
mismatch is a verdict-class failure naming the expectation and the
actual body. `expectReply` on any other target, an `http:` partner or
`fake:`, is a load-time error. Runnable reference:
`tests/direct_reply_test.rs`. The
parser bans the
unit-tier keys (`inputs`, `expects`, `intercepts`) at load time, and
it rejects `env` keys that collide with a declared `bindVar`. The
harness provisions each `http:` partner on `127.0.0.1:0` and folds the
partner's `bindVar` into a layered environment. Config and route
`${env:}` placeholders resolve through that environment, never the
process environment. Every action prints a PASS or FAIL row. Exit codes
follow the ADR-0069 taxonomy: 0 when all actions pass, 1 on a verdict
failure, 2 on a parse, boot, or apparatus failure.

## Usage

A minimal scenario document (`orders.test.yaml`):

```yaml
routeFiles:
  - routes/bridge.yaml
scenario:
  - send:
      to: direct:start
      method: PUT
      body: order-payload-7f3a
  - receive:
      from:
        endpoint: http://127.0.0.1:0/orders
        provisioning: harness
        bindVar: PARTNER
      deadline: 2s
      extract:
        body: body
  - validate:
      target: { lastReceived: http://127.0.0.1:0/orders }
      expectation: order-payload-7f3a
```

The partner endpoint binds on the harness and injects
`PARTNER=http://127.0.0.1:<bound>` into the layered environment, so the
route's `${env:PARTNER}` reaches the local listener. Run the document
with the CLI; the `http:` partner adapter rides the `integration-http`
Cargo feature, default-on in `camel-cli` since 2026-09-05:

```sh
cargo run -p camel-cli -- test --integration orders.test.yaml
```

## `partners:` section

A `partners:` section scripts how a harness partner answers requests.
It is a map. Each key is the exact declared endpoint string, the `:0`
URI as written in the scenario. The value is a sequence of script
entries.

Each entry carries optional `method` and `path` matchers. A request
matches an entry when its method and path match. An entry with no
`method` or `path` matches any request.

An entry declares exactly one of `response` or `fault: close`. The
`response` holds optional `status` (100-599), `headers`, and `body`. The
`fault: close` drops the connection without an HTTP response. The
request is still recorded before the fault.

An entry may also declare `times` and `delay`. The `times` is an
integer of 1 or more. It defaults to 1. An entry with no `times` keeps
the shipped serve-once behavior. The entry serves its first `times`
matching requests, then it is spent. The `delay` is a humantime
string. The harness holds for the delay before it serves the response
or commits the fault.

The harness serves the first unspent matching entry in order. A
request no entry matches serves status 500 with an empty body. A
document with no `partners:` section is permissive: every request
gets status 200 with an empty body.

For example, a timed delayed response entry can precede a fault entry
for the same request:

```yaml
partners:
  http://127.0.0.1:0/orders:
  - method: PUT
    path: /orders
    times: 2
    delay: 100ms
    response:
      status: 201
      body: A
  - method: PUT
    path: /orders
    fault: close
```

The first entry serves the first two matching requests. Each response
arrives after the delay with status 201 and body `A`. The entry is
then spent. The third matching request hits the fault entry. Its
connection drops without an HTTP response.

Every `partners:` key must equal a declared harness `http` endpoint
reference. A key that matches no wired reference fails load with a
`doc-validation` error, exit 2, naming the key. The check runs before
any partner binds. A typo of a real key, for example `http://127.0.0.1:0/order` for
`:0/orders`, fails here. It never falls silently to permissive.

## `validate` targets

A `validate` action asserts against one of four targets, chosen by the
single key of its `target` map:

- `lastReceived`: the last message a `receive` action collected on that
  endpoint. The expectation is a subset match, as in the usage example
  above.
- `variable`: a scenario variable set by an earlier `extract`. Variable
  existence is checked at run time. The expectation is a subset match
  against the variable's value.
- `partner`: the recorded-request count of a harness partner, described
  next.
- `sql`: datasource rows at rest, described under [SQL actions](#sql-actions).

A `partner` target asserts the recorded-request
count of a harness partner. The expectation holds `count`, an exact
non-negative integer, plus optional `method` and `path` filters. The
`method` filter compares ASCII-case-insensitively. The `path` filter
compares the path-and-query exactly. The count counts only the recorded
requests that pass both filters.

Without a `deadline`, the assertion reads one immediate snapshot. With a
`deadline`, it polls a fresh snapshot every 100 ms until the filtered
count equals the expectation or the deadline passes; the final snapshot
then decides. Arrivals only add, so a count above the expectation fails
at every snapshot and never settles back.

Partner expectations are exact-count, not subset like message
expectations: the count must equal the filtered arrivals, never a lower
bound.

## SQL actions

An `sql:` action prepares datasource state before the assertions:

```yaml
scenario:
- sql:
    datasource: appdb
    prepare:
    - DELETE FROM orders
    - INSERT INTO orders VALUES ('seed-a')
```

`datasource` names a key under `[datasources]` in `Camel.toml`.
`prepare` is an ordered list of non-SELECT statements, run in order over
the datasource's pool. An empty list, or a statement with a
`select`/`with` prefix, fails doc validation: reads belong to the
`validate` sql target.

That target pairs the prepare action with a read assertion:

```yaml
- validate:
    target: {sql: {datasource: appdb, query: SELECT id FROM orders}}
    expectation: {rows: [[1]]}
    deadline: 5s
```

The expectation holds row patterns (`rows`, with optional `columns` and
`unordered`) or a row-count bound (`count`, `atLeast`, `atMost`). The
`deadline` is valid on `partner` and `sql` targets only. The full
surface — row grammar, cell verbs, settle semantics, and the sqlite
shared-memory rule — is documented in the book's
[SQL state assertions](../../docs/src/testing/scenario-sql.md) page.

## Concurrency: the burst-send recipe

Back-to-back `send:` actions with no intervening `receive:` dispatch
genuinely concurrent requests. Each send returns at connect; the request
writes land asynchronously. The partner recorder is the proof surface: a
`validate` with a `partner` target counts the wire arrivals.

The recipe sends a burst to one lane key, settles, then asserts the
count. It mirrors the `IMMEDIATE_COUNT_DOC` shape:

```yaml
scenario:
- send:
    to:
      endpoint: http://127.0.0.1:0/orders
      provisioning: harness
    method: POST
- send:
    to:
      endpoint: http://127.0.0.1:0/orders
      provisioning: harness
    method: POST
- send:
    to:
      endpoint: http://127.0.0.1:0/orders
      provisioning: harness
    method: POST
- sleep:
    duration: 100ms
- validate:
    target: {partner: http://127.0.0.1:0/orders}
    expectation: {count: 3}
```

The three sends park three responses on the same lane key. The sleep
lets the writes land. The validate asserts the exact count of three.

Runnable references: `tests/partner_verification_test.rs::immediate_count_assert_e2e`
and `tests/http_partner_scripting_test.rs`.

The v1 bound: same-key responses park in a bounded FIFO of 64. A burst
past the bound fails apparatus-class, naming the lane key. The recipe
has no native wall-clock concurrency-comparison primitive; that is a
future consideration.

## `${name}` interpolation

Scenario strings interpolate `${name}`. The surface covers three
places:

- endpoint strings in `send` and `receive`,
- body string leaves,
- header values.

Substitution is string-only. A string leaf with no placeholder stays
as is. Raw substitution applies, with no percent-encoding. An unset
variable at send time fails `scenario-var-unresolved`, exit 2, naming
the variable. Exit 2 is the apparatus class: an unset variable is an
authoring bug, so CI reports a harness/document error, not a product
failure.

`$${` escapes a literal `${`. The escape applies to body leaves and
header values too. For example, a JSON body leaf that must reach the
wire as `${literal}` is written `$${literal}`.

The `receive` resolves by the interpolated authority. The path and
query need not match the `send`. A receive declared as
`http://${PARTNER}/orders` finds the roundtrip a map-form send parked.

## Minimal route source

An integration document may declare `routeFiles: []`. The empty list is
the minimal route source: no routes boot, and the scenario drives only
harness partners.

## Two layers, one name: `PARTNER`

The same name can carry two forms in one run.

| Layer | Form | Usage |
|-------|------|-------|
| scenario variable | `host:port` | `http://${PARTNER}/orders` |
| route env | `http://host:port` | `${env:PARTNER}` |

One-line rule: scenario = authority, route env = full URI. `${env:}`
deliberately does not resolve in scenario strings.

The multi-path pattern builds on this rule: one declared partner and
one `bindVar` serve every path a route dials on that authority
(runnable pair: `examples/integration-testing/partner-multi-path.test.yaml`
+ `partner-multi-path.routes.yaml`).

## `inbound:` section

A document may declare `inbound: {bindVar: NAME}`. The harness
provisions a staged port-0 listener, and route files interpolate the
full-URL variable through `${env:NAME}`, for example `from:
${env:NAME}/in`. The bound address is exposed on the run outcome as
`inbound_bound`. Documents that pin a literal port keep working — the
back-compat shape that predates `inbound:`. v1 bound: boot-owning
library callers only; the CLI cannot run inbound documents (named
infra-unavailable). Runnable reference: `tests/http_inbound_test.rs`.

The [Testing chapter](../../docs/src/testing/index.md) documents the
full action grammar, the partner adapters, and the exit contract.
`examples/integration-testing/` is a runnable example. The
`partner-retry-route.test.yaml` pair there runs a real retrying route
against a faulted partner and asserts the two wire attempts.

## Capability matrix: what the tier can drive today

The tier is black-box at the HTTP boundary (rc-xnob) and asserts SQL
state at rest ([SQL actions](#sql-actions)). Route shapes and their
coverage:

| route shape | trigger today | effect observable today |
|---|---|---|
| `from:http` -> `to:http` | YES scenario client role | YES partner + `receive` (v1 flagship) |
| `from:http` -> `to:sql` | YES scenario | YES `validate` sql target |
| `from:http` -> `to:kafka` | YES scenario | NO consumer/probe exists |
| `from:sql` -> `to:http` | YES `sql:` prepare (seeds rows) | YES partner |
| `from:kafka` -> `to:http` | NO producer exists | YES partner (matchers + `lastReceived` already free) |

The remaining gap is kafka on both sides: a partner-as-producer unlocks
the `from:kafka` trigger, and a kafka consumer partner unlocks the
`to:kafka` effect. The extension points already exist in the contract —
the `Provisioning` enum for partner transports and the scenario action
enum for a future probe action (`query` -> `extract` -> `validate`
reuses the existing variables and matchers). Open design questions for
that horizon: partner lifecycle for long-lived transports (topics vs
listeners) and CI infrastructure (kafka container).

## Related crates

- **camel-cli**: `camel test`, which parses and runs the documents
- **camel-bundles**: the boot cascade the scenario boot composes (ADR-0069 section 10)
- **camel-test**: unit-tier harness; the scenario tier never depends on it (ADR-0055)
