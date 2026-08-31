T1 startup-minimal fixture for the `node-fastify` contender: the same
protocol-B marker contract as `../node-native/` (see its README) with
the **Fastify 5.12.1** application (pinned exactly in `package.json`;
`package-lock.json` is committed — run `npm ci --omit=dev` before the
first run, that is the harness build step) booted in front. The
module import, `fastify()` construction, route registration, and
`await app.ready()` (the full avvio boot — plugin loading, route
compilation, handler finalization, the same framework tax every
co-contender pays before its marker) run WITHOUT binding any socket —
this scenario has no wire protocol (no-bind rule for protocol B).
After the boot, the fixture performs the same route
semantics as every contender — the one-shot timer route reduced to a
single `BENCH_ROUTE_READY` line, printed exactly once — and exits 0.
No BENCH_* env variable is read: timing/RSS are captured externally
and the marker timing IS the output. Run standalone: `npm ci` then
`node route.mjs`.
