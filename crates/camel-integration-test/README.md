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
`integration-http` feature:

```sh
cargo run -p camel-cli --features integration-http -- test --integration orders.test.yaml
```

## `partners:` section

A `partners:` section scripts the responses a harness partner serves.
It is a map. Each key is the exact declared endpoint string, the `:0`
URI as written in the scenario. The value is a sequence of script
entries.

Each entry carries optional `method` and `path` matchers plus a
`response`. A request matches an entry when its method and path match.
The `response` holds optional `status`, `headers`, and `body`. An entry
with no `method` or `path` matches any request. The harness serves the
first matching entry in order. A request no entry matches serves status
500 with an empty body. A document with no `partners:` section is
permissive: every request gets status 200 with an empty body.

Every `partners:` key must equal a declared harness `http` endpoint
reference. A key that matches no wired reference fails load with a
`doc-validation` error, exit 2, naming the key. The check runs before
any partner binds. A typo of a real key, for example `http://127.0.0.1:0/order` for
`:0/orders`, fails here. It never falls silently to permissive.

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
`examples/integration-testing/` is a runnable example.

## Related crates

- **camel-cli**: `camel test`, which parses and runs the documents
- **camel-bundles**: the boot cascade the scenario boot composes (ADR-0069 section 10)
- **camel-test**: unit-tier harness; the scenario tier never depends on it (ADR-0055)
