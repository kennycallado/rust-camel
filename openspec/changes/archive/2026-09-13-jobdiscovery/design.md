# Design: jobdiscovery

## Approach

Extend the jobs configuration model with an ordered `dirs` collection and a compatibility `dir` input. Resolve both relative paths against the directory containing `Camel.toml`, never the process current directory. Keep the listing path as a metadata-only scanner: walk each root recursively in lexical entry order with a maximum depth of 8 and maximum of 512 encountered files per root, filter candidates through the existing `camel_dsl::discovery::is_job_document` predicate, and reuse the current description-only probe. Do not follow symlinked directories. Emit one truncation warning per truncated root when either cap is reached.

For a named job, probe the exact `<name>.job.yaml` candidate in each configured root in declaration order. Collect matches before selecting so a duplicate stem in different roots is an explicit exit-2 ambiguity naming all matching paths; otherwise select the first match. Missing and empty roots remain successful no-op listings. No `Discover::Patterns`, environment interpolation, route compilation, or security context is introduced by no-argument listing.

## Affected crates

- `crates/camel-config`: deserialize `[jobs].dirs`, preserve `dir` compatibility, and expose ordered resolved roots.
- `crates/camel-cli`: use ordered roots for listing and named-job lookup, bounded walking, filtering, truncation diagnostics, and collision errors.
- Existing `camel-dsl` discovery code: reuse `is_job_document`; no changes expected.
- CLI/config tests: cover compatibility, ordering, bounds, filtering, malformed siblings, missing roots, and collisions.
- `docs/adr/0062-reserved-test-suffix-and-placement-contract.md`: amend the job-root contract.
- `CONTEXT-MAP.md`: refresh the jobs-directory authority and discovery-set terminology.
- `crates/camel-cli/CONTEXT.md`: update the single-root discovery contract.

## Architecture boundaries

No-argument listing remains a control-plane metadata operation in the CLI/config layer. It does not enter Runtime lowering, DSL route discovery, Components, Services, Languages, or Functions. Named-job execution retains its existing normal loading and route compilation after resolution. The listing uses the reserved-document contract from ADR-0062, specifically one shared predicate with a separate bounded walker. Paths are anchored at configuration location, matching the configuration boundary documented in `CONTEXT-MAP.md`.

## Alternatives considered

- Reusing route `Discover::Patterns`: rejected because it performs interpolation and security-aware route discovery, which violates the metadata-only boundary.
- Adding a CLI glob: rejected by the ruling; discovery scope must be declarative and reproducible.
- Breaking `dir`: rejected because the pre-1.0 compatibility alias has low cost and preserves existing projects.
- Treating duplicate stems as first-wins: rejected because it hides configuration errors across the discovery set.
