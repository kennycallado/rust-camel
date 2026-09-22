# Design: inflightfix

## Approach

Mirror commit 101327e5 (`fix(grpc): reject consumer concurrency above
Semaphore max`, bd rc-9kgtm) onto the http consumer's inflight
semaphore. One shared fallible helper validates the bound; every seam
that can reach `tokio::sync::Semaphore::new` calls it.

Helper (placed next to `envelope_channel_capacity`, lib.rs ~1779):

```rust
pub(crate) fn max_inflight_requests_limit(
    configured: usize,
) -> Result<usize, CamelError> {
    if configured > tokio::sync::Semaphore::MAX_PERMITS {
        return Err(CamelError::Config(format!(
            "maxInflightRequests {configured} exceeds the supported \
             upper bound {} (tokio::sync::Semaphore::MAX_PERMITS)",
            tokio::sync::Semaphore::MAX_PERMITS
        )));
    }
    Ok(configured)
}
```

Unlike gRPC's `consumer_concurrency_limit`, this helper does NOT
normalize 0 to 1: http treats `maxInflightRequests=0` as a
representable reject-everything value (rc-3y6j;
`envelope_channel_capacity` already applies `.max(1)` to the channel
capacity alone, so `mpsc::channel(0)` is never constructed).

Seams (all in `crates/components/camel-http/src/lib.rs`):

1. **Parse** — `HttpServerConfig::from_components` (lib.rs:772-774):
   validate after the `.unwrap_or(1024)` default resolves, propagate
   with `?`. `from_uri_with_defaults` calls `from_components`, so it
   inherits the check.
2. **create_consumer** — `HttpEndpoint::create_consumer` (`impl
   Endpoint for HttpEndpoint`, lib.rs:2761): validate
   `self.server_config.max_inflight_requests` before constructing
   `HttpConsumer`. Catches endpoints built directly (struct literal)
   instead of via URI parse.
3. **Consumer start** — `HttpConsumer::start` (lib.rs:1807): validate
   `self.config.max_inflight_requests` before
   `ServerRegistry::global().get_or_spawn(...)` — i.e. before
   shared-server registry interaction, listener binding, envelope
   channel construction, and route registration.
4. **Defense-in-depth** — `spawn_entry` (lib.rs:1229): validate as
   the first statement, before listener bind, `local_addr`, registry
   and CancellationToken setup, so every path that reaches
   `tokio::sync::Semaphore::new` (lib.rs:1256) is guarded fail-closed
   and no side effect precedes the rejection. Both spawn paths
   (`get_or_spawn` and `get_or_spawn_with_listener`/`stage_listener`)
   funnel through `spawn_entry` (lib.rs:1160); seams 1-3 remain the
   primary gates, seam 4 is the backstop at the primitive.

The shared-server compatibility check (lib.rs:1086) only compares
values of an already-running server; with seams 1-3 in place an
oversized value can never reach it, so it needs no change.

Doc updates: `max_inflight_requests` field doc note (rc-ns3yc bound),
crate `CONTEXT.md` inflight paragraph (one sentence), `README.md`
parameter table row (upper-bound clause). Mirrors the grpc commit's
CONTEXT.md note.

## Affected crates

- `camel-component-http` (crates/components/camel-http): helper + 4
  seam validations + tests + docs. Nothing else. `CamelError::Config`
  comes from `camel-api` (already a dependency; first use of the
  variant in this crate).

## Architecture boundaries

Component-layer change only. No Runtime, DSL, or Services surface
moves; the helper is `pub(crate)`, so no public API change. Error
reporting uses the existing typed `CamelError::Config` variant — the
same idiom the grpc fix used, keeping component-error-semantics
consistent across components.

## Alternatives considered

- **Saturate instead of reject** (clamp oversized values to
  MAX_PERMITS): rejected — a silent clamp hides operator error; the
  retro520 ruling for rc-9kgtm was fail-closed typed rejection.
- **Validate only at spawn_entry** (single seam at the primitive):
  rejected — mirrors grpc's multi-seam decision; direct endpoint
  construction and early startup must fail before side effects
  (registry mutation, listener bind), not at the last instant.
- **Validate in `get_or_spawn_internal`**: rejected — `spawn_entry`
  is the single construction site of the semaphore and is shared by
  both spawn paths; validating there covers the primitive, while
  seams 1-3 give earlier, side-effect-free failures.

Single-phase change; no `## Phase N` headings (see tasks.md).
