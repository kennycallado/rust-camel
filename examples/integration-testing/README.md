# integration-testing: a scenario-tier integration test with `camel test`

A runnable example of the integration-tier testing contract (ADR-0069).
`orders.test.yaml` is a `scenario:` document: it stimulates the booted
route at `direct:start`, receives the request that arrives at the
harness-provisioned HTTP partner, and validates the wire arrival. The
assertions run on the partner side, so a PASS is wire proof, not a mock
expectation: method, path, route-stamped headers, and body must match
what the booted route actually sent.

Files:

- `Camel.toml`: project config. `allow_internal` opts the http producer
  past the SSRF guard for the loopback partner.
- `routes/bridge.yaml`: the route under test, `direct:start` to
  `${env:PARTNER}/orders`.
- `orders.test.yaml`: the scenario document. The partner endpoint
  declares `provisioning: harness` and `bindVar: PARTNER`; the harness
  binds it on `127.0.0.1:0` and injects `PARTNER` into the layered
  environment.

## Build and run

```bash
# from the workspace root
cargo build -p camel-cli --features integration-http

cd examples/integration-testing
../../target/debug/camel test orders.test.yaml
```

The `integration-http` feature is non-default. It supplies the harness
HTTP partner adapter. Without it, the run names the missing adapter on
stderr (`infra-unavailable`) and exits 2. `--integration` is the tier
filter; a single named scenario document does not need it.

## What to expect

The run boots the real composition root, prints one tier line, one PASS
row per scenario action, and the summary:

```
orders.test.yaml [full]
PASS orders.test.yaml#scenario[0] send
PASS orders.test.yaml#scenario[1] receive
PASS orders.test.yaml#scenario[2] validate
PASS orders.test.yaml#scenario[3] validate
PASS orders.test.yaml#scenario[4] validate
PASS orders.test.yaml#scenario[5] validate
PASS orders.test.yaml#scenario[6] validate
7 passed, 0 failed
```

The full exit contract: 0 when all actions pass, 1 on a verdict failure
(receive timeout, validation mismatch), 2 on a parse, boot, or apparatus
failure.

## Inbound method example

`inbound-put.test.yaml` shows the other direction: the scenario is the
client, and an inbound route is the oracle. The `send` performs a real
`method: PUT` into the booted consumer route, and the `receive` awaits
the route's response (client role: the response parked by the send).
The route registers PUT /orders only (`httpMethod=PUT` in
`routes/inbound-orders.yaml`), so the response body `put-accepted`
proves the wire method. A legacy GET misses the endpoint, and the
status validation fails on the unmatched 404.

```bash
cd examples/integration-testing
../../target/debug/camel test inbound-put.test.yaml
```

The route pins loopback port 18097 in the route URI: no bound-address
API exists in v1, so a CI job that shares loopback with other jobs can
collide on that port.
