T2 t2-json fixture for the `node-fastify` contender: the same
protocol-B contract as `../node-native/` (see its README for the full
extraction — env contract, canonical body builder, transform chain,
SIZE+13 output assert, golden `BENCH_INPUT_SHA256` INPUT parity) with
the **Fastify 5.12.1** application (pinned exactly in `package.json`;
`package-lock.json` is committed — run `npm ci --omit=dev` before the
first run, that is the harness build step) booted in front. The module
import, `fastify()` construction, route registration, and
`await app.ready()` (the full avvio boot — plugin loading, route
compilation, handler finalization, the same framework tax every
co-contender pays before its marker) run WITHOUT binding any socket —
this scenario has no wire protocol (no-bind rule for protocol B), and
the registered route is never served. After the boot, the fixture
performs the same single route execution (`repeatCount=1` semantics):
SHA log → unmarshal → filter → transform → marshal → output assert →
marker `BENCH_ROUTE_READY bytes=<len>` exactly once, then idles until
the smoke/harness kills it. OUTPUT byte-parity across runtimes is not
claimed (serializer member-order caveat in the scenario README). Run
standalone: `npm ci` then `BENCH_PAYLOAD_BYTES=32768 node route.mjs`.
