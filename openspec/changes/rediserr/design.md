# Design: rediserr

## Approach

Classification becomes structural, powered by structure preserved at the
conversion boundaries. No `camel-api` change: the existing
`CamelError::ProcessorErrorWithSource(String, Arc<dyn Error + Send + Sync>)`
carries the source chain.

### Identity lemma (per-site, not byte-identity)

The legacy classifier OR-matches classifier words over the full
`CamelError` Display, whose text comes from three sources:

- (a) the variant prefix (`IO error:` → `io error`),
- (b) the wrap site's STATIC PROSE (`Redis HSET failed: `, `failed to
  build Redis connection info: `),
- (c) the inner `redis::RedisError` Display (General desc + `Kind` Debug,
  inner io/TLS text, or server-controlled message).

Verdict identity is proven PER SITE: a site whose static prose contains a
classifier word was ALWAYS transient (regardless of inner kind) and must
produce an always-transient structure; a site whose prose contains no
classifier word has legacy verdict = inner-text match, which the
inner-Display fallback reproduces exactly. The design deliverable is the
completed audit table (below), maintained by every boundary task.

### New module: `transport_error.rs` (camel-component-redis)

One source of truth for the boundary + classification:

1. `pub(crate) fn redis_error_to_camel(op: &str, err: redis::RedisError) -> CamelError`
   — `ProcessorErrorWithSource(format!("Redis {op} failed: {err}"), Arc::new(err))`.
   Message text byte-identical to today's wraps (log/UX unchanged); the
   redis error rides in `#[source]`. `is_transient_redis_error` re-export
   from `config` stays (`camel-redis-repo` consumes it).
2. Typed markers (each `impl std::error::Error`, attached as source):
   - `TransientRetryBudgetExhausted { stage, attempts }` — attached by
     `retry.rs::retry_budget_exhausted` (text unchanged).
   - `TransportTimeout { stage }` — attached where tokio `Elapsed` maps to
     `ProcessorError` today (component `executor.rs` connect timeout).
   - `TransientByProse { site }` — attached at audit-table sites whose
     static prose contains a classifier word (legacy always-transient).
3. `pub(crate) fn is_transient_redis_error(err: &CamelError) -> bool`
   moves its body here (re-exported from `config`), precedence:

   | # | Input shape | Verdict | Identity rationale |
   |---|---|---|---|
   | 1 | `Config(_)`, `ConfigValidation(_)` | false | legacy early-returned only `Config(_)` (rc-ezi0f); `ConfigValidation(_)` was text-sniffed (and never transient in practice — no redis/repo producer constructs it); rule 1 adds it to the early return for the same ADR-0012 boundary reason |
   | 2 | `Io(_)` | true | source (a): Display prefix `IO error:` always matched |
   | 3 | source chain has `TransientRetryBudgetExhausted`, `TransportTimeout`, or `TransientByProse` | true | source (b): prose word always matched |
   | 4 | first `redis::RedisError` in the chain, kind match: `Server(ReadOnly)`; `ClusterConnectionNotFound`; `Io` with `io::Error` source of kind `ConnectionRefused/ConnectionReset/ConnectionAborted/BrokenPipe/TimedOut` | true | source (c): each renders a classifier word today (incl. the `connection` inside the `ClusterConnectionNotFound` Debug) |
   | 5 | that `RedisError` matched NONE of rule 4's enumerated shapes | legacy substring test on THAT RedisError's Display only | documented fallback — preserves server-controlled messages (`ERR connection lost …`), TLS inner text, redis-rs static details (`SSL Handshake error`), custom io text |
   | 6 | no redis error, no marker, not `Io` (plain `ProcessorError`/`ProcessorErrorWithSource` with foreign source) | false | removes the bd's false-positive class: synthetic/foreign ProcessorError text sniffing |

   Chain walk: iterate `err.source()` (bounded depth 8); the first
   `redis::RedisError` found is THE classification input (inner-most
   wrap wins; markers may appear alongside).

### Per-site static-prose audit (delivered in Phase 1, extended per task)

| Site | Static prose | Classifier word in prose? | Treatment |
|---|---|---|---|
| commands/*.rs (93 sites) | `Redis {OP} failed: ` | no | `redis_error_to_camel` |
| executor.rs connect timeout | `Redis connection to '{}' timed out after {}s` | YES (`connection`, `timed out`) | `TransportTimeout { stage: "executor connect" }` |
| executor.rs connect err (auth-enriched) | `Failed to connect to Redis at '{url}': {e}` | no (`connect` ≠ `connection`) | preserve `RedisError` source |
| retry.rs budget | `connection lost while {stage} (retry budget exhausted…)` | YES (`connection`) | `TransientRetryBudgetExhausted { stage, attempts }` (text unchanged) |
| topology.rs connection info | `failed to build Redis connection info: {e}` | YES (`connection`) | `TransientByProse { site: "topology connection info" }` |
| topology.rs open client (×2 production + ×1 test (FakeTopology)) | `failed to open Redis client: {e}` | no | preserve `RedisError` source |
| topology.rs sentinel client (×1) | `failed to build sentinel client: {e}` | no | preserve `RedisError` source |
| topology.rs sentinel resolve (auth-enriched) | `sentinel resolve: {e}` | no | preserve `RedisError` source |
| topology.rs sentinel resolve join | `sentinel resolve join: {e}` | no | preserve `RedisError` source |
| topology.rs sentinel mutex poisoned | `sentinel mutex poisoned` | no | plain `ProcessorError` (rule 6 = legacy false) |
| topology.rs replica reads not yet supported | `replica reads not yet supported` | no | plain `ProcessorError` (rule 6 = legacy false) |
| queue.rs connect timeout | `Queue connection timed out after {}s` | YES (`connection`, `timed out`) | `TransportTimeout { stage: "queue connect" }` |
| queue.rs connect err | `Failed to create connection: {}` | YES (`connection`) | `TransientByProse { site: "queue connect" }` |
| queue.rs blpop guard | `Queue connection not established` | YES (`connection`) | `TransientByProse { site: "queue blpop guard" }` |
| queue.rs blpop passthrough | `{e}` (bare redis Display) | no | `redis_error_raw` |
| pubsub.rs connect timeout | `PubSub connection timed out after {}s` | YES (`connection`, `timed out`) | `TransportTimeout { stage: "pubsub connect" }` |
| pubsub.rs connect err | `Failed to create PubSub connection: {}` | YES (`connection`) | `TransientByProse { site: "pubsub connect" }` |
| pubsub.rs guard (×2) | `PubSub connection not established` | YES (`connection`) | `TransientByProse { site: "pubsub guard" }` |
| pubsub.rs subscribe/psubscribe wraps | `Failed to subscribe to channel/pattern {}: {}` | no | preserve `RedisError` source |
| repo get_conn remap / to_camel_error / backstop | `CamelError::Io` | source (a) | unchanged — rule 2 |
| FakeExecutor transient seam (executor.rs, test) | `Connection error: {message}` | YES (`connection`) | `TransientByProse { site: "FakeExecutor transient seam" }` |
| producer.rs:99 — NOT on classification path | `Redis health check PING failed for '{}': {}` | — | unchanged — flows to route error handling, never classified |
| health.rs:44/50/60 — NOT on classification path | `Health check connection to '{}' timed out` (+ connect/PING variants) | — | unchanged — flows to `HealthStatus::Unhealthy`, never classified |

Every row must prove `structured verdict == legacy verdict` or be listed
as a documented flip. The audit introduced no new flips; the two
documented flips are:

- Flip 1 (accepted, false→true): `io::Error` with an enumerated transient
  kind but custom text lacking classifier words (e.g.
  `io::Error::new(ConnectionRefused, "no route")`) — legacy sniffed the
  text and missed it; rule 4 classifies on the kind, which is ground truth
  for an OS connection-refusal. Pinned by
  `io_custom_text_refused_kind_is_transient`.
- Flip 2 (accepted, true→false): a plain `ProcessorError` /
  `ProcessorErrorWithSource` with foreign or synthetic text containing a
  classifier word (e.g. `ProcessorError("connection refused")`) — legacy
  sniffed the text and classified it transient; rule 6 no longer sniffs
  plain ProcessorError text. Plain ProcessorError text sniffing was the
  bd-cited false-positive class; foreign/synthetic text no longer
  classifies. Pinned by `plain_processor_error_text_is_not_sniffed`.

Sites whose legacy behavior is always-transient despite fatal inner kinds
(e.g. a TLS-feature-absent defect behind a prose word) are filed as bd
follow-ups for a future documented verdict change — NOT changed here.

### Test strategy

- Verdict-pin table in `transport_error.rs` tests: structured fixtures
  for each row of the legacy table, asserting the SAME verdict (io
  refused/reset/aborted/broken-pipe/timed-out, `READONLY` server error,
  `ClusterConnectionNotFound`, budget marker, timeout marker, prose
  marker, `Io(_)` variant, Config-with-substring → false), plus Flip 2
  asserted as the false-positive removal
  (`ProcessorError("connection refused")` → false, pinned by
  `plain_processor_error_text_is_not_sniffed`), plus fallback pins
  (server message `ERR connection lost …` → true via rule 5; rustls
  inner `peer closed connection …` → true via rule 5;
  `SSL Handshake error - Io: …` → per its detail text).
- Existing suites (`retry.rs`, `executor.rs`, `queue.rs`, `pubsub.rs`,
  `topology_tests.rs`, repo `executor.rs`/`idempotent_repo.rs`) ported to
  structured fixtures where they fed synthetic transient strings.
- FakeCommandExecutor (test seam): transient fakes construct a real
  `redis::RedisError` (io refused) + `redis_error_to_camel`.
- ADR-0012 doc comments (CONTEXT.md "load-bearing word" paragraph)
  updated: the invariant is now the typed marker, not the word.

## Affected crates

- `camel-component-redis`: new `transport_error.rs`; `config.rs`
  classification body + tests; `commands/*.rs` mechanical map_err swap;
  `executor.rs`, `retry.rs`, `queue.rs`, `pubsub.rs`, `topology.rs`
  boundary wraps; CONTEXT.md ADR-0012 paragraph.
- `camel-redis-repo`: no production change; test fixtures only if they
  assert on classification inputs.

## Architecture boundaries

Component-internal hardening; no Runtime/DSL/Services contract change.
`camel-api` untouched. Honors ADR-0012 (error-family early returns kept),
ADR-0007 (bounded reconnect → supervision unchanged — same verdicts), and
the dependency boundary ruling for camel-redis (driver types used
directly in Redis-specific modules).

## Phases

- Phase 1 — classification core: `transport_error.rs` (helpers, markers,
  classifier, verdict-pin tests), completed audit table, `config.rs`
  re-export, `retry.rs` budget marker, `executor.rs` connect wraps.
- Phase 2 — boundary sweep: `commands/*.rs` + `topology.rs` +
  `queue.rs`/`pubsub.rs` wraps + test ports across both crates; CONTEXT.md
  note.

Single review pass per task; inter-phase r_glm review between the groups.
