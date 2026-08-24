# Design: declarative-repository-stubs

## Approach

Follow the `beans:` delivery shape exactly (archived change
`bean-test-registry`): a new declarative block on `TestDocument`, eager
validation in `parse_test_document`, registration in the runner before
routes are added and compiled, spec delta in `mock-testkit`.

1. **Document surface** (`crates/camel-cli/src/commands/test/document.rs`):
   new field `repositories: Option<RepositoriesDoc>` on `TestDocument`
   (which already carries `deny_unknown_fields, rename_all = "camelCase"`).
   `RepositoriesDoc` holds three optional maps — `cache`, `idempotent`,
   `claimCheck` — each `BTreeMap<String, String>` (name → stub target).
   The only valid target in v1 is the literal `"memory"`; anything else is
   a document error listing the supported target. Blank repository names
   are document errors. The built-in name `memory` is rejected as a stub
   name (registration would collide with the built-in
   `RegistryError::AlreadyRegistered`). `RepositoriesDoc` uses
   `#[serde(flatten)] extra: BTreeMap<String, serde_yaml::Value>` instead of
   `deny_unknown_fields`: the noyalib serde_yaml shim discards serde's
   `_expected` field list, so a `deny_unknown_fields` error cannot name the
   supported kinds. `validate_repositories` rejects a non-empty `extra` with
   `InvalidRepositories` listing the three kinds.
2. **Runner wiring** (`crates/camel-cli/src/commands/test/runner.rs`):
   `run_test_doc` parses route definitions first (`load_routes`) and then
   boots; `boot_context` takes the parsed stub declarations and, after
   `builder().build()`, calls the existing public registration APIs —
   `CamelContext::register_cache_repository`,
   `register_idempotent_repository`,
   `register_claim_check_repository` — with fresh
   `MemoryCacheRepository::new(name, 10_000)`,
   `MemoryIdempotentRepository::new(name)`,
   `MemoryClaimCheckRepository::new(name)` under each declared name.
   Registration precedes `add_route_definition` (route compilation), so
   named repositories resolve at compile time. No `camel-core` change is
   needed: the registration APIs and memory backends already exist.
3. **Warning** (`runner.rs` / command layer): on any run whose document
   declares stubs, emit one stderr warning with the single code
   `R-REPOSITORY-STUB`, naming each stubbed registry and repository in the
   message text. Caveats are scoped per registry: cache stubs warn about
   prefix purge, backend-specific TTL/stale timing fidelity, disk offload,
   and backend-specific stats fidelity; idempotent and claim-check stubs
   warn about persistence; all warn about backend-failure paths and point
   to the integration tier. Advisory only; no `camel lint`
   rule (e_gpt decision 2026-08-24: lint never inspects `*.test.yaml`,
   `lint.rs:81-95`). The blessing gate collapsed the earlier
   `R-CACHE-STUB`-family codes into this single code.

Masking containment: a stub resolves only its explicitly declared name, so
typo'd names still hit the compile-time `ComponentNotFound` gate identical
to production — the one signal that catches routes wired to nonexistent
repositories. Residual, documented limitation: stubs bypass production
`Camel.toml` repository registration entirely, so a route whose production
backend configuration is missing or invalid still passes under stubs;
`Camel.toml` validity remains a production-boot concern. rc-q16d
(CircuitBreaker stale-cache fallback) stays integration-tier work per the
pinned consult decisions.

## Affected crates

- `camel-cli`: `commands/test/document.rs` (schema + validation),
  `commands/test/runner.rs` (registration + warning),
  `commands/test/document_tests/repositories.rs` (new test module).
- No other crate. `camel-core` registration APIs are consumed as-is.

## Architecture boundaries

Test-harness ergonomics only. No data/control plane change (ADR-0002 /
ADR-0045): stubs swap repository impls inside one in-process registry
before route compile; nothing threads through the CQRS read side. Tier
contract (ADR-0064): repositories are not lean-set components and carry no
inbound stimulus — the creep rule (§3) does not fire, and the block is
reviewer-gated per document, satisfying the §2 no-silent-growth principle
that a hardcoded boot alias would violate. `camel run` non-interference
mirrors the beans/intercepts requirements. No ADR-0046 consultation
(harness registration, not EIP semantics). Single-phase change.

## Alternatives considered

- **Hardcoded boot aliases** (register `persistent`/`redis` in
  `boot_context`): rejected — exactly ADR-0064 §2's silent
  `register_component`-line growth with no review; also global, so tests
  asserting `ComponentNotFound` for typos stop working.
- **Builder-level boot profile**: rejected — same global blast radius;
  splits repository truth between builder and document.
- **Cache-only block**: rejected for v1 shape — the gap is one structural
  gap across three registries (`context_builder.rs:227-251`); a cache-only
  block invites a third partial fix (e_gpt decision 2).
- **Non-aliasable name denylist** (forbid stubbing `redis` because
  `invalidate_prefix` diverges): rejected — the per-step semantic gaps are
  the hazard, not the names; warning + spec caveat carry that load
  (e_opus verdict Q3).
