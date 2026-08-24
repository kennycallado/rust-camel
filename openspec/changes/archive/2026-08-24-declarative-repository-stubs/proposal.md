# Proposal: declarative-repository-stubs

## Why

Routes that name a non-memory cache, idempotent, or claim-check repository
(`persistent`, `redis`, `redb`) fail to compile under `camel test`: the lean
boot inherits only the three built-in `memory` defaults
(`crates/camel-core/src/context_builder.rs:227-251`), and named backends
register only on the `camel run` / `Camel.toml` path
(`crates/camel-config/src/context_ext.rs`). The failure is the same
compile-time `ComponentNotFound` gate production uses
(`step_compilers/core.rs` `cache_clear_unknown_repository_fails_compile`),
so no `*.test.yaml` document can exercise such a route at all. The
backend-agnostic decision logic (hit/miss branching, `coalesce_misses`,
write-back skip, stale-fallback control flow) is structurally locked out of
the fast unit tier (bd rc-8869, epic rc-7roi; e_opus + e_gpt consults
2026-08-24).

## What Changes

Add a declarative `repositories:` block to the test document, mirroring the
established `intercepts:` / `beans:` grain:

```yaml
repositories:
  cache: { persistent: memory }
  idempotent: { redis: memory }
  claimCheck: { redb: memory }
```

The runner registers a `Memory*Repository` under each declared name before
routes are added and compiled, so named-repository routes compile and run
against in-memory stubs. Validation is eager (`deny_unknown_fields`,
exit 2). A per-run stderr warning (`R-REPOSITORY-STUB`) names the stubbed
registries and repositories and the semantics NOT exercised. Undeclared
names still fail `ComponentNotFound`. The built-in name `memory` cannot be
stubbed.

Excluded: non-memory stub targets, per-stub configuration, any `camel run`
behavior change, any change to repository registration names (rc-vl1l stays
separate), any ADR-0064 amendment (repositories are not lean-set components;
the creep rule does not fire).

## Acceptance criteria

- A route with `cache: { repository: persistent }` compiles and runs under
  `camel test` when the document declares `repositories: { cache: { persistent: memory } }`.
- Same for `idempotent:` (duplicate-input filtering proof) and
  `claimCheck:` (content round-trip proof at runtime).
- Undeclared/typo'd repository names still fail at route load with
  `ComponentNotFound` (exit 2 document error).
- Unknown registry kind, unknown stub target, blank repository name,
  stubbing the built-in `memory` name, unknown fields in the block:
  document errors at parse time (exit 2).
- Every run with stubs emits the `R-REPOSITORY-STUB` stderr warning naming
  unexercised surfaces (cache: prefix purge, backend-specific TTL/stale
  timing fidelity, disk offload, backend-specific stats fidelity;
  idempotent/claim-check: persistence; all: backend failure).
- `camel run` ignores the block entirely.

## Risk budget

Accepted: unit-tier masking risk mitigated by explicit per-document
declaration + warning + spec caveat routing backend semantics to the
integration tier (rc-kk69). Stubs bypass production `Camel.toml`
registration, so they cannot validate missing or invalid backend
configuration — that stays a production-boot concern. rc-q16d
(CircuitBreaker stale-cache fallback) remains integration-tier work.
Out of bounds: widening the lean component set, runner-level registry
mutation outside the document (ADR-0064 §2), stub targets beyond `memory`.

Bd: rc-8869
