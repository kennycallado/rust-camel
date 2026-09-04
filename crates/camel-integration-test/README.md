# camel-integration-test

> Scenario-tier test support for rust-camel: `.test.yaml` documents that declare a `scenario:` section

Owns the scenario document model and parser behind the integration tier
of `camel test` (ADR-0069). A scenario document runs one action
vocabulary: `send`, `receive`, `sleep`, and `validate`. The parser bans
the unit-tier keys (`inputs`, `expects`, `intercepts`) at load time, and
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

The [Testing chapter](../../docs/src/testing/index.md) documents the
full action grammar, the partner adapters, and the exit contract.
`examples/integration-testing/` is a runnable example.

## Related crates

- **camel-cli**: `camel test`, which parses and runs the documents
- **camel-bundles**: the boot cascade the scenario boot composes (ADR-0069 section 10)
- **camel-test**: unit-tier harness; the scenario tier never depends on it (ADR-0055)
