# Proposal: redis-live-tls-and-ready-race

## Why

Two production gaps in the Redis area, filed as rc-lmx9 and rc-3ckqr:

1. **Zero live `rediss://` coverage.** The TLS tri-state work (36847c28, bd
   rc-ayy11) added `tls_mode`, `validate_tls`, and shipped `redis-tls` in
   release images — but no test anywhere drives a real TLS listener.
   Worse, `RedisConfig::tls_ca_cert` (config.rs) is dead config: it is
   parsed and redacted but never reaches the client builder, so
   self-signed-CA deployments (the common private-Redis case) cannot
   connect at all. A live test is impossible without wiring it first.
2. **PubSub consumer ready race.** `run_pubsub_consumer` calls
   `ctx.mark_ready()` (consumer.rs:283) before the session completes
   connect+SUBSCRIBE server-side. Pub/Sub has no replay, so a PUBLISH
   landing in that window is lost. This breaks the camel-test harness
   contract "start() returns only when consumers are ready"
   (crates/camel-test/CONTEXT.md) and hits camel-cache's cross-replica
   cache-purge pattern directly. The rc-8kha fix masked the race at test
   level with a PUBSUB CHANNELS barrier; the component bug remains.

## What Changes

- **Prod, TLS**: wire `tls_ca_cert` into `StandaloneTopology::resolve`
  via `Client::build_with_tls` + `TlsCertificates { root_cert }`
  (redis-rs, `tls-rustls` feature). No insecure flag on this path.
- **Prod, race**: move pubsub `mark_ready` to after the first successful
  `subscribe_all`; fire-once ready hook threaded through
  `pubsub_session`. Queue consumer (BLPOP) path unchanged — no loss
  window exists there.
- **Tests**: new `crates/camel-test/tests/redis_tls_test.rs`
  (rediss:// round-trip through the repo path + plaintext negative
  control, rcgen-generated CA at test time, redis:7-alpine
  `--tls-port`/`--port 0`); remove the rc-8kha test-side barrier and
  prove immediate-publish delivery; restore subscriber-side assertion
  in `redis_pubsub_producer` (e_glm review note, rc-3ckqr).
- **Feature plumbing**: `camel-test` `integration-tests` enables the
  redis TLS client feature needed by the repo path.
- **Docs**: `docs/src/components/redis.md` + camel-redis CONTEXT.md
  document `tls_ca_cert` trust behavior.

**Excluded**: sentinel TLS live topology (follow-up bd rc-hbde6 — the
CA trust path is standalone-only in this change), queue-consumer
`mark_ready` relocation (no loss window; separate lifecycle concern).

## Acceptance criteria

- A live TLS Redis container is exercised end-to-end: put/get/delete
  through `rediss://` via `camel-redis-repo` succeeds with a
  self-signed CA accepted through `tls_ca_cert` — proving handshake +
  full command path, not just connect.
- Negative control: plaintext `redis://` against the TLS-only port
  fails with a TLS/protocol error (and `rediss://` against a plaintext
  port fails), proving TLS is real.
- Pre-fix red evidence for the ready race (immediate publish lost),
  post-fix green: readiness fires after subscribe-ack; ≥20 consecutive
  stress runs of the un-barriered pubsub test, no sleeps/retries.
- Harness start() contract holds: unreachable Redis still fails fast
  (bounded startup budget; errors classified per the component retry
  taxonomy, ADR-0007).
- `cargo fmt --check`, clippy `-D warnings` on touched crates, and the
  camel-redis + camel-test redis suites pass.

## Risk budget

Acceptable: small prod surface in topology.rs (additive branch when CA
configured); consumer ready-timing change bounded by the existing 90s
startup budget. Out of bounds: `insecure: true` trust bypass on the CA
path, sleeps/retry-masking in tests, sentinel TLS scope creep, changes
to queue-consumer startup ordering.

Bd: rc-lmx9, rc-3ckqr.
