# Proposal: inflightfix

## Why

`camel-component-http` parses `maxInflightRequests` from the consumer URI
(`HttpServerConfig::from_components`, lib.rs:772-774) and passes it
unbounded to `tokio::sync::Semaphore::new` in `spawn_entry`
(lib.rs:1256). Values above `tokio::sync::Semaphore::MAX_PERMITS`
(2^61-1 on 64-bit targets) pass config parse and PANIC at semaphore
construction during route startup instead of returning a typed
configuration error.

This is the same blind-spot class that retro520 flagged ("upper
bounds: values valid to parse but panic downstream") and that bd
rc-9kgtm fixed for the gRPC `consumerConcurrency` parameter
(commit 101327e5). bd rc-ns3yc tracks this instance.

## What Changes

Mirror the rc-9kgtm pattern in `camel-component-http`:

- Add one shared fallible bound helper
  (`max_inflight_requests_limit`) that rejects values above
  `tokio::sync::Semaphore::MAX_PERMITS` with a typed
  `CamelError::Config` naming the parameter, the configured value, and
  the limit.
- Apply the helper at the validation seams:
  1. URI parse (`HttpServerConfig::from_components`;
     `from_uri_with_defaults` inherits),
  2. `create_consumer` (catches directly constructed endpoints),
  3. `HttpConsumer::start` (before shared-server registry interaction
     and listener binding),
  4. defense-in-depth in `spawn_entry` immediately before
     `Semaphore::new`.
- Boundary tests (L-1 / L / L+1), red-first where the current code
  panics or accepts.
- Doc notes: `max_inflight_requests` field doc, crate `CONTEXT.md`
  inflight section, `README.md` parameter table row.

**Divergence from the gRPC fix (intentional):** `maxInflightRequests=0`
stays accepted — it is a representable "reject everything with 503"
semantic (rc-3y6j). The helper enforces the upper bound only; it does
NOT normalize 0 (gRPC normalizes 0 to 1; http must not).

**Excluded:** non-numeric `maxInflightRequests` values still silently
fall back to the default (1024) — that lenient-parse behavior is
pre-existing, out of scope, and unchanged.

## Acceptance criteria

- No representable `maxInflightRequests` value panics route startup;
  values above `Semaphore::MAX_PERMITS` return a typed
  `CamelError::Config` naming `maxInflightRequests`, the value, and
  the limit, at all four seams.
- L-1 and L pass through unchanged at parse; L+1 is rejected at all
  validation seams.
- `maxInflightRequests=0` behavior is unchanged (503 reject-everything
  test still passes).
- `cargo fmt --check`, `cargo clippy -p camel-component-http -- -D
  warnings`, and the crate's affected tests pass.

## Risk budget

Low. Behavior change is limited to oversized values that previously
panicked (panic -> typed error; strictly better). No public API
change; the helper is `pub(crate)`. Zero-value and default semantics
untouched.

Bd: rc-ns3yc (discovered-from rc-9kgtm)
