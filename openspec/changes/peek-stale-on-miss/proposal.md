# Proposal: peek-stale-on-miss

## Why

Production report (bd rc-7zfu, external deployment on v0.28.0): `cache_peek_stale`
on a key absent from the repository terminates the whole route. The HTTP consumer
answers immediately with the current status/body (200 + empty body), no error, no
log. Steps after the peek never run. This makes the stale-while-revalidate (SWR)
pattern inexpressible in YAML.

The MISS-stop behavior is spec'd (`openspec/specs/eip-cache`, "Cache EIP face"
requirement), but the spec rationale only considered the `CircuitBreaker.fallback`
context, where absence legitimately means "no stale available". At route top level
the same semantics produce a silent truncated response. Additionally, HIT vs MISS
is indistinguishable from YAML: no property or header exposes the peek result, so
users branch on `body.len() > 0` — a fragile heuristic (a legitimately empty
cached body reads as MISS).

The step currently overloads two roles: data access (HIT: replace body, continue)
and control flow (MISS: stop branch). Branching belongs to `choice`/`filter`, not
to a cache lookup.

## What Changes

1. `cache_peek_stale` gains an optional `on_miss:` knob with values `stop`
   (default — preserves current behavior and the CircuitBreaker.fallback
   composition) and `continue` (MISS leaves the body untouched and the pipeline
   continues).
2. The step sets exchange properties on every completed evaluation:
   `CamelCachePeekHit` (true on HIT, false on MISS) and `CamelCachePeekStale`
   (true when the served entry is past its `expires_at`), so `choice` can branch
   explicitly. This is the framework-level answer to HIT/MISS discrimination.
3. The key-expression-`None` arm keeps `Stopped` (an anomalous key resolution is
   not a miss; fail closed) but gains a `debug!` log, as does the MISS-stop arm
   (absorbs bd rc-uow1: route-terminating control flow from a data step must be
   observable). Logs name the step and repository only — raw keys are not logged
   (key expressions may resolve credential-bearing exchange data).
4. User-facing docs per CONTEXT-MAP.md: `docs/src/eip/cache.md`,
   `docs/src/yaml-dsl/step-verbs.md`, and an anchored stale-while-revalidate
   example in `examples/cache-example/routes.yaml`.

## Acceptance Criteria

- A SWR route is expressible: `cache_peek_stale` with `on_miss: continue` on an
  absent key reaches subsequent steps with the body unchanged and
  `CamelCachePeekHit=false`.
- Existing fallback scenario (absence Stops the branch) passes unchanged under
  the default.
- HIT sets `CamelCachePeekHit=true` and `CamelCachePeekStale` matching expiry
  state; body behavior unchanged.
- MISS-triggered stop emits one `debug!` record naming step and repository.
- Spec delta MODIFIED on `eip-cache` merged; processor CONTEXT.md and user docs
  (`docs/src/eip/cache.md`, `docs/src/yaml-dsl/step-verbs.md`, cache example)
  updated.

## Risk Budget

Low. Default `stop` is outcome- and body-compatible with today for existing
routes, including the `CircuitBreaker.fallback` composition pinned by rc-q16d
(new properties and a debug record are observable additions, not behavior
changes). New knob and properties are additive. No repository-trait or backend
changes.
