# Design: redis-live-tls-and-ready-race

## Approach

Two independent production fixes, sequenced so the cheap deterministic
one lands first and the TLS one builds on a stable base.

**(A) PubSub ready race (rc-3ckqr).** Today `run_pubsub_consumer`
(consumer.rs) marks the consumer ready, then enters the tokio::select
that drives `pubsub_session` → `connect_and_subscribe` (pubsub.rs):
resolve → connect → `subscribe_all`. The ready signal therefore fires
before the server has registered SUBSCRIBE, and Pub/Sub has no replay.
Fix: delete the eager `ctx.mark_ready()` and thread a fire-once ready
hook into `pubsub_session`; the session invokes it exactly once, right
after the FIRST `subscribe_all` returns Ok. Reconnect replays
re-subscribe but must not re-signal. Lifecycle stays safe: consumers
are `Explicit`, `await_consumer_startup` is bounded by the 90s
  `CONSUMER_STARTUP_BUDGET`, and reconnect-budget exhaustion returns Err
  before that — unreachable Redis keeps startup bounded and correctly
  failing (component retry taxonomy `is_transient_redis_error` +
  ADR-0007 route-supervised failure; no retry-loop masking). Latency
  note: with the default retry policy (10 attempts / up-to-30s delays,
  network_retry.rs) exhaustion may only be reached at the 90s budget
  ceiling rather than quickly; that is correct Explicit semantics and
  the spec's unreachable scenario configures a short policy to assert
  the fast path. A possible refinement (out of scope here): wire
  `ctx.mark_failed(e)` on pre-ready session Err so start() surfaces
  the real Redis error instead of a dropped-startup-signal error. The
  queue consumer (BLPOP, consumer.rs
second `mark_ready`) has no loss window — items persist in the list —
and is out of scope.

**(B) TLS CA wiring + live coverage (rc-lmx9).** `tls_ca_cert` is
parsed config that never reaches the client: `RedisEndpointConfig`
(config.rs) has no `tls_ca_cert` field at all — the global
`RedisConfig::tls_ca_cert` (config.rs:258) is a FILE PATH that stops
at the global layer — and `StandaloneTopology` hardcodes `TcpTls {
insecure: false, tls_params: None }` with `resolve()` using
`Client::open` (webpki roots only). Fix, in order: (1) propagate the
CA path onto the endpoint config through `apply_defaults`; (2) at
topology construction, read the file to PEM bytes — read failure is a
fail-closed `Config` error whose message may name the path (paths are
metadata, ADR-0051) but never file contents; (3) when the endpoint
resolved TLS and CA PEM is present, `resolve()` branches to
`Client::build_with_tls(conn_info, TlsCertificates { root_cert:
Some(pem) })` (redis-rs `tls-rustls`). The `rediss://` scheme required
by `build_with_tls` is already what `build_url` emits under TLS. No
insecure bypass on this path — the negative control depends on real
verification. CA trust is standalone-only in this change; the sentinel
surface is follow-up bd rc-hbde6. Live coverage: `support/redis.rs`
gains a TLS topology — rcgen-generated self-signed CA + server cert
(SAN 127.0.0.1), written into a `redis:7-alpine` container started
with `--tls-port 6379 --port 0` (plaintext disabled so the negative
control is meaningful), gated `#![cfg(feature = "integration-tests")]`,
never `#[ignore]` (ADR-0054). `redis_tls_test.rs` drives the
camel-redis-repo connection path (`tls_ca_cert` = generated CA) with
put/get/delete, plus the negative controls: plaintext `redis://`
against the TLS port fails, `rediss://` against a plaintext port
fails, and a client trusting a DIFFERENT CA is rejected (wrong-CA
control). CA PEM is metadata, not credential bytes (ADR-0051 scope) —
server key stays in the container fs/tmpdir. Feature plumbing:
`camel-test`'s `integration-tests` feature enables
`camel-redis-repo/tls` (the repo path must build a TLS client under
the integration feature).

## Affected crates

- `camel-component-redis` (as `camel-redis`): consumer.rs ready
  ordering; pubsub.rs ready hook; topology.rs CA branch + unit test;
  CONTEXT.md TLS trust section.
- `camel-test`: support/redis.rs TLS fixture; new redis_tls_test.rs;
  redis_test.rs barrier removal + producer assertion; Cargo.toml
  `integration-tests` feature enables the repo TLS client feature.
- `docs/src/components/redis.md`: `tls_ca_cert` documentation.

## Architecture boundaries

Component layer only (`camel-component-redis`) + test workspace
(`camel-test`). No Runtime/DSL/Services API changes; the repo path is
exercised, not modified. Data/control plane split untouched — the ready
hook is control-plane lifecycle signaling inside the component's own
consumer task, mirroring the DirectConsumer "ready after registry
insert" precedent (direct-startup-handshake).

## Phases

### Phase 1: PubSub ready race — red to green
- **Goal:** readiness fires after server-side SUBSCRIBE; race proven
  red first, then fixed and stress-verified.
- **Dependencies:** none (self-contained, unit-testable without
  Docker).
- **Externally-visible types/interfaces:** none (internal hook in
  pubsub session seam).
- **Deliverable:** consumer.rs/pubsub.rs fix + unit test + un-barriered
  camel-test pubsub tests + restored producer subscriber-side
  assertion.
- **Exit-criteria:** deterministic unit test asserts ready-after-
  subscribe (and not-before, via blocked fake I/O); immediate-publish-
  after-start test green ≥20 consecutive runs; unreachable-broker test
  proves `harness.start()` fails fast (short retry policy, outer
  timeout, panic-through-expect observable at harness.rs:271-273) while
  the reconnect-exhaustion error still classifies as retryable network
  failure; redis suites green; fmt/clippy clean.

### Phase 2: Standalone TLS CA wiring
- **Goal:** `tls_ca_cert` reaches the client builder; self-signed CAs
  connect without insecure bypass.
- **Dependencies:** none (no code coupling to Phase 1; implementation
  order retained for a stable base only).
- **Externally-visible types/interfaces:** `RedisEndpointConfig` gains
  the public `tls_ca_cert: Option<String>` field — a public-struct API
  addition that breaks exhaustive struct literals (camel-redis and
  camel-redis-repo sites updated in-task; no builder/serde surface
  change beyond the field). `StandaloneTopology`'s public constructor
  arity is PRESERVED (the CA-aware path is an internal
  `pub(crate) new_with_ca`), so no other public signatures change.
- **Deliverable:** topology.rs `build_with_tls` branch + unit test
  (builder selection, no network) + docs.
- **Exit-criteria:** unit test proves CA-configured endpoints take the
  CA branch and CA-absent endpoints keep `Client::open`; validate_tls
  still gates feature-absent builds; fmt/clippy clean.

### Phase 3: rediss:// live coverage
- **Goal:** a real TLS listener exercised end-to-end through the repo
  path.
- **Dependencies:** Phase 2 (CA wiring is the precondition — dead
  config cannot be tested live).
- **Externally-visible types/interfaces:** new test support fixture
  (`shared_redis_tls`), new test file.
- **Deliverable:** support/redis.rs TLS topology + redis_tls_test.rs
  (round-trip + negative controls) + camel-test feature plumbing.
- **Exit-criteria:** live suite green locally under Docker; `rg
  rediss://` shows live-test surfaces; negative control fails with
  TLS/protocol error; no `#[ignore]`.

## Alternatives considered

- **stunnel sidecar / in-test rustls proxy** for TLS: extra moving
  parts, no precedent; rejected in favor of redis native `--tls-port`.
- **`insecure: true` for the live test**: defeats the negative control
  and the security posture; rejected.
- **Keep the rc-8kha test barrier** alongside the component fix:
  anti-masking discipline says remove it — its removal is the
  end-to-end proof that readiness is now trustworthy.
- **Move queue-consumer `mark_ready` too**: no loss window (BLPOP
  persists); deferred to avoid unnecessary startup-timing risk.
- **Committed PEM fixtures**: violates no-committed-secrets posture;
  rcgen at test time matches existing dev-deps.
