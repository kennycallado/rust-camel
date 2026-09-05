# camel-integration-test

> Scenario-tier test support for rust-camel: `.test.yaml` documents that declare a `scenario:` section

Owns the scenario document model and parser behind the integration tier
of `camel test` (ADR-0069). A scenario document runs one action
vocabulary: `send`, `receive`, `sleep`, and `validate`. A `send` takes
an optional `method` HTTP token, uppercased at load. An invalid token
fails doc validation with exit 2. A `send` with a body defaults to
`POST`. A bodyless `send` defaults to `GET`. The parser bans the
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
with the CLI; the `http:` partner adapter rides the non-default
`integration-http` feature (enabled by default in `camel-cli` since
2026-09-05; the explicit flag remains valid):

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
`response` holds optional `status`, `headers`, and `body`. The
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

## `validate` partner target

A `validate` action with a `partner` target asserts the recorded-request
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

## `${name}` interpolation

Scenario strings interpolate `${name}`. The surface covers three
places:

- endpoint strings in `send` and `receive`,
- body string leaves,
- header values.

Substitution is string-only. A string leaf with no placeholder stays
as is. Raw substitution applies, with no percent-encoding. An unset
variable at send time fails `scenario-var-unresolved`, exit 1, naming
the variable. Exit 1 is a verdict failure, not a parse error. In CI, a
document that fails this way is a failed test run, not a harness
error.

`$${` escapes a literal `${`. The escape applies to body leaves and
header values too. For example, a JSON body leaf that must reach the
wire as `${literal}` is written `$${literal}`.

The `receive` resolves by the interpolated authority. The path and
query need not match the `send`. A receive declared as
`http://${PARTNER}/orders` finds the roundtrip a map-form send parked.

## Two layers, one name: `PARTNER`

The same name can carry two forms in one run.

| Layer | Form | Usage |
|-------|------|-------|
| scenario variable | `host:port` | `http://${PARTNER}/orders` |
| route env | `http://host:port` | `${env:PARTNER}` |

One-line rule: scenario = authority, route env = full URI. `${env:}`
deliberately does not resolve in scenario strings.

The [Testing chapter](../../docs/src/testing/index.md) documents the
full action grammar, the partner adapters, and the exit contract.
`examples/integration-testing/` is a runnable example. The
`partner-retry-route.test.yaml` pair there runs a real retrying route
against a faulted partner and asserts the two wire attempts.

## Related crates

- **camel-cli**: `camel test`, which parses and runs the documents
- **camel-bundles**: the boot cascade the scenario boot composes (ADR-0069 section 10)
- **camel-test**: unit-tier harness; the scenario tier never depends on it (ADR-0055)
